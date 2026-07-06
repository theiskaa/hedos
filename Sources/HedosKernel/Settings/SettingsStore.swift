import Foundation

public actor SettingsStore {
    public let directory: URL
    private var cache: [String: any SettingsDomain] = [:]

    public init(directory: URL) {
        self.directory = directory
    }

    private var settingsDirectory: URL {
        directory.appendingPathComponent("settings", isDirectory: true)
    }

    private func fileURL<D: SettingsDomain>(_ type: D.Type) -> URL {
        settingsDirectory.appendingPathComponent("\(D.domainName).json")
    }

    public func load<D: SettingsDomain>(_ type: D.Type) -> D {
        if let cached = cache[D.domainName] as? D { return cached }
        let loaded = read(D.self)
        cache[D.domainName] = loaded
        return loaded
    }

    public func save<D: SettingsDomain>(_ domain: D) throws {
        try FileManager.default.createDirectory(
            at: settingsDirectory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(domain).write(to: fileURL(D.self), options: .atomic)
        cache[D.domainName] = domain
    }

    private func read<D: SettingsDomain>(_ type: D.Type) -> D {
        guard let data = try? Data(contentsOf: fileURL(D.self)) else {
            return D.compatibilityRead(from: directory) ?? D()
        }
        return (try? JSONDecoder().decode(D.self, from: data)) ?? D()
    }

    public func general() -> GeneralSettings { load(GeneralSettings.self) }
    public func models() -> ModelsSettings { load(ModelsSettings.self) }
    public func chat() -> ChatSettings { load(ChatSettings.self) }
    public func voice() -> VoiceSettings { load(VoiceSettings.self) }
    public func appearance() -> AppearanceSettings { load(AppearanceSettings.self) }
    public func advanced() -> AdvancedSettings { load(AdvancedSettings.self) }

    public func shellState() -> ShellState {
        load(ShellSettings.self).shell
    }

    public func saveShellState(_ shell: ShellState) throws {
        var settings = load(ShellSettings.self)
        guard settings.shell != shell else { return }
        settings.shell = shell
        try save(settings)
    }

    public func defaultChatModelID() -> String? {
        chat().defaultModelID
    }

    public func setDefaultChatModelID(_ modelID: String?) throws {
        var settings = chat()
        guard settings.defaultModelID != modelID else { return }
        settings.defaultModelID = modelID
        try save(settings)
    }

    @discardableResult
    public func addWatchedFolder(_ path: String) throws -> ModelsSettings {
        var settings = models()
        let normalized = (path as NSString).expandingTildeInPath
        if !settings.watchedFolders.contains(normalized) {
            settings.watchedFolders.append(normalized)
            try save(settings)
        }
        return settings
    }

    @discardableResult
    public func removeWatchedFolder(_ path: String) throws -> ModelsSettings {
        var settings = models()
        let normalized = (path as NSString).expandingTildeInPath
        if settings.watchedFolders.contains(normalized) {
            settings.watchedFolders.removeAll { $0 == normalized }
            try save(settings)
        }
        return settings
    }
}
