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

    static func samplingMode(
        temperature: Double?, topP: Double?, topK: Int?, seed: UInt64?
    ) -> GenerationOptions.SamplingMode? {
        if let temperature, temperature <= 0 { return .greedy }
        if let topK { return .random(top: topK, seed: seed) }
        if let topP { return .random(probabilityThreshold: topP, seed: seed) }
        return nil
    }

    func stream(
        messages: [ChatMessage], temperature: Double?, topP: Double?, topK: Int?,
        seed: UInt64?, maxTokens: Int?, tools: [ToolSpec],
        resultProvider: BuiltinToolResultProvider?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let parts = Self.split(messages)
                    let bridged: [any FoundationModels.Tool] =
                        resultProvider.map { provider in
                            tools.compactMap {
                                BridgedFoundationTool(
                                    spec: $0, provider: provider,
                                    onCall: { call, result in
                                        continuation.yield(
                                            .toolCalled(call, result: result))
                                    })
                            }
                        } ?? []
                    let session = Self.session(
                        instructions: parts.instructions, history: parts.history,
                        tools: bridged)
                    let options = GenerationOptions(
                        sampling: Self.samplingMode(
                            temperature: temperature, topP: topP, topK: topK, seed: seed),
                        temperature: temperature, maximumResponseTokens: maxTokens)
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
        instructions: String?, history: [ChatMessage], tools: [any FoundationModels.Tool] = []
    ) -> LanguageModelSession {
        guard !history.isEmpty else {
            return LanguageModelSession(
                model: .default, tools: tools, instructions: instructions)
        }
        var entries: [Transcript.Entry] = []
        if let instructions {
            entries.append(
                .instructions(
                    Transcript.Instructions(
                        segments: [.text(Transcript.TextSegment(content: instructions))],
                        toolDefinitions: tools.map { Transcript.ToolDefinition(tool: $0) })))
        }
        for message in history {
            switch message.role {
            case .assistant where !message.toolCalls.isEmpty:
                entries.append(
                    .toolCalls(
                        Transcript.ToolCalls(
                            message.toolCalls.map { call in
                                Transcript.ToolCall(
                                    id: call.id,
                                    toolName: call.name,
                                    arguments: (try? GeneratedContent(
                                        json: call.arguments.jsonString))
                                        ?? GeneratedContent(call.arguments.jsonString))
                            })))
            case .tool:
                entries.append(
                    .toolOutput(
                        Transcript.ToolOutput(
                            id: message.toolCallID ?? UUID().uuidString.lowercased(),
                            toolName: message.toolName ?? "",
                            segments: [
                                .text(Transcript.TextSegment(content: message.content))
                            ])))
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
            model: .default, tools: tools, transcript: Transcript(entries: entries))
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

struct BridgedFoundationTool: FoundationModels.Tool {
    typealias Arguments = GeneratedContent
    typealias Output = String

    let name: String
    let description: String
    let parameters: GenerationSchema
    private let provider: BuiltinToolResultProvider
    private let onCall: @Sendable (ToolCall, String) -> Void

    init?(
        spec: ToolSpec, provider: @escaping BuiltinToolResultProvider,
        onCall: @escaping @Sendable (ToolCall, String) -> Void
    ) {
        guard let schema = Self.schema(from: spec) else { return nil }
        self.name = spec.name
        self.description = spec.description
        self.parameters = schema
        self.provider = provider
        self.onCall = onCall
    }

    func call(arguments: GeneratedContent) async throws -> String {
        let parsed =
            (try? JSONSerialization.jsonObject(
                with: Data(arguments.jsonString.utf8)) as? [String: Any])
            .flatMap { JSONValue.fromAny($0) } ?? .object([:])
        let call = ToolCall(name: name, arguments: parsed)
        let result = await provider(call)
        onCall(call, result)
        return result
    }

    static func schema(from spec: ToolSpec) -> GenerationSchema? {
        guard let root = dynamicSchema(named: spec.name, from: spec.parameters) else {
            return nil
        }
        return try? GenerationSchema(root: root, dependencies: [])
    }

    private static func dynamicSchema(
        named name: String, from schema: JSONValue
    ) -> DynamicGenerationSchema? {
        guard case .object(let fields) = schema else { return nil }
        if case .array(let options)? = fields["enum"] {
            let choices = options.compactMap(\.stringValue)
            guard choices.count == options.count else { return nil }
            return DynamicGenerationSchema(name: name, anyOf: choices)
        }
        switch fields["type"]?.stringValue ?? "object" {
        case "string":
            return DynamicGenerationSchema(type: String.self)
        case "number":
            return DynamicGenerationSchema(type: Double.self)
        case "integer":
            return DynamicGenerationSchema(type: Int.self)
        case "boolean":
            return DynamicGenerationSchema(type: Bool.self)
        case "array":
            guard let items = fields["items"],
                let item = dynamicSchema(named: name + "Item", from: items)
            else { return nil }
            return DynamicGenerationSchema(arrayOf: item)
        case "object":
            guard case .object(let properties)? = fields["properties"] ?? .object([:])
            else { return nil }
            var required: Set<String> = []
            if case .array(let names)? = fields["required"] {
                required = Set(names.compactMap(\.stringValue))
            }
            var built: [DynamicGenerationSchema.Property] = []
            for key in properties.keys.sorted() {
                guard let value = dynamicSchema(named: name + "-" + key, from: properties[key]!)
                else { return nil }
                built.append(
                    DynamicGenerationSchema.Property(
                        name: key, schema: value, isOptional: !required.contains(key)))
            }
            return DynamicGenerationSchema(name: name, properties: built)
        default:
            return nil
        }
    }
}
