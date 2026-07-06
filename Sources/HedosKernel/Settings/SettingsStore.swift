import Foundation

public struct HedosSettings: Codable, Sendable, Equatable {
    public var schemaVersion: Int
    public var watchedFolders: [String]
    public var shell: ShellState

    public init(
        schemaVersion: Int = 1, watchedFolders: [String] = [], shell: ShellState = ShellState()
    ) {
        self.schemaVersion = schemaVersion
        self.watchedFolders = watchedFolders
        self.shell = shell
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decodeIfPresent(Int.self, forKey: .schemaVersion) ?? 1
        watchedFolders = try container.decodeIfPresent([String].self, forKey: .watchedFolders) ?? []
        shell = try container.decodeIfPresent(ShellState.self, forKey: .shell) ?? ShellState()
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

    public func shellState() throws -> ShellState {
        try load().shell
    }

    public func saveShellState(_ shell: ShellState) throws {
        var settings = try load()
        guard settings.shell != shell else { return }
        settings.shell = shell
        try save(settings)
    }

    public func removeWatchedFolder(_ path: String) throws -> HedosSettings {
        var settings = try load()
        let normalized = (path as NSString).expandingTildeInPath
        settings.watchedFolders.removeAll { $0 == normalized }
        try save(settings)
        return settings
    }
}
