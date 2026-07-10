import Foundation

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
