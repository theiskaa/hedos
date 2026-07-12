
public struct Modality: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let unknown = Modality(rawValue: "unknown")
    public static let text = Modality(rawValue: "text")
    public static let image = Modality(rawValue: "image")
    public static let speech = Modality(rawValue: "speech")
    public static let audio = Modality(rawValue: "audio")
    public static let video = Modality(rawValue: "video")
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
    public static let builtin = SourceKind(rawValue: "builtin")
    public static let endpoint = SourceKind(rawValue: "endpoint")
    public static let file = SourceKind(rawValue: "file")
    public static let folder = SourceKind(rawValue: "folder")
}

public struct RuntimeID: RawRepresentable, Codable, Hashable, Sendable, ExpressibleByStringLiteral,
    CustomStringConvertible
{
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }
    public var description: String { rawValue }

    public static let llamaCpp = RuntimeID(rawValue: "llama-cpp")
    public static let whisperCpp = RuntimeID(rawValue: "whisper-cpp")
    public static let ollama = RuntimeID(rawValue: "ollama")
    public static let mlxSwift = RuntimeID(rawValue: "mlx-swift")
    public static let appleFoundation = RuntimeID(rawValue: "apple-foundation")
    public static let openAIEndpoint = RuntimeID(rawValue: "generic:openai-server")
    public static let mflux = RuntimeID(rawValue: "python:mflux")
    public static let diffusers = RuntimeID(rawValue: "python:diffusers")
    public static let mlxLm = RuntimeID(rawValue: "python:mlx-lm")
    public static let mlxAudio = RuntimeID(rawValue: "python:mlx-audio")
    public static let mlxVlm = RuntimeID(rawValue: "python:mlx-vlm")
    public static let embeddings = RuntimeID(rawValue: "python:embeddings")
    public static let comfyUI = RuntimeID(rawValue: "comfyui")
    public static let a1111 = RuntimeID(rawValue: "a1111")
}

public enum BidPreference {
    public static let llamaCpp = 10
    public static let whisperCpp = 10
    public static let endpoint = 10
    public static let mlxVlm = 14
    public static let mlxSwift = 15
    public static let appleFoundation = 15
    public static let ollama = 20
    public static let mflux = 25
    public static let diffusers = 26
    public static let comfyUI = 27
    public static let a1111 = 28
    public static let mlxAudio = 30
    public static let embeddings = 32
    public static let mlxLm = 40
    public static let manifest = 100
}

public enum ExecutionMode: String, Codable, Hashable, Sendable {
    case stream
    case job
    case sync
}

public enum RunTier: String, Codable, Hashable, Sendable {
    case native
    case managed
    case remote
    case recipeNeeded = "recipe-needed"
}

public enum ModelState: String, Codable, Hashable, Sendable {
    case ready
    case unresolved
    case missing
}
