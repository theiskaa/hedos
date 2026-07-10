import Foundation

public struct RuntimeBid: Sendable, Hashable {
    public var tier: RunTier
    public var preference: Int
    public var alternatives: [String]

    public init(tier: RunTier, preference: Int, alternatives: [String] = []) {
        self.tier = tier
        self.preference = preference
        self.alternatives = alternatives
    }
}

public struct AdapterBidReport: Sendable, Hashable {
    public var adapterID: String
    public var tier: RunTier
    public var preference: Int

    public init(adapterID: String, tier: RunTier, preference: Int) {
        self.adapterID = adapterID
        self.tier = tier
        self.preference = preference
    }
}

public struct ResolutionExplanation: Sendable {
    public var record: ModelRecord
    public var identified: IdentifiedModel
    public var bids: [AdapterBidReport]

    public var winner: String? { bids.first?.adapterID }

    public init(record: ModelRecord, identified: IdentifiedModel, bids: [AdapterBidReport]) {
        self.record = record
        self.identified = identified
        self.bids = bids
    }
}

public actor ResolutionEngine {
    private let adapters: [any RuntimeAdapter]
    private let profiles: ProfileRegistry
    private let identificationCache: IdentificationCache?

    public init(
        adapters: [any RuntimeAdapter], profiles: ProfileRegistry = .builtin,
        identificationCache: IdentificationCache? = nil
    ) {
        self.adapters = adapters
        self.profiles = profiles
        self.identificationCache = identificationCache
    }

    public func resolveAll(in registry: Registry, kinds: Set<SourceKind>? = nil) async throws {
        var changed: [ModelRecord] = []
        for record in try await registry.list() {
            if let kinds, !kinds.contains(record.source.kind) { continue }
            if let updated = resolvedUpdate(for: record) {
                changed.append(updated)
            }
        }
        try await registry.register(contentsOf: changed)
    }

    public func resolve(_ record: ModelRecord, in registry: Registry) async throws {
        if let updated = resolvedUpdate(for: record) {
            try await registry.register(updated)
        }
    }

    private func resolvedUpdate(for record: ModelRecord) -> ModelRecord? {
        if record.runtime.resolved == .user,
            let pinned = record.runtime.id,
            adapters.contains(where: { $0.id == pinned })
        {
            return nil
        }
        guard record.state != .missing else { return nil }

        let identified = identificationCache?.identify(record) ?? Identification.identify(record)
        let bids = collectBids(record, identified)

        var updated = record
        if let winner = bids.first {
            let previous = record.runtime
            var alternatives = bids.dropFirst().map(\.id)
            for declared in winner.bid.alternatives
            where declared != winner.id && !alternatives.contains(declared) {
                alternatives.append(declared)
            }
            updated.runtime = RuntimeRef(
                id: winner.id,
                resolved: .auto,
                tier: winner.bid.tier,
                alternatives: alternatives,
                confirmedAt: previous.id == winner.id ? previous.confirmedAt : nil)
            updated.state = .ready
        } else {
            updated.runtime = RuntimeRef(
                id: nil, resolved: .unresolved, tier: .recipeNeeded, alternatives: [])
            updated.state = .unresolved
        }
        if let modality = identified.modality { updated.modality = modality }
        if !identified.capabilities.isEmpty { updated.capabilities = identified.capabilities }
        if !identified.params.isEmpty { updated.params = identified.params }
        if let contextLength = identified.contextLength { updated.contextLength = contextLength }
        if let hasChatTemplate = identified.hasChatTemplate {
            updated.hasChatTemplate = hasChatTemplate
        }
        updated.execution = identified.execution
        if let winner = bids.first,
            let backed = adapters.first(where: { $0.id == winner.id }) as? any ManifestBacked
        {
            if identified.modality == nil, let modality = backed.manifest.modalities.first {
                updated.modality = modality
            }
            if identified.capabilities.isEmpty {
                updated.capabilities = backed.manifest.capabilities
                updated.execution = backed.manifest.execution
            }
        }
        if identified.params.isEmpty {
            updated = profiles.refreshed(updated)
        }

        return updated != record ? updated : nil
    }

    public func explain(_ record: ModelRecord) -> ResolutionExplanation {
        let identified = Identification.identify(record)
        let bids = collectBids(record, identified).map {
            AdapterBidReport(adapterID: $0.id, tier: $0.bid.tier, preference: $0.bid.preference)
        }
        return ResolutionExplanation(record: record, identified: identified, bids: bids)
    }

    public func explainAll(in registry: Registry) async throws -> [ResolutionExplanation] {
        try await registry.list().map { explain($0) }
    }

    private func collectBids(
        _ record: ModelRecord, _ identified: IdentifiedModel
    ) -> [(id: String, bid: RuntimeBid)] {
        adapters.compactMap { adapter -> (id: String, bid: RuntimeBid)? in
            guard let bid = adapter.bid(record, identified) else { return nil }
            return (adapter.id, bid)
        }
        .sorted { $0.bid.preference < $1.bid.preference }
    }
}
