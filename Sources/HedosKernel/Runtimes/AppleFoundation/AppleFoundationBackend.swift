import Foundation

enum BuiltinAvailability: Sendable, Hashable {
    case available
    case notEnabled
    case notReady
    case notEligible
}

enum BuiltinGenerationEvent: Sendable, Hashable {
    case snapshot(String)
    case done(promptTokens: Int?, completionTokens: Int?)
}

protocol AppleFoundationBackend: Sendable {
    func availability() -> BuiltinAvailability
    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error>
}
