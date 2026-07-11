import Foundation

public struct TurnRole: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let system = TurnRole(rawValue: "system")
    public static let user = TurnRole(rawValue: "user")
    public static let assistant = TurnRole(rawValue: "assistant")
    public static let tool = TurnRole(rawValue: "tool")

    public var messageRole: ChatMessage.Role? {
        ChatMessage.Role(rawValue: rawValue)
    }
}

public struct TurnDraft: Sendable, Hashable {
    public var role: TurnRole
    public var content: String
    public var thinking: String?
    public var modelID: String?
    var statsJSON: String?
    public var artifactRefs: [String]
    public var toolCallsJSON: String?
    public var toolCallID: String?
    public var toolName: String?

    public init(
        role: TurnRole,
        content: String,
        thinking: String? = nil,
        modelID: String? = nil,
        statsJSON: String? = nil,
        artifactRefs: [String] = [],
        toolCallsJSON: String? = nil,
        toolCallID: String? = nil,
        toolName: String? = nil
    ) {
        self.role = role
        self.content = content
        self.thinking = thinking
        self.modelID = modelID
        self.statsJSON = statsJSON
        self.artifactRefs = artifactRefs
        self.toolCallsJSON = toolCallsJSON
        self.toolCallID = toolCallID
        self.toolName = toolName
    }

    public init(
        role: TurnRole,
        content: String,
        thinking: String? = nil,
        modelID: String? = nil,
        stats: GenerationStats?,
        artifactRefs: [String] = [],
        toolCalls: [ToolCall] = [],
        toolCallID: String? = nil,
        toolName: String? = nil
    ) {
        self.init(
            role: role,
            content: content,
            thinking: thinking,
            modelID: modelID,
            statsJSON: stats?.turnStatsJSON,
            artifactRefs: artifactRefs,
            toolCallsJSON: toolCalls.turnToolCallsJSON,
            toolCallID: toolCallID,
            toolName: toolName)
    }
}

extension [ToolCall] {
    public var turnToolCallsJSON: String? {
        guard !isEmpty else { return nil }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        guard let data = try? encoder.encode(self) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public static func fromTurnToolCallsJSON(_ json: String?) -> [ToolCall] {
        guard let json, let data = json.data(using: .utf8) else { return [] }
        return (try? JSONDecoder().decode([ToolCall].self, from: data)) ?? []
    }
}

extension GenerationStats {
    public var turnStatsJSON: String? {
        guard let data = try? JSONEncoder().encode(self) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public static func fromTurnStatsJSON(_ json: String?) -> GenerationStats? {
        guard let json, let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(GenerationStats.self, from: data)
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
    var statsJSON: String?
    public var artifactRefs: [String]
    public var supersededBy: String?
    public var contentHash: String
    public let createdAt: Date
    public var updatedAt: Date
    public var toolCallsJSON: String?
    public var toolCallID: String?
    public var toolName: String?
    public var interrupted: Bool

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
        updatedAt: Date,
        toolCallsJSON: String? = nil,
        toolCallID: String? = nil,
        toolName: String? = nil,
        interrupted: Bool = false
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
        self.toolCallsJSON = toolCallsJSON
        self.toolCallID = toolCallID
        self.toolName = toolName
        self.interrupted = interrupted
    }

    enum CodingKeys: String, CodingKey {
        case id, sessionID, seq, role, content, thinking, modelID, statsJSON, artifactRefs
        case supersededBy, contentHash, createdAt, updatedAt, toolCallsJSON, toolCallID, toolName
        case interrupted
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        sessionID = try container.decode(String.self, forKey: .sessionID)
        seq = try container.decode(Int.self, forKey: .seq)
        role = try container.decode(TurnRole.self, forKey: .role)
        content = try container.decode(String.self, forKey: .content)
        thinking = try container.decodeIfPresent(String.self, forKey: .thinking)
        modelID = try container.decodeIfPresent(String.self, forKey: .modelID)
        statsJSON = try container.decodeIfPresent(String.self, forKey: .statsJSON)
        artifactRefs = try container.decodeIfPresent([String].self, forKey: .artifactRefs) ?? []
        supersededBy = try container.decodeIfPresent(String.self, forKey: .supersededBy)
        contentHash = try container.decode(String.self, forKey: .contentHash)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        updatedAt = try container.decode(Date.self, forKey: .updatedAt)
        toolCallsJSON = try container.decodeIfPresent(String.self, forKey: .toolCallsJSON)
        toolCallID = try container.decodeIfPresent(String.self, forKey: .toolCallID)
        toolName = try container.decodeIfPresent(String.self, forKey: .toolName)
        interrupted = try container.decodeIfPresent(Bool.self, forKey: .interrupted) ?? false
    }

    public var stats: GenerationStats? {
        GenerationStats.fromTurnStatsJSON(statsJSON)
    }

    public var toolCalls: [ToolCall] {
        [ToolCall].fromTurnToolCallsJSON(toolCallsJSON)
    }

    public var isGeneratedArtifact: Bool {
        role == .assistant && content.isEmpty && !artifactRefs.isEmpty
    }
}
