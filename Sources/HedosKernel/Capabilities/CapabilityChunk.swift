import Foundation

public enum CapabilityChunk: Sendable, Hashable {
    case text(String)
    case thinking(String)
    case segment(String, startMs: Int, endMs: Int)
    case audio(AudioFrame)
    case vector([Double])
    case toolCall(ToolCall)
    case status(String)
    case done(GenerationStats?)
}

public struct AudioFrame: Sendable, Hashable {
    public var data: Data
    public var sampleRate: Int

    public init(data: Data, sampleRate: Int) {
        self.data = data
        self.sampleRate = sampleRate
    }
}

public struct GenerationStats: Codable, Sendable, Hashable {
    public var promptTokens: Int?
    public var completionTokens: Int?
    public var durationMs: Int?
    public var ttftMs: Int?
    public var loadMs: Int?
    public var finishReason: String?
    public var tokenCountsEstimated: Bool

    public init(
        promptTokens: Int? = nil, completionTokens: Int? = nil, durationMs: Int? = nil,
        ttftMs: Int? = nil, loadMs: Int? = nil, finishReason: String? = nil,
        tokenCountsEstimated: Bool = false
    ) {
        self.promptTokens = promptTokens
        self.completionTokens = completionTokens
        self.durationMs = durationMs
        self.ttftMs = ttftMs
        self.loadMs = loadMs
        self.finishReason = finishReason
        self.tokenCountsEstimated = tokenCountsEstimated
    }

    enum CodingKeys: String, CodingKey {
        case promptTokens, completionTokens, durationMs, ttftMs, loadMs, finishReason
        case tokenCountsEstimated
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        promptTokens = try container.decodeIfPresent(Int.self, forKey: .promptTokens)
        completionTokens = try container.decodeIfPresent(Int.self, forKey: .completionTokens)
        durationMs = try container.decodeIfPresent(Int.self, forKey: .durationMs)
        ttftMs = try container.decodeIfPresent(Int.self, forKey: .ttftMs)
        loadMs = try container.decodeIfPresent(Int.self, forKey: .loadMs)
        finishReason = try container.decodeIfPresent(String.self, forKey: .finishReason)
        tokenCountsEstimated =
            try container.decodeIfPresent(Bool.self, forKey: .tokenCountsEstimated) ?? false
    }
}

public struct ChatMessage: Codable, Sendable, Hashable {
    public enum Role: String, Codable, Sendable {
        case system, user, assistant, tool
    }

    public var role: Role
    public var content: String
    public var toolCalls: [ToolCall]
    public var toolCallID: String?
    public var toolName: String?

    public init(
        role: Role, content: String, toolCalls: [ToolCall] = [],
        toolCallID: String? = nil, toolName: String? = nil
    ) {
        self.role = role
        self.content = content
        self.toolCalls = toolCalls
        self.toolCallID = toolCallID
        self.toolName = toolName
    }

    enum CodingKeys: String, CodingKey {
        case role, content, toolCalls, toolCallID, toolName
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        role = try container.decode(Role.self, forKey: .role)
        content = try container.decode(String.self, forKey: .content)
        toolCalls = try container.decodeIfPresent([ToolCall].self, forKey: .toolCalls) ?? []
        toolCallID = try container.decodeIfPresent(String.self, forKey: .toolCallID)
        toolName = try container.decodeIfPresent(String.self, forKey: .toolName)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(role, forKey: .role)
        try container.encode(content, forKey: .content)
        if !toolCalls.isEmpty { try container.encode(toolCalls, forKey: .toolCalls) }
        try container.encodeIfPresent(toolCallID, forKey: .toolCallID)
        try container.encodeIfPresent(toolName, forKey: .toolName)
    }
}
