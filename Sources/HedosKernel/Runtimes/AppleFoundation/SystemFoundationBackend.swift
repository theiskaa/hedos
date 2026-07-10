import Foundation
import FoundationModels

struct SystemFoundationBackend: AppleFoundationBackend {
    init() {}

    func availability() -> BuiltinAvailability {
        switch SystemLanguageModel.default.availability {
        case .available:
            return .available
        case .unavailable(let reason):
            switch reason {
            case .appleIntelligenceNotEnabled:
                return .notEnabled
            case .modelNotReady:
                return .notReady
            case .deviceNotEligible:
                return .notEligible
            @unknown default:
                return .notReady
            }
        }
    }

    static func split(_ messages: [ChatMessage]) -> (
        instructions: String?, history: [ChatMessage], prompt: String
    ) {
        var remaining = messages
        var instructions: [String] = []
        while let first = remaining.first, first.role == .system {
            instructions.append(first.content)
            remaining.removeFirst()
        }
        let prompt = remaining.last?.content ?? ""
        let history = remaining.isEmpty ? [] : Array(remaining.dropLast())
        return (
            instructions.isEmpty ? nil : instructions.joined(separator: "\n\n"),
            history,
            prompt
        )
    }

    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let parts = Self.split(messages)
                    let session = Self.session(
                        instructions: parts.instructions, history: parts.history)
                    let options = GenerationOptions(
                        sampling: nil, temperature: temperature, maximumResponseTokens: maxTokens)
                    var finalText = ""
                    for try await snapshot in session.streamResponse(
                        to: parts.prompt, options: options)
                    {
                        try Task.checkCancellation()
                        finalText = snapshot.content
                        continuation.yield(.snapshot(snapshot.content))
                    }
                    let promptText = messages.map(\.content).joined(separator: "\n")
                    continuation.yield(
                        .done(
                            promptTokens: await Self.tokenCount(promptText),
                            completionTokens: await Self.tokenCount(finalText)))
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch let error as LanguageModelSession.GenerationError {
                    continuation.finish(throwing: Self.mapped(error))
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func session(
        instructions: String?, history: [ChatMessage]
    ) -> LanguageModelSession {
        guard !history.isEmpty else {
            return LanguageModelSession(model: .default, tools: [], instructions: instructions)
        }
        var entries: [Transcript.Entry] = []
        if let instructions {
            entries.append(
                .instructions(
                    Transcript.Instructions(
                        segments: [.text(Transcript.TextSegment(content: instructions))],
                        toolDefinitions: [])))
        }
        for message in history {
            switch message.role {
            case .assistant:
                entries.append(
                    .response(
                        Transcript.Response(
                            assetIDs: [],
                            segments: [.text(Transcript.TextSegment(content: message.content))])))
            default:
                entries.append(
                    .prompt(
                        Transcript.Prompt(
                            segments: [.text(Transcript.TextSegment(content: message.content))])))
            }
        }
        return LanguageModelSession(
            model: .default, tools: [], transcript: Transcript(entries: entries))
    }

    private static func tokenCount(_ text: String) async -> Int? {
        guard #available(macOS 26.4, *) else { return nil }
        return try? await SystemLanguageModel.default.tokenCount(for: text)
    }

    private static func mapped(_ error: LanguageModelSession.GenerationError) -> KernelError {
        switch error {
        case .guardrailViolation, .refusal:
            return .runtimeFailed("Apple's model declined this request.")
        case .exceededContextWindowSize:
            return .contextExceeded(model: "Apple Intelligence")
        case .rateLimited:
            return .runtimeFailed("Apple's model is rate limiting requests — try again in a moment.")
        case .concurrentRequests:
            return .runtimeFailed("Apple's model is busy with another request.")
        case .assetsUnavailable:
            return .runtimeFailed(
                "Apple's model isn't available right now — check Apple Intelligence in System Settings.")
        default:
            return .runtimeFailed("Apple's model hit an error: \(error.localizedDescription)")
        }
    }
}
