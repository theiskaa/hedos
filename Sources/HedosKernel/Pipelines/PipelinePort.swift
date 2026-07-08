import Foundation

public enum PipelinePort: String, Codable, Sendable, Hashable {
    case audio
    case text
    case image
    case vector
}

public struct CapabilitySignature: Sendable, Hashable {
    public let input: PipelinePort
    public let output: PipelinePort
    public let mode: ExecutionMode

    public init(input: PipelinePort, output: PipelinePort, mode: ExecutionMode) {
        self.input = input
        self.output = output
        self.mode = mode
    }
}

public enum CapabilitySignatures {
    public static let table: [Capability: CapabilitySignature] = [
        .transcribe: CapabilitySignature(input: .audio, output: .text, mode: .stream),
        .chat: CapabilitySignature(input: .text, output: .text, mode: .stream),
        .complete: CapabilitySignature(input: .text, output: .text, mode: .stream),
        .speak: CapabilitySignature(input: .text, output: .audio, mode: .stream),
        .image: CapabilitySignature(input: .text, output: .image, mode: .job),
        .embed: CapabilitySignature(input: .text, output: .vector, mode: .sync),
    ]

    public static func signature(_ capability: Capability) -> CapabilitySignature? {
        table[capability]
    }

    public static var composable: [Capability] {
        table.keys.sorted { $0.rawValue < $1.rawValue }
    }
}
