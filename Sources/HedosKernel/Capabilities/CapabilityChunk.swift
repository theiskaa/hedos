import Foundation

public enum CapabilityChunk: Sendable, Hashable {
    case text(String)
    case thinking(String)
    case audio(AudioFrame)
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

    public init(
        promptTokens: Int? = nil, completionTokens: Int? = nil, durationMs: Int? = nil,
        ttftMs: Int? = nil
    ) {
        self.promptTokens = promptTokens
        self.completionTokens = completionTokens
        self.durationMs = durationMs
        self.ttftMs = ttftMs
    }
}

public struct ChatMessage: Codable, Sendable, Hashable {
    public enum Role: String, Codable, Sendable {
        case system, user, assistant
    }

    public var role: Role
    public var content: String

    public init(role: Role, content: String) {
        self.role = role
        self.content = content
    }
}
