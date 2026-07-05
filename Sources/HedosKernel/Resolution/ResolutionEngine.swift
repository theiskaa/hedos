import Foundation

public struct RuntimeBid: Sendable, Hashable {
    public var tier: RunTier
    public var preference: Int

    public init(tier: RunTier, preference: Int) {
        self.tier = tier
        self.preference = preference
    }
}

public actor ResolutionEngine {
    private let adapters: [any RuntimeAdapter]

    public init(adapters: [any RuntimeAdapter]) {
        self.adapters = adapters
    }

    public func resolveAll(in registry: Registry) async throws {
        for record in try await registry.list() {
            try await resolve(record, in: registry)
        }
    }

    public func resolve(_ record: ModelRecord, in registry: Registry) async throws {
        guard record.runtime.resolved != .user else { return }
        guard record.state != .missing else { return }

        let identified = Identification.identify(record)
        let bids = adapters.compactMap { adapter -> (id: String, bid: RuntimeBid)? in
            guard let bid = adapter.bid(record, identified) else { return nil }
            return (adapter.id, bid)
        }
        .sorted { $0.bid.preference < $1.bid.preference }

        var updated = record
        if let winner = bids.first {
            let previous = record.runtime
            updated.runtime = RuntimeRef(
                id: winner.id,
                resolved: .auto,
                tier: winner.bid.tier,
                alternatives: bids.dropFirst().map(\.id),
                confirmedAt: previous.id == winner.id ? previous.confirmedAt : nil)
            updated.state = .ready
        } else {
            updated.runtime = RuntimeRef(
                id: nil, resolved: .unresolved, tier: .recipeNeeded, alternatives: [])
            updated.state = .unresolved
        }
        if let modality = identified.modality { updated.modality = modality }
        if !identified.capabilities.isEmpty { updated.capabilities = identified.capabilities }
        updated.execution = identified.execution

        if updated != record {
            try await registry.register(updated)
        }
    }
}
