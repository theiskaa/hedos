/// Open, string-backed identifiers. Modalities, capabilities, and source
/// kinds are extensible sets: new ones arrive with runtime manifests and
/// scanners, not kernel releases. Only genuinely fixed semantics (execution
/// mode, run tier, model state) are closed enums.

public struct Modality: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let unknown = Modality(rawValue: "unknown")
    public static let text = Modality(rawValue: "text")
    public static let image = Modality(rawValue: "image")
    public static let speech = Modality(rawValue: "speech")
    public static let audio = Modality(rawValue: "audio")
    public static let vision = Modality(rawValue: "vision")
    public static let embedding = Modality(rawValue: "embedding")
}

public struct Capability: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let chat = Capability(rawValue: "chat")
    public static let complete = Capability(rawValue: "complete")
    public static let embed = Capability(rawValue: "embed")
    public static let see = Capability(rawValue: "see")
    public static let image = Capability(rawValue: "image")
    public static let speak = Capability(rawValue: "speak")
    public static let transcribe = Capability(rawValue: "transcribe")
}

public struct SourceKind: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let ollama = SourceKind(rawValue: "ollama")
    public static let huggingfaceCache = SourceKind(rawValue: "huggingface-cache")
    public static let lmStudio = SourceKind(rawValue: "lm-studio")
    public static let file = SourceKind(rawValue: "file")
    public static let folder = SourceKind(rawValue: "folder")
}

/// How a capability is exercised: interactive stream, long-running job with
/// progress and artifacts, or plain request/response.
public enum ExecutionMode: String, Codable, Hashable, Sendable {
    case stream
    case job
    case sync
}

/// The honest badge on the shelf: how this model will run, surfaced before
/// the user clicks, never after.
public enum RunTier: String, Codable, Hashable, Sendable {
    case native
    case managed
    case recipeNeeded = "recipe-needed"
}

public enum ModelState: String, Codable, Hashable, Sendable {
    case ready
    case unresolved
    case missing
}
