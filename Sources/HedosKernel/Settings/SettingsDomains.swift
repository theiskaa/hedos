import Foundation

public protocol SettingsDomain: Codable, Sendable, Equatable {
    static var domainName: String { get }
    static func compatibilityRead(from directory: URL) -> Self?
    init()
}

extension SettingsDomain {
    public static func compatibilityRead(from directory: URL) -> Self? {
        nil
    }
}

extension KeyedDecodingContainer {
    func lenient<T: Decodable>(_ type: T.Type, _ key: Key, fallback: T) -> T {
        (try? decodeIfPresent(type, forKey: key)) ?? fallback
    }

    func lenient<T: Decodable>(_ type: T.Type, _ key: Key) -> T? {
        try? decodeIfPresent(type, forKey: key)
    }
}

public struct GeneralSettings: SettingsDomain {
    public static let domainName = "general"

    public var restoreLastSession: Bool

    public init() {
        restoreLastSession = true
    }

    public init(restoreLastSession: Bool) {
        self.restoreLastSession = restoreLastSession
    }

    enum CodingKeys: String, CodingKey {
        case restoreLastSession
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        restoreLastSession = container.lenient(
            Bool.self, .restoreLastSession, fallback: defaults.restoreLastSession)
    }
}

public struct ModelsSettings: SettingsDomain {
    public static let domainName = "models"

    public var watchedFolders: [String]
    public var keepWarm: KeepWarmPolicy
    public var eviction: EvictionPolicy
    public var ramBudgetMB: Int?

    public init() {
        watchedFolders = []
        keepWarm = .fiveMinutes
        eviction = .strictSingle
        ramBudgetMB = nil
    }

    public init(
        watchedFolders: [String] = [],
        keepWarm: KeepWarmPolicy = .fiveMinutes,
        eviction: EvictionPolicy = .strictSingle,
        ramBudgetMB: Int? = nil
    ) {
        self.watchedFolders = watchedFolders
        self.keepWarm = keepWarm
        self.eviction = eviction
        self.ramBudgetMB = ramBudgetMB
    }

    public var residencyPolicy: ResidencyPolicy {
        ResidencyPolicy(keepWarm: keepWarm, eviction: eviction, ramBudgetMB: ramBudgetMB)
    }

    enum CodingKeys: String, CodingKey {
        case watchedFolders, keepWarm, eviction, ramBudgetMB
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        watchedFolders = container.lenient(
            [String].self, .watchedFolders, fallback: defaults.watchedFolders)
        keepWarm = container.lenient(KeepWarmPolicy.self, .keepWarm, fallback: defaults.keepWarm)
        eviction = container.lenient(EvictionPolicy.self, .eviction, fallback: defaults.eviction)
        ramBudgetMB = container.lenient(Int.self, .ramBudgetMB)
    }

    public static func compatibilityRead(from directory: URL) -> ModelsSettings? {
        let legacy = directory.appendingPathComponent("settings.json")
        guard let data = try? Data(contentsOf: legacy),
            let value = try? JSONDecoder().decode(JSONValue.self, from: data),
            case .object(let fields) = value,
            case .array(let folders)? = fields["watchedFolders"]
        else { return nil }
        var settings = ModelsSettings()
        settings.watchedFolders = folders.compactMap {
            guard case .string(let path) = $0 else { return nil }
            return path
        }
        return settings
    }
}

public struct ChatSettings: SettingsDomain {
    public static let domainName = "chat"

    public var defaultModelID: String?
    public var defaultSystemPrompt: String?

    public init() {
        defaultModelID = nil
        defaultSystemPrompt = nil
    }

    public init(defaultModelID: String? = nil, defaultSystemPrompt: String? = nil) {
        self.defaultModelID = defaultModelID
        self.defaultSystemPrompt = defaultSystemPrompt
    }

    enum CodingKeys: String, CodingKey {
        case defaultModelID, defaultSystemPrompt
    }

