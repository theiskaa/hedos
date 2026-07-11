import Foundation

public enum ChatExportFormat: String, Codable, Sendable, CaseIterable {
    case markdown
    case json
}

public struct ChatSettings: SettingsDomain {
    public static let domainName = "chat"

    public var defaultModelID: String?
    public var defaultSystemPrompt: String?
    public var showStats: Bool
    public var sendWithEnter: Bool
    public var exportFormat: ChatExportFormat

    public init() {
        defaultModelID = nil
        defaultSystemPrompt = nil
        showStats = true
        sendWithEnter = true
        exportFormat = .markdown
    }

    public init(
        defaultModelID: String? = nil, defaultSystemPrompt: String? = nil,
        showStats: Bool = true, sendWithEnter: Bool = true,
        exportFormat: ChatExportFormat = .markdown
    ) {
        self.defaultModelID = defaultModelID
        self.defaultSystemPrompt = defaultSystemPrompt
        self.showStats = showStats
        self.sendWithEnter = sendWithEnter
        self.exportFormat = exportFormat
    }

    enum CodingKeys: String, CodingKey {
        case defaultModelID, defaultSystemPrompt, showStats, sendWithEnter, exportFormat
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        defaultModelID = container.lenient(String.self, .defaultModelID)
        defaultSystemPrompt = container.lenient(String.self, .defaultSystemPrompt)
        showStats = container.lenient(Bool.self, .showStats, fallback: defaults.showStats)
        sendWithEnter = container.lenient(
            Bool.self, .sendWithEnter, fallback: defaults.sendWithEnter)
        exportFormat = container.lenient(
            ChatExportFormat.self, .exportFormat, fallback: defaults.exportFormat)
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
