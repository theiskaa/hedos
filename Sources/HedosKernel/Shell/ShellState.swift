import Foundation

public enum AppMode: String, Codable, CaseIterable, Sendable {
    case home
    case chat
    case images
    case voice
    case pipelines
    case library
    case settings

    public var ordinal: Int {
        switch self {
        case .home: 0
        case .chat: 1
        case .images: 2
        case .voice: 3
        case .pipelines: 4
        case .library: 5
        case .settings: 6
        }
    }

    public static func at(ordinal: Int) -> AppMode? {
        allCases.first { $0.ordinal == ordinal }
    }
}

public enum Launcher {
    public static func destination(for record: ModelRecord) -> AppMode {
        guard record.runtime.id != nil, record.runtime.tier != .recipeNeeded else {
            return .library
        }
        if record.capabilities.contains(.chat) { return .chat }
        if record.capabilities.contains(.speak) { return .voice }
        if record.capabilities.contains(.image) { return .images }
        return .library
    }

    public static func defaultChatModel(
        in shelf: [ModelRecord], preferring id: String? = nil
    ) -> ModelRecord? {
        let candidates = shelf.filter { $0.state == .ready && destination(for: $0) == .chat }
        if let id, let preferred = candidates.first(where: { $0.id == id }) {
            return preferred
        }
        return candidates.first
    }

    public static func models(in shelf: [ModelRecord], for mode: AppMode) -> [ModelRecord] {
        shelf.filter { belongs($0, to: mode) }
    }

    private static func belongs(_ record: ModelRecord, to mode: AppMode) -> Bool {
        let destination = destination(for: record)
        if destination == mode { return true }
        guard destination == .library, mode != .library else { return false }
        switch mode {
        case .chat:
            return record.capabilities.contains(.chat) || record.modality == .text
        case .images:
            return record.capabilities.contains(.image) || record.modality == .image
        case .voice:
            return record.capabilities.contains(.speak) || record.modality == .speech
                || record.modality == .audio
        default:
            return false
        }
    }
}

public struct ShellState: Codable, Sendable, Equatable {
    public var mode: AppMode
    public var chatSessionID: String?
    public var imagesSelection: String?
    public var voiceModelID: String?
    public var pipelineSelection: String?
    public var libraryModelID: String?
    public var sidebarCollapsed: Bool

    public init(
        mode: AppMode = .home,
        chatSessionID: String? = nil,
        imagesSelection: String? = nil,
        voiceModelID: String? = nil,
        pipelineSelection: String? = nil,
        libraryModelID: String? = nil,
        sidebarCollapsed: Bool = false
    ) {
        self.mode = mode
        self.chatSessionID = chatSessionID
        self.imagesSelection = imagesSelection
        self.voiceModelID = voiceModelID
        self.pipelineSelection = pipelineSelection
        self.libraryModelID = libraryModelID
        self.sidebarCollapsed = sidebarCollapsed
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let rawMode = try container.decodeIfPresent(String.self, forKey: .mode)
        mode = rawMode.flatMap(AppMode.init(rawValue:)) ?? .home
        chatSessionID = try container.decodeIfPresent(String.self, forKey: .chatSessionID)
        imagesSelection = try container.decodeIfPresent(String.self, forKey: .imagesSelection)
        voiceModelID = try container.decodeIfPresent(String.self, forKey: .voiceModelID)
        pipelineSelection = try container.decodeIfPresent(String.self, forKey: .pipelineSelection)
        libraryModelID = try container.decodeIfPresent(String.self, forKey: .libraryModelID)
        sidebarCollapsed =
            (try container.decodeIfPresent(Bool.self, forKey: .sidebarCollapsed)) ?? false
    }

    public func selection(in mode: AppMode) -> String? {
        switch mode {
        case .chat: chatSessionID
        case .images: imagesSelection
        case .voice: voiceModelID
        case .pipelines: pipelineSelection
        case .library: libraryModelID
        case .home, .settings: nil
        }
    }

    public mutating func setSelection(_ id: String?, in mode: AppMode) {
        switch mode {
        case .chat: chatSessionID = id
        case .images: imagesSelection = id
        case .voice: voiceModelID = id
        case .pipelines: pipelineSelection = id
        case .library: libraryModelID = id
        case .home, .settings: break
        }
    }
}