    public init(from decoder: any Decoder) throws {
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = Self()
            return
        }
        defaultModelID = container.lenient(String.self, .defaultModelID)
        defaultSystemPrompt = container.lenient(String.self, .defaultSystemPrompt)
    }

    public static func compatibilityRead(from directory: URL) -> ChatSettings? {
        struct Legacy: Decodable {
            var defaultChatModelID: String?
        }
        let legacy = directory.appendingPathComponent("settings.json")
        guard let data = try? Data(contentsOf: legacy),
            let value = try? JSONDecoder().decode(Legacy.self, from: data),
            let modelID = value.defaultChatModelID
        else { return nil }
        return ChatSettings(defaultModelID: modelID)
    }
}

public struct ShellSettings: SettingsDomain {
    public static let domainName = "shell"

    public var shell: ShellState

    public init() {
        shell = ShellState()
    }

    public init(shell: ShellState) {
        self.shell = shell
    }

    enum CodingKeys: String, CodingKey {
        case shell
    }

    public init(from decoder: any Decoder) throws {
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = Self()
            return
        }
        shell = container.lenient(ShellState.self, .shell, fallback: ShellState())
    }

    public static func compatibilityRead(from directory: URL) -> ShellSettings? {
        struct Legacy: Decodable {
            var shell: ShellState?
        }
        let legacy = directory.appendingPathComponent("settings.json")
        guard let data = try? Data(contentsOf: legacy),
            let value = try? JSONDecoder().decode(Legacy.self, from: data),
            let shell = value.shell
        else { return nil }
        return ShellSettings(shell: shell)
    }
}

public struct VoiceSettings: SettingsDomain {
    public static let domainName = "voice"

    public var defaultVoice: String?
    public var speed: Double
    public var autoSpeak: Bool

    public init() {
        defaultVoice = nil
        speed = 1.0
        autoSpeak = false
    }

    public init(defaultVoice: String? = nil, speed: Double = 1.0, autoSpeak: Bool = false) {
        self.defaultVoice = defaultVoice
        self.speed = speed
        self.autoSpeak = autoSpeak
    }

    enum CodingKeys: String, CodingKey {
        case defaultVoice, speed, autoSpeak
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        defaultVoice = container.lenient(String.self, .defaultVoice)
        speed = container.lenient(Double.self, .speed, fallback: defaults.speed)
        autoSpeak = container.lenient(Bool.self, .autoSpeak, fallback: defaults.autoSpeak)
    }
}

public struct AppearanceSettings: SettingsDomain {
    public static let domainName = "appearance"

    public enum Theme: String, Codable, Sendable, CaseIterable {
        case system, light, dark
    }

    public enum ChatWidth: String, Codable, Sendable, CaseIterable {
        case comfortable, wide
    }

    public enum Density: String, Codable, Sendable, CaseIterable {
        case relaxed, compact
    }

    public var theme: Theme
    public var chatWidth: ChatWidth
    public var density: Density

    public init() {
        theme = .system
        chatWidth = .comfortable
        density = .relaxed
    }

    public init(
        theme: Theme = .system,
        chatWidth: ChatWidth = .comfortable,
        density: Density = .relaxed
    ) {
        self.theme = theme
        self.chatWidth = chatWidth
        self.density = density
    }

    enum CodingKeys: String, CodingKey {
        case theme, chatWidth, density
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        theme = container.lenient(Theme.self, .theme, fallback: defaults.theme)
        chatWidth = container.lenient(ChatWidth.self, .chatWidth, fallback: defaults.chatWidth)
        density = container.lenient(Density.self, .density, fallback: defaults.density)
    }
}

public struct AdvancedSettings: SettingsDomain {
    public static let domainName = "advanced"

    public var jobHistoryLimit: Int

    public init() {
        jobHistoryLimit = 50
    }

    public init(jobHistoryLimit: Int = 50) {
        self.jobHistoryLimit = jobHistoryLimit
    }

    enum CodingKeys: String, CodingKey {
        case jobHistoryLimit
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        jobHistoryLimit = container.lenient(
            Int.self, .jobHistoryLimit, fallback: defaults.jobHistoryLimit)
    }
}
