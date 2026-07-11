import Foundation

enum BuiltinAvailability: Sendable, Hashable {
    case available
    case notEnabled
    case notReady
    case notEligible
}

enum BuiltinGenerationEvent: Sendable, Hashable {
    case snapshot(String)
    case toolCalled(ToolCall, result: String)
    case done(promptTokens: Int?, completionTokens: Int?)
}

typealias BuiltinToolResultProvider = @Sendable (ToolCall) async -> String

protocol AppleFoundationBackend: Sendable {
    func availability() -> BuiltinAvailability
    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?,
        tools: [ToolSpec], resultProvider: BuiltinToolResultProvider?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error>
}

extension AppleFoundationBackend {
    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error> {
        stream(
            messages: messages, temperature: temperature, maxTokens: maxTokens,
            tools: [], resultProvider: nil)
    }
}
