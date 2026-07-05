public enum CapabilityChunk: Sendable, Hashable {
    case text(String)
    case thinking(String)
    case done(GenerationStats?)
}

public struct GenerationStats: Sendable, Hashable {
    public var promptTokens: Int?
    public var completionTokens: Int?
    public var durationMs: Int?

    public init(
        promptTokens: Int? = nil, completionTokens: Int? = nil, durationMs: Int? = nil
    ) {
        self.promptTokens = promptTokens
        self.completionTokens = completionTokens
        self.durationMs = durationMs
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
