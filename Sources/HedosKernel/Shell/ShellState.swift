import Foundation

public enum AppMode: String, Codable, CaseIterable, Sendable {
    case chat
    case images
    case voice
    case library
    case settings

    public var ordinal: Int {
        Self.allCases.firstIndex(of: self)! + 1
    }

    public static func at(ordinal: Int) -> AppMode? {
        guard ordinal >= 1, ordinal <= allCases.count else { return nil }
        return allCases[ordinal - 1]
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

    public static func defaultChatModel(in shelf: [ModelRecord]) -> ModelRecord? {
        shelf.first { $0.state == .ready && destination(for: $0) == .chat }
    }

    public static func models(in shelf: [ModelRecord], for mode: AppMode) -> [ModelRecord] {
        shelf.filter { destination(for: $0) == mode }
    }
}

public struct ShellState: Codable, Sendable, Equatable {
    public var mode: AppMode
    public var chatSessionID: String?
    public var imagesSelection: String?
    public var voiceModelID: String?
    public var libraryModelID: String?

    public init(
        mode: AppMode = .library,
        chatSessionID: String? = nil,
        imagesSelection: String? = nil,
        voiceModelID: String? = nil,
        libraryModelID: String? = nil
    ) {
        self.mode = mode
        self.chatSessionID = chatSessionID
        self.imagesSelection = imagesSelection
        self.voiceModelID = voiceModelID
        self.libraryModelID = libraryModelID
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let rawMode = try container.decodeIfPresent(String.self, forKey: .mode)
        mode = rawMode.flatMap(AppMode.init(rawValue:)) ?? .library
        chatSessionID = try container.decodeIfPresent(String.self, forKey: .chatSessionID)
        imagesSelection = try container.decodeIfPresent(String.self, forKey: .imagesSelection)
        voiceModelID = try container.decodeIfPresent(String.self, forKey: .voiceModelID)
        libraryModelID = try container.decodeIfPresent(String.self, forKey: .libraryModelID)
    }

    public func selection(in mode: AppMode) -> String? {
        switch mode {
        case .chat: chatSessionID
        case .images: imagesSelection
        case .voice: voiceModelID
        case .library: libraryModelID
        case .settings: nil
        }
    }

    public mutating func setSelection(_ id: String?, in mode: AppMode) {
        switch mode {
        case .chat: chatSessionID = id
        case .images: imagesSelection = id
        case .voice: voiceModelID = id
        case .library: libraryModelID = id
        case .settings: break
        }
    }
}
