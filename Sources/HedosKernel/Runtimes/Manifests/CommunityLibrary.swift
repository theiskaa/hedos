import Foundation

public struct CommunityLibrary: Sendable {
    public struct Recipe: Sendable, Hashable {
        public let manifest: RuntimeManifest
        public let directory: URL
    }

    let directories: [URL]

    public init(directories: [URL]? = nil) {
        self.directories = directories ?? Self.bundledDirectories()
    }

    static func bundledDirectories() -> [URL] {
        guard let root = Bundle.module.resourceURL else { return [] }
        let candidates = [
            root.appendingPathComponent("Resources/Community"),
            root.appendingPathComponent("Community"),
            root.deletingLastPathComponent().appendingPathComponent("Resources/Community"),
        ]
        guard
            let base = candidates.first(where: {
                FileManager.default.fileExists(atPath: $0.path)
            })
        else { return [] }
        let entries =
            (try? FileManager.default.contentsOfDirectory(
                at: base, includingPropertiesForKeys: [.isDirectoryKey])) ?? []
        return entries.filter {
            (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    public func recipes() -> [Recipe] {
        directories.compactMap { directory in
            guard let manifest = try? Self.loadManifest(directory) else { return nil }
            return Recipe(manifest: manifest, directory: directory)
        }
    }

    public func matches(record: ModelRecord) -> [Recipe] {
        recipes().filter { $0.manifest.detect?.matches(record) ?? false }
    }

    static func loadManifest(_ directory: URL) throws -> RuntimeManifest {
        let manifestURL = directory.appendingPathComponent("manifest.toml")
        let text = try String(contentsOf: manifestURL, encoding: .utf8)
        let table = try TOMLLite.parse(text)
        return try RuntimeManifest.load(table: table, directory: directory)
    }
}
