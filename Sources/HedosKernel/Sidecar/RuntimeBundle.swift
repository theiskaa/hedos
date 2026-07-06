import Foundation

enum RuntimeBundle {
    static func directory(named name: String) -> URL? {
        guard let root = Bundle.module.resourceURL else { return nil }
        let candidates = [
            root.appendingPathComponent("Resources/Runtimes/\(name)"),
            root.appendingPathComponent("Runtimes/\(name)"),
            root.deletingLastPathComponent()
                .appendingPathComponent("Resources/Runtimes/\(name)"),
        ]
        return candidates.first { FileManager.default.fileExists(atPath: $0.path) }
    }
}

enum SidecarModelPaths {
    static func resolve(_ record: ModelRecord) -> (sandboxRoot: String, snapshot: String) {
        let base = URL(
            fileURLWithPath: (record.source.path as NSString).expandingTildeInPath)
        let root = base.resolvingSymlinksInPath()
        if record.source.kind == .huggingfaceCache, let ref = record.source.ref {
            let snapshot = root.appendingPathComponent("snapshots/\(ref)")
            if FileManager.default.fileExists(atPath: snapshot.path) {
                return (root.path, snapshot.path)
            }
        }
        return (root.path, root.path)
    }
}
