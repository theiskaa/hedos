import Foundation

private final class SettingsSubscribers: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<String>.Continuation] = [:]

    func add(_ id: UUID, _ continuation: AsyncStream<String>.Continuation) {
        lock.lock()
        continuations[id] = continuation
        lock.unlock()
    }

    func remove(_ id: UUID) {
        lock.lock()
        continuations.removeValue(forKey: id)
        lock.unlock()
    }

    func yield(_ domain: String) {
        lock.lock()
        let all = Array(continuations.values)
        lock.unlock()
        for continuation in all {
            continuation.yield(domain)
        }
    }
}

public actor SettingsStore {
    public let directory: URL
    private var cache: [String: any SettingsDomain] = [:]
    private let subscribers = SettingsSubscribers()

    public init(directory: URL) {
        self.directory = directory
    }

    public nonisolated func changes() -> AsyncStream<String> {
        let id = UUID()
        let subscribers = subscribers
        return AsyncStream { continuation in
            subscribers.add(id, continuation)
            continuation.onTermination = { _ in
                subscribers.remove(id)
            }
        }
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
        try StoreCoding.encoder().encode(domain).write(to: fileURL(D.self), options: .atomic)
        cache[D.domainName] = domain
        subscribers.yield(D.domainName)
    }

    private func read<D: SettingsDomain>(_ type: D.Type) -> D {
        let url = fileURL(D.self)
        guard let data = try? Data(contentsOf: url) else {
            return D.compatibilityRead(from: directory) ?? D()
        }
        guard let decoded = try? StoreCoding.decoder().decode(D.self, from: data) else {
            StoreCoding.quarantine(url)
            return D()
        }
        return decoded
    }

    public func general() -> GeneralSettings { load(GeneralSettings.self) }
    public func models() -> ModelsSettings { load(ModelsSettings.self) }
    public func chat() -> ChatSettings { load(ChatSettings.self) }
    public func voice() -> VoiceSettings { load(VoiceSettings.self) }
    public func appearance() -> AppearanceSettings { load(AppearanceSettings.self) }
    public func advanced() -> AdvancedSettings { load(AdvancedSettings.self) }
    public func gateway() -> GatewaySettings { load(GatewaySettings.self) }

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
        try Self.rejectIfTooBroad(normalized)
        if !settings.watchedFolders.contains(normalized) {
            settings.watchedFolders.append(normalized)
            try save(settings)
        }
        return settings
    }

    static func rejectIfTooBroad(_ path: String) throws {
        let resolved = ManifestSupport.canonicalPath(URL(fileURLWithPath: path)).lowercased()
        if resolved == "/" {
            throw KernelError.runtimeFailed(
                "watching / would expose the entire filesystem to discovery")
        }
        let home = ManifestSupport.canonicalPath(
            FileManager.default.homeDirectoryForCurrentUser
        ).lowercased()
        if resolved == home || home.hasPrefix(resolved + "/") {
            throw KernelError.runtimeFailed(
                "watching \(path) would expose your entire home folder to discovery")
        }
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

    @discardableResult
    public func approveHostRuntime(_ id: String, contentHash: String? = nil, network: Bool) throws
        -> ModelsSettings
    {
        var settings = models()
        var changed = false
        if !settings.approvedHostRuntimes.contains(id) {
            settings.approvedHostRuntimes.append(id)
            changed = true
        }
        if let contentHash, settings.approvedHostRuntimeHashes[id] != contentHash {
            settings.approvedHostRuntimeHashes[id] = contentHash
            changed = true
        }
        if network {
            if !settings.approvedNetworkRuntimes.contains(id) {
                settings.approvedNetworkRuntimes.append(id)
                changed = true
            }
            if let contentHash, settings.approvedNetworkRuntimeHashes[id] != contentHash {
                settings.approvedNetworkRuntimeHashes[id] = contentHash
                changed = true
            }
        } else {
            if settings.approvedNetworkRuntimes.contains(id) {
                settings.approvedNetworkRuntimes.removeAll { $0 == id }
                changed = true
            }
            if settings.approvedNetworkRuntimeHashes[id] != nil {
                settings.approvedNetworkRuntimeHashes[id] = nil
                changed = true
            }
        }
        if changed {
            try save(settings)
        }
        return settings
    }

    @discardableResult
    public func revokeRuntime(_ id: String) throws -> ModelsSettings {
        var settings = models()
        var changed = false
        if settings.approvedHostRuntimes.contains(id) {
            settings.approvedHostRuntimes.removeAll { $0 == id }
            changed = true
        }
        if settings.approvedHostRuntimeHashes[id] != nil {
            settings.approvedHostRuntimeHashes[id] = nil
            changed = true
        }
        if settings.approvedNetworkRuntimes.contains(id) {
            settings.approvedNetworkRuntimes.removeAll { $0 == id }
            changed = true
        }
        if settings.approvedNetworkRuntimeHashes[id] != nil {
            settings.approvedNetworkRuntimeHashes[id] = nil
            changed = true
        }
        if changed {
            try save(settings)
        }
        return settings
    }

    @discardableResult
    public func addHFCacheRoot(_ path: String) throws -> ModelsSettings {
        var settings = models()
        let normalized = (path as NSString).expandingTildeInPath
        if !settings.hfCacheRoots.contains(normalized) {
            settings.hfCacheRoots.append(normalized)
            try save(settings)
        }
        return settings
    }

    @discardableResult
    public func removeHFCacheRoot(_ path: String) throws -> ModelsSettings {
        var settings = models()
        let normalized = (path as NSString).expandingTildeInPath
        if settings.hfCacheRoots.contains(normalized) {
            settings.hfCacheRoots.removeAll { $0 == normalized }
            try save(settings)
        }
        return settings
    }
}
