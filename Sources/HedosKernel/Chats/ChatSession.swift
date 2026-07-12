import Foundation

public enum ChatIntent: String, Codable, Sendable, Hashable {
    case text
    case image
    case speak
}

public struct ChatSession: Codable, Sendable, Hashable, Identifiable {
    public let id: String
    public var title: String
    public let createdAt: Date
    public var updatedAt: Date
    public var modelID: String?
    public var capabilityTags: [String]
    public var turnCount: Int
    public var pinned: Bool
    public var archived: Bool
    public var deletedAt: Date?
    public var place: String?
    public var systemPrompt: String?
    public var titledBy: String?
    public var intent: ChatIntent
    public var imageModelID: String?
    public var voiceModelID: String?

    public init(
        id: String,
        title: String,
        createdAt: Date,
        updatedAt: Date,
        modelID: String? = nil,
        capabilityTags: [String] = [],
        turnCount: Int = 0,
        pinned: Bool = false,
        archived: Bool = false,
        deletedAt: Date? = nil,
        place: String? = nil,
        systemPrompt: String? = nil,
        titledBy: String? = nil,
        intent: ChatIntent = .text,
        imageModelID: String? = nil,
        voiceModelID: String? = nil
    ) {
        self.id = id
        self.title = title
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.modelID = modelID
        self.capabilityTags = capabilityTags
        self.turnCount = turnCount
        self.pinned = pinned
        self.archived = archived
        self.deletedAt = deletedAt
        self.place = place
        self.systemPrompt = systemPrompt
        self.titledBy = titledBy
        self.intent = intent
        self.imageModelID = imageModelID
        self.voiceModelID = voiceModelID
    }

    enum CodingKeys: String, CodingKey {
        case id, title, createdAt, updatedAt, modelID, capabilityTags, turnCount
        case pinned, archived, deletedAt, place, systemPrompt, titledBy
        case intent, imageModelID, voiceModelID
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decode(String.self, forKey: .title)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        updatedAt = try container.decode(Date.self, forKey: .updatedAt)
        modelID = try container.decodeIfPresent(String.self, forKey: .modelID)
        capabilityTags = try container.decodeIfPresent([String].self, forKey: .capabilityTags) ?? []
        turnCount = try container.decodeIfPresent(Int.self, forKey: .turnCount) ?? 0
        pinned = try container.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
        archived = try container.decodeIfPresent(Bool.self, forKey: .archived) ?? false
        deletedAt = try container.decodeIfPresent(Date.self, forKey: .deletedAt)
        place = try container.decodeIfPresent(String.self, forKey: .place)
        systemPrompt = try container.decodeIfPresent(String.self, forKey: .systemPrompt)
        titledBy = try container.decodeIfPresent(String.self, forKey: .titledBy)
        intent = try container.decodeIfPresent(ChatIntent.self, forKey: .intent) ?? .text
        imageModelID = try container.decodeIfPresent(String.self, forKey: .imageModelID)
        voiceModelID = try container.decodeIfPresent(String.self, forKey: .voiceModelID)
    }

    public static let defaultTitle = "New Chat"

    public static func title(from content: String, limit: Int = 60) -> String {
        let firstLine =
            content.split(whereSeparator: \.isNewline).first.map(String.init) ?? content
        let trimmed = firstLine.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return defaultTitle }
        guard trimmed.count > limit else { return trimmed }
        return trimmed.prefix(limit).trimmingCharacters(in: .whitespaces) + "…"
    }
}

public enum SessionTag {
    public static let thinking = "thinking"
    public static let spoke = "spoke"
    public static let generatedImage = "generated-image"
}

public enum ChatSessionFilter: String, Codable, Sendable {
    case active
    case archived
    case all
}

public struct ChatTranscript: Codable, Sendable, Hashable {
    public let session: ChatSession
    public let turns: [ChatTurn]

    public init(session: ChatSession, turns: [ChatTurn]) {
        self.session = session
        self.turns = turns
    }

    public var attributionNeeded: Bool {
        let modelIDs = Set(
            turns
                .filter { $0.supersededBy == nil && $0.role == .assistant }
                .compactMap(\.modelID))
        if modelIDs.count > 1 { return true }
        if let bound = session.modelID, let only = modelIDs.first, only != bound { return true }
        return false
    }
}

public struct SearchHit: Codable, Sendable, Hashable {
    public let sessionID: String
    public let turnID: String
    public let sessionTitle: String
    public let snippet: String
    public let rank: Double

    public init(
        sessionID: String, turnID: String, sessionTitle: String, snippet: String, rank: Double
    ) {
        self.sessionID = sessionID
        self.turnID = turnID
        self.sessionTitle = sessionTitle
        self.snippet = snippet
        self.rank = rank
    }
}
