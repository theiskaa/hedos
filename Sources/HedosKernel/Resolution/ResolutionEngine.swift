import Foundation

public struct RuntimeBid: Sendable, Hashable {
    public var tier: RunTier
    public var preference: Int
    public var alternatives: [RuntimeID]

    public init(tier: RunTier, preference: Int, alternatives: [RuntimeID] = []) {
        self.tier = tier
        self.preference = preference
        self.alternatives = alternatives
    }
}

struct BidEntry: Sendable, Hashable {
    let id: RuntimeID
    let bid: RuntimeBid
}

struct ResolutionPlan: Sendable {
    let identified: IdentifiedModel
    let bids: [BidEntry]
    let backfillManifest: RuntimeManifest?
    let profiles: ProfileRegistry
}

public struct AdapterBidReport: Sendable, Hashable {
    public var adapterID: RuntimeID
    public var tier: RunTier
    public var preference: Int

    public init(adapterID: RuntimeID, tier: RunTier, preference: Int) {
        self.adapterID = adapterID
        self.tier = tier
        self.preference = preference
    }
}

public struct ResolutionExplanation: Sendable {
    public var record: ModelRecord
    public var identified: IdentifiedModel
    public var bids: [AdapterBidReport]

    public var winner: RuntimeID? { bids.first?.adapterID }

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

    private var liveAdapterIDs: Set<RuntimeID> { Set(adapters.map(\.id)) }

    public func resolveAll(in registry: Registry, kinds: Set<SourceKind>? = nil) async throws {
        var built: [String: ResolutionPlan] = [:]
        for record in try await registry.list() {
            if let kinds, !kinds.contains(record.source.kind) { continue }
            if let plan = plan(for: record) { built[record.id] = plan }
        }
        let plans = built
        guard !plans.isEmpty else { return }
        let live = liveAdapterIDs
        try await registry.update(ids: Array(plans.keys)) { current in
            guard let plan = plans[current.id] else { return nil }
            return Self.applied(plan, to: current, liveAdapterIDs: live)
        }
    }

    public func resolve(_ record: ModelRecord, in registry: Registry) async throws {
        guard let plan = plan(for: record) else { return }
        let live = liveAdapterIDs
        try await registry.update(id: record.id) { current in
            Self.applied(plan, to: current, liveAdapterIDs: live)
        }
    }

    private func plan(for record: ModelRecord) -> ResolutionPlan? {
        if record.runtime.resolved == .user,
            let pinned = record.runtime.id,
            adapters.contains(where: { $0.id == pinned })
        {
            return nil
        }
        guard record.state != .missing else { return nil }

        let identified = identificationCache?.identify(record) ?? Identification.identify(record)
        let bids = collectBids(record, identified)
        let backfillManifest = bids.first.flatMap { winner in
            (adapters.first(where: { $0.id == winner.id }) as? any ManifestBacked)?.manifest
        }
        return ResolutionPlan(
            identified: identified, bids: bids,
            backfillManifest: backfillManifest, profiles: profiles)
    }

    private static func applied(
        _ plan: ResolutionPlan, to record: ModelRecord, liveAdapterIDs: Set<RuntimeID>
    ) -> ModelRecord? {
        if record.runtime.resolved == .user,
            let pinned = record.runtime.id,
            liveAdapterIDs.contains(pinned)
        {
            return nil
        }
        guard record.state != .missing else { return nil }
        if record.downloading {
            var updated = record
            updated.runtime = RuntimeRef(
                id: nil, resolved: .unresolved, tier: .recipeNeeded, alternatives: [])
            updated.state = .unresolved
            return updated != record ? updated : nil
        }

        var updated = record
        applyWinner(plan.bids, to: &updated, previous: record.runtime)
        merge(identified: plan.identified, into: &updated)
        backfill(manifest: plan.backfillManifest, identified: plan.identified, into: &updated)
        if plan.identified.params.isEmpty {
            updated = plan.profiles.refreshed(updated)
        }
        return updated != record ? updated : nil
    }

    private static func applyWinner(
        _ bids: [BidEntry], to updated: inout ModelRecord, previous: RuntimeRef
    ) {
        guard let winner = bids.first else {
            updated.runtime = RuntimeRef(
                id: nil, resolved: .unresolved, tier: .recipeNeeded, alternatives: [])
            updated.state = .unresolved
            return
        }
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
    }

    private static func merge(identified: IdentifiedModel, into updated: inout ModelRecord) {
        if let modality = identified.modality { updated.modality = modality }
        if !identified.capabilities.isEmpty { updated.capabilities = identified.capabilities }
        if !identified.params.isEmpty { updated.params = identified.params }
        if let contextLength = identified.contextLength { updated.contextLength = contextLength }
        if let hasChatTemplate = identified.hasChatTemplate {
            updated.hasChatTemplate = hasChatTemplate
        }
        updated.execution = identified.execution
    }

    private static func backfill(
        manifest: RuntimeManifest?, identified: IdentifiedModel, into updated: inout ModelRecord
    ) {
        guard let manifest else { return }
        if identified.modality == nil, let modality = manifest.modalities.first {
            updated.modality = modality
        }
        if identified.capabilities.isEmpty {
            updated.capabilities = manifest.capabilities
            updated.execution = manifest.execution
        }
    }

    public func explain(_ record: ModelRecord) -> ResolutionExplanation {
        let identified = identificationCache?.identify(record) ?? Identification.identify(record)
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
    ) -> [BidEntry] {
        adapters.compactMap { adapter -> BidEntry? in
            guard let bid = adapter.bid(record, identified) else { return nil }
            return BidEntry(id: adapter.id, bid: bid)
        }
        .sorted { ($0.bid.preference, $0.id.rawValue) < ($1.bid.preference, $1.id.rawValue) }
    }
}
