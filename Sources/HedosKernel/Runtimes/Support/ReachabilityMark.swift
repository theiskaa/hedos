import Foundation

enum ReachabilityMark {
    static func reachable(_ id: String, kind: SourceKind, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == kind,
            record.state != .ready
        else { return }
        _ = try? await registry.setStateIfPresent(id: id, to: .ready)
    }

    static func unreachable(_ id: String, kind: SourceKind, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == kind,
            record.state != .missing
        else { return }
        _ = try? await registry.setStateIfPresent(id: id, to: .missing)
    }
}
