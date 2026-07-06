import Foundation

public struct TurnRole: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let system = TurnRole(rawValue: "system")
    public static let user = TurnRole(rawValue: "user")
    public static let assistant = TurnRole(rawValue: "assistant")
}

public struct TurnDraft: Sendable, Hashable {
    public var role: TurnRole
    public var content: String
    public var thinking: String?
    public var modelID: String?
    public var statsJSON: String?
    public var artifactRefs: [String]

    public init(
        role: TurnRole,
        content: String,
        thinking: String? = nil,
        modelID: String? = nil,
        statsJSON: String? = nil,
        artifactRefs: [String] = []
    ) {
        self.role = role
        self.content = content
        self.thinking = thinking
        self.modelID = modelID
        self.statsJSON = statsJSON
        self.artifactRefs = artifactRefs
    }
}

public struct ChatTurn: Codable, Sendable, Hashable, Identifiable {
    public let id: String
    public let sessionID: String
    public let seq: Int
    public let role: TurnRole
    public var content: String
    public var thinking: String?
    public var modelID: String?
    public var statsJSON: String?
    public var artifactRefs: [String]
    public var supersededBy: String?
    public var contentHash: String
    public let createdAt: Date
    public var updatedAt: Date

    public init(
        id: String,
        sessionID: String,
        seq: Int,
        role: TurnRole,
        content: String,
        thinking: String? = nil,
        modelID: String? = nil,
        statsJSON: String? = nil,
        artifactRefs: [String] = [],
        supersededBy: String? = nil,
        contentHash: String,
        createdAt: Date,
        updatedAt: Date
    ) {
        self.id = id
        self.sessionID = sessionID
        self.seq = seq
        self.role = role
        self.content = content
        self.thinking = thinking
        self.modelID = modelID
        self.statsJSON = statsJSON
        self.artifactRefs = artifactRefs
        self.supersededBy = supersededBy
        self.contentHash = contentHash
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }
}
