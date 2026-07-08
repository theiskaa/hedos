import Foundation

public enum BuiltinAvailability: Sendable, Hashable {
    case available
    case notEnabled
    case notReady
    case notEligible
}

public enum BuiltinGenerationEvent: Sendable, Hashable {
    case snapshot(String)
    case done(promptTokens: Int?, completionTokens: Int?)
}

public protocol AppleFoundationBackend: Sendable {
    func availability() -> BuiltinAvailability
    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error>
}
