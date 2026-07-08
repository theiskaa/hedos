import Foundation

enum GatewayModelResolver {
    static func resolve(_ requested: String, shelf: [ModelRecord]) throws -> ModelRecord {
        let ready = shelf.filter { $0.state == .ready }
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
