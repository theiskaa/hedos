import Foundation

public struct HedosSettings: Codable, Sendable, Equatable {
    public var schemaVersion: Int
    public var watchedFolders: [String]

    public init(schemaVersion: Int = 1, watchedFolders: [String] = []) {
        self.schemaVersion = schemaVersion
        self.watchedFolders = watchedFolders
    }
}

public actor SettingsStore {
    public let directory: URL
    private var cached: HedosSettings?

    public init(directory: URL) {
        self.directory = directory
    }

    private var fileURL: URL {
        directory.appendingPathComponent("settings.json")
    }

    public func load() throws -> HedosSettings {
        if let cached { return cached }
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            let fresh = HedosSettings()
            cached = fresh
            return fresh
        }
        let settings = try JSONDecoder().decode(
            HedosSettings.self, from: Data(contentsOf: fileURL))
        cached = settings
        return settings
    }

    public func save(_ settings: HedosSettings) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(settings).write(to: fileURL, options: .atomic)
        cached = settings
    }

    public func addWatchedFolder(_ path: String) throws -> HedosSettings {
        var settings = try load()
        let normalized = (path as NSString).expandingTildeInPath
        if !settings.watchedFolders.contains(normalized) {
            settings.watchedFolders.append(normalized)
            try save(settings)
        }
        return settings
    }

    public func removeWatchedFolder(_ path: String) throws -> HedosSettings {
        var settings = try load()
        let normalized = (path as NSString).expandingTildeInPath
        settings.watchedFolders.removeAll { $0 == normalized }
        try save(settings)
        return settings
    }
}
