import Foundation

enum RuntimeBundle {
    static func directory(named name: String) -> URL? {
        guard let root = Bundle.kernelModule.resourceURL else { return nil }
        let candidates = [
            root.appendingPathComponent("Resources/Runtimes/\(name)"),
            root.appendingPathComponent("Runtimes/\(name)"),
            root.deletingLastPathComponent()
                .appendingPathComponent("Resources/Runtimes/\(name)"),
        ]
        return candidates.first { FileManager.default.fileExists(atPath: $0.path) }
    }
}
