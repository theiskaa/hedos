import Foundation

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
        deletedAt: Date? = nil
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
    }
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
