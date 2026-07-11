import Foundation

enum GatewayModelResolver {
    static func resolveAuthorized(
        _ requested: String, capability: Capability, kind: GatewayWorkKind,
        port: any GatewayPort, identity: GatewayIdentity
    ) async throws -> ModelRecord {
        let shelf = try await port.shelf()
        let record = try resolve(requested, shelf: shelf, scopes: identity.scopes)
        try identity.require(modelID: record.id, capability: capability)
        try await GatewayBackpressure.require(port, record: record, kind: kind)
        return record
    }

    static func resolve(
        _ requested: String, shelf: [ModelRecord], scopes: GatewayScopes = .all
    ) throws -> ModelRecord {
        let ready = scopes.filter(shelf).filter { $0.state == .ready }
        if let exact = ready.first(where: { $0.id == requested }) { return exact }

        let tiers: [[ModelRecord]] = [
            ready.filter { $0.alias == requested },
            ready.filter { $0.name == requested },
            ready.filter { $0.name.caseInsensitiveCompare(requested) == .orderedSame },
            ready.filter { normalizedTag($0.name) == normalizedTag(requested) },
        ]
        for candidates in tiers {
            if candidates.count > 1 {
                let ids = candidates.map(\.id).sorted().joined(separator: ", ")
                throw GatewayError(
                    .badRequest,
                    "\(requested) matches more than one model — use an id: \(ids)")
            }
            if let match = candidates.first { return match }
        }

        throw GatewayError(.notFound, "no ready model matches \(requested)")
    }

    static func normalizedTag(_ name: String) -> String {
        let lowered = name.lowercased()
        if lowered.hasSuffix(":latest") {
            return String(lowered.dropLast(7))
        }
        return lowered
    }
}
