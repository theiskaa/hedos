// The Swift shim bridging Apple's FoundationModels framework to the Rust
// runtime over a flat C ABI, ported from the Swift kernel's
// SystemFoundationBackend (macos-app branch, f85874f). Compiled by the
// crate's build script on every macOS build with a capable SDK into
// `libhedos_apple_shim.dylib`; loaded and driven by
// `runtime/src/adapters/apple_foundation/ffi.rs`.
//
// The ABI:
// - `hedos_af_abi_version() -> u32` — this contract's version; currently 2.
// - `hedos_af_availability() -> i32` — 0 available, 1 Apple Intelligence not
//   enabled, 2 model not ready (still downloading), 3 device not eligible.
// - `hedos_af_stream(request_json, ctx, callback) -> u64` — starts a
//   generation and returns its handle (never 0). The request is
//   `{"messages":[…],"tools":[…],"temperature","top_p","top_k","seed",
//   "max_tokens"}` with every option, and `tools`, omittable. A message is
//   `{"role","content"}` plus, on an assistant turn, `"tool_calls":
//   [{"id","name","arguments"}]` and, on a tool turn, `"tool_call_id"` and
//   `"tool_name"`. A tool is `{"name","description","parameters"}` where
//   `parameters` is a JSON-Schema object; a tool whose schema the framework
//   cannot express is dropped from the offer, never failing the request.
//   Returns 0 without starting a generation when the callback is null (no
//   event fires) or the request fails to decode (one `error` event fires
//   synchronously on the calling thread, before the return — the only
//   re-entrant emit; every other event arrives later, from the generation
//   task).
// - `hedos_af_cancel(handle)` — requests cooperative cancellation; a
//   terminal event still follows, though not necessarily `cancelled` (a
//   generation already past its stream loop finishes as `done` or `error`).
//
// Events arrive through the callback as `(ctx, kind, payload)` on an
// arbitrary thread, the payload pointer valid only during the call (see
// `Event` for the kinds). Every generation ends with exactly one terminal
// event — done, error, or cancelled — after which the shim never touches
// `ctx` again, so the caller frees it on the terminal event.
//
// Tool calling is capture-and-replay: when the model invokes an offered
// tool, the shim emits its call as a `tool_call` event and finishes the
// generation — the terminal event after a capture is always `done`, never
// `cancelled` or `error`, because the caller executes the tool and replays
// the result in a later request. That replay arrives as a conversation
// ending in a tool turn; the shim rebuilds it as transcript history and the
// model continues from the tool output.

import Foundation
import FoundationModels

public typealias HedosEventCallback = @convention(c) (
    UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?
) -> Void

// The callback event kinds. `snapshot` carries the full reply text so far
// (cumulative, not a delta); `toolCall` a `{"name","arguments":{…}}` object
// where `arguments` is an object or omitted, never null; `done` carries
// `{"prompt_tokens":n|null,"completion_tokens":n|null}`; `error` a
// human-readable message; `cancelled` an empty payload. `snapshot` and
// `toolCall` are the non-terminal kinds.
private enum Event: Int32 {
    case snapshot = 0
    case done = 1
    case error = 2
    case cancelled = 3
    case toolCall = 4

    var isTerminal: Bool {
        self == .done || self == .error || self == .cancelled
    }
}

private struct ShimError: Error {
    let text: String
}

private struct CallbackContext: @unchecked Sendable {
    let raw: UnsafeMutableRawPointer?
    let callback: HedosEventCallback

    func emit(_ kind: Event, _ payload: String) {
        payload.withCString { pointer in
            callback(raw, kind.rawValue, pointer)
        }
    }
}

// Running generations by handle. A task removes itself on completion; a handle
// whose task finished before `store` ran is tombstoned so nothing lingers.
private final class GenerationTable: @unchecked Sendable {
    static let shared = GenerationTable()
    private let lock = NSLock()
    private var tasks: [UInt64: Task<Void, Never>] = [:]
    private var finished: Set<UInt64> = []
    private var nextHandle: UInt64 = 0

    func reserve() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        nextHandle += 1
        return nextHandle
    }

    func store(_ handle: UInt64, _ task: Task<Void, Never>) {
        lock.lock()
        defer { lock.unlock() }
        if finished.remove(handle) == nil {
            tasks[handle] = task
        }
    }

    func finish(_ handle: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        if tasks.removeValue(forKey: handle) == nil {
            finished.insert(handle)
        }
    }

    func cancel(_ handle: UInt64) {
        lock.lock()
        let task = tasks[handle]
        lock.unlock()
        task?.cancel()
    }
}

// Arbitrary JSON as it appears in tool parameters and call arguments, which
// the typed wire structs cannot pin down.
private indirect enum WireValue: Decodable {
    case null
    case bool(Bool)
    case integer(Int64)
    case number(Double)
    case string(String)
    case array([WireValue])
    case object([String: WireValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let flag = try? container.decode(Bool.self) {
            self = .bool(flag)
        } else if let integer = try? container.decode(Int64.self) {
            // Tried before Double so a full-range i64 argument (an id, say)
            // survives the replay without rounding through a double.
            self = .integer(integer)
        } else if let number = try? container.decode(Double.self) {
            self = .number(number)
        } else if let text = try? container.decode(String.self) {
            self = .string(text)
        } else if let entries = try? container.decode([WireValue].self) {
            self = .array(entries)
        } else if let fields = try? container.decode([String: WireValue].self) {
            self = .object(fields)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container, debugDescription: "unsupported JSON value")
        }
    }

    var stringValue: String? {
        if case .string(let text) = self { return text }
        return nil
    }

    var anyValue: Any {
        switch self {
        case .null: return NSNull()
        case .bool(let flag): return flag
        case .integer(let integer): return NSNumber(value: integer)
        case .number(let number): return number
        case .string(let text): return text
        case .array(let entries): return entries.map(\.anyValue)
        case .object(let fields): return fields.mapValues(\.anyValue)
        }
    }

    var jsonString: String {
        let data = try? JSONSerialization.data(
            withJSONObject: anyValue, options: [.fragmentsAllowed, .sortedKeys])
        return data.flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }
}

private struct WireToolCall: Decodable {
    let id: String?
    let name: String
    let arguments: WireValue?
}

private struct WireTool: Decodable {
    let name: String
    let description: String?
    let parameters: WireValue?
}

private struct WireMessage: Decodable {
    let role: String
    let content: String
    let tool_calls: [WireToolCall]?
    let tool_call_id: String?
    let tool_name: String?
}

private struct WireRequest: Decodable {
    let messages: [WireMessage]
    let tools: [WireTool]?
    let temperature: Double?
    let top_p: Double?
    let top_k: Int?
    let seed: UInt64?
    let max_tokens: Int?
}

private struct PromptParts {
    let instructions: String?
    let history: [WireMessage]
    let prompt: String
}

// System turns merge into the session instructions; a trailing user turn is
// the prompt. A conversation that ends some other way — a tool result being
// replayed, an assistant turn to continue — stays wholly in the transcript,
// in order, and the prompt is empty so the model continues from where the
// history leaves off.
private func split(_ messages: [WireMessage]) throws -> PromptParts {
    var instructions: [String] = []
    var conversation: [WireMessage] = []
    for message in messages {
        if message.role == "system" {
            instructions.append(message.content)
        } else {
            conversation.append(message)
        }
    }
    guard conversation.contains(where: { $0.role == "user" }) else {
        throw ShimError(text: "Apple Intelligence needs a user message to answer")
    }
    var prompt = ""
    if let last = conversation.last, last.role == "user" {
        prompt = last.content
        conversation.removeLast()
    }
    return PromptParts(
        instructions: instructions.isEmpty ? nil : instructions.joined(separator: "\n\n"),
        history: conversation,
        prompt: prompt
    )
}

private func samplingMode(_ request: WireRequest) -> GenerationOptions.SamplingMode? {
    if let temperature = request.temperature, temperature <= 0 { return .greedy }
    if let topK = request.top_k { return .random(top: topK, seed: request.seed) }
    if let topP = request.top_p { return .random(probabilityThreshold: topP, seed: request.seed) }
    return nil
}

// The event channel for one generation: every emit passes through one lock,
// and nothing emits after the terminal event — the Rust side frees the
// callback context on the terminal, so a late emit (a parallel tool call
// still in flight when the generation unwound, say) would touch freed
// memory. The callback is invoked under the lock and must therefore never
// block or re-enter the shim; the Rust callback is a non-blocking channel
// send.
private final class GenerationEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var closed = false
    private var captured = false
    private let ctx: CallbackContext
    private let handle: UInt64

    init(ctx: CallbackContext, handle: UInt64) {
        self.ctx = ctx
        self.handle = handle
    }

    var hasCaptured: Bool {
        lock.lock()
        defer { lock.unlock() }
        return captured
    }

    func emit(_ kind: Event, _ payload: String) {
        lock.lock()
        defer { lock.unlock() }
        guard !closed else { return }
        if kind.isTerminal {
            closed = true
        }
        ctx.emit(kind, payload)
    }

    // Capturing emits the call and then cancels the generation — the result
    // the framework is waiting for arrives in a later request, not this
    // turn. A capture that lost the race against a terminal event (a user
    // cancel, usually) emits nothing.
    func captureToolCall(name: String, argumentsJSON: String) {
        var object: [String: Any] = ["name": name]
        if let parsed = try? JSONSerialization.jsonObject(with: Data(argumentsJSON.utf8)),
            parsed is [String: Any]
        {
            object["arguments"] = parsed
        }
        // The fallback is unreachable for a valid Swift string; a nameless
        // payload is dropped upstream, degrading to a plain finished turn.
        let payload =
            (try? JSONSerialization.data(withJSONObject: object))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        lock.lock()
        if !closed {
            captured = true
            ctx.emit(.toolCall, payload)
        }
        lock.unlock()
        GenerationTable.shared.cancel(handle)
    }
}

// A JSON-Schema object as a schema the framework can constrain generation
// with; nil for any shape it cannot express (that tool is then dropped from
// the offer).
private func dynamicSchema(named name: String, from schema: WireValue) -> DynamicGenerationSchema? {
    guard case .object(let fields) = schema else { return nil }
    if case .array(let options)? = fields["enum"] {
        let choices = options.compactMap(\.stringValue)
        guard choices.count == options.count, !choices.isEmpty else { return nil }
        return DynamicGenerationSchema(name: name, anyOf: choices)
    }
    // A present but non-string "type" (the nullable ["string","null"] form,
    // say) is a shape the framework can't express — distinct from an absent
    // type, which conventionally means an object.
    let typeName: String
    switch fields["type"] {
    case nil: typeName = "object"
    case .string(let text)?: typeName = text
    default: return nil
    }
    switch typeName {
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
        let propertiesValue = fields["properties"] ?? .object([:])
        guard case .object(let properties) = propertiesValue else { return nil }
        var required: Set<String> = []
        if case .array(let names)? = fields["required"] {
            required = Set(names.compactMap(\.stringValue))
        }
        var built: [DynamicGenerationSchema.Property] = []
        for (key, value) in properties.sorted(by: { $0.key < $1.key }) {
            guard let property = dynamicSchema(named: name + "-" + key, from: value)
            else { return nil }
            built.append(
                DynamicGenerationSchema.Property(
                    name: key, schema: property, isOptional: !required.contains(key)))
        }
        return DynamicGenerationSchema(name: name, properties: built)
    default:
        return nil
    }
}

// An offered tool that captures its invocation instead of executing it: the
// call is emitted as an event and the generation ends, since the executor
// sits on the far side of the wire.
private struct CapturingTool: FoundationModels.Tool {
    typealias Arguments = GeneratedContent
    typealias Output = String

    let name: String
    let description: String
    let parameters: GenerationSchema
    private let events: GenerationEvents

    init?(tool: WireTool, events: GenerationEvents) {
        // Only object-rooted schemas are offered: the capture path re-parses
        // the generated arguments as a JSON object (the convention every
        // dialect on the far side speaks), so a bare-string or array root
        // would generate arguments the wire cannot carry.
        let parameters = tool.parameters ?? .object([:])
        guard case .object(let fields) = parameters,
            fields["enum"] == nil,
            fields["type"] == nil || fields["type"]?.stringValue == "object",
            let root = dynamicSchema(named: tool.name, from: parameters),
            let schema = try? GenerationSchema(root: root, dependencies: [])
        else { return nil }
        self.name = tool.name
        self.description = tool.description ?? ""
        self.parameters = schema
        self.events = events
    }

    func call(arguments: GeneratedContent) async throws -> String {
        events.captureToolCall(name: name, argumentsJSON: arguments.jsonString)
        throw CancellationError()
    }
}

private func generatedContent(_ value: WireValue?) -> GeneratedContent {
    let json = (value ?? .object([:])).jsonString
    return (try? GeneratedContent(json: json)) ?? GeneratedContent(json)
}

private func makeSession(
    instructions: String?, history: [WireMessage], tools: [any FoundationModels.Tool]
) -> LanguageModelSession {
    guard !history.isEmpty else {
        return LanguageModelSession(model: .default, tools: tools, instructions: instructions)
    }
    var entries: [Transcript.Entry] = []
    if let instructions {
        entries.append(
            .instructions(
                Transcript.Instructions(
                    segments: [.text(Transcript.TextSegment(content: instructions))],
                    toolDefinitions: tools.map { Transcript.ToolDefinition(tool: $0) })))
    }
    var unansweredCallIds: [String] = []
    for message in history {
        if message.role == "assistant", let calls = message.tool_calls, !calls.isEmpty {
            // Text the model produced before calling is its own response
            // entry, so replay keeps the turn's preamble.
            if !message.content.isEmpty {
                entries.append(
                    .response(
                        Transcript.Response(
                            assetIDs: [],
                            segments: [.text(Transcript.TextSegment(content: message.content))])))
            }
            let ids = calls.map { $0.id ?? UUID().uuidString.lowercased() }
            unansweredCallIds.append(contentsOf: ids)
            entries.append(
                .toolCalls(
                    Transcript.ToolCalls(
                        zip(ids, calls).map { id, call in
                            Transcript.ToolCall(
                                id: id,
                                toolName: call.name,
                                arguments: generatedContent(call.arguments))
                        })))
        } else if message.role == "tool" {
            // An id-less dialect still needs the output to reference its
            // call, so the oldest unanswered call id stands in.
            let id: String
            if let explicit = message.tool_call_id {
                id = explicit
                if let index = unansweredCallIds.firstIndex(of: explicit) {
                    unansweredCallIds.remove(at: index)
                }
            } else if unansweredCallIds.isEmpty {
                id = UUID().uuidString.lowercased()
            } else {
                id = unansweredCallIds.removeFirst()
            }
            entries.append(
                .toolOutput(
                    Transcript.ToolOutput(
                        id: id,
                        toolName: message.tool_name ?? "",
                        segments: [.text(Transcript.TextSegment(content: message.content))])))
        } else if message.role == "assistant" {
            entries.append(
                .response(
                    Transcript.Response(
                        assetIDs: [],
                        segments: [.text(Transcript.TextSegment(content: message.content))])))
        } else {
            entries.append(
                .prompt(
                    Transcript.Prompt(
                        segments: [.text(Transcript.TextSegment(content: message.content))])))
        }
    }
    return LanguageModelSession(
        model: .default, tools: tools, transcript: Transcript(entries: entries))
}

private func tokenCount(_ text: String) async -> Int? {
    guard #available(macOS 26.4, *) else { return nil }
    return try? await SystemLanguageModel.default.tokenCount(for: text)
}

private func donePayload(promptTokens: Int?, completionTokens: Int?) -> String {
    let prompt = promptTokens.map(String.init) ?? "null"
    let completion = completionTokens.map(String.init) ?? "null"
    return "{\"prompt_tokens\":\(prompt),\"completion_tokens\":\(completion)}"
}

private func errorMessage(for error: LanguageModelSession.GenerationError) -> String {
    switch error {
    case .guardrailViolation, .refusal:
        return "Apple's model declined this request."
    case .exceededContextWindowSize:
        return "The conversation no longer fits Apple Intelligence's context window."
    case .rateLimited:
        return "Apple's model is rate limiting requests — try again in a moment."
    case .concurrentRequests:
        return "Apple's model is busy with another request."
    case .assetsUnavailable:
        return
            "Apple's model isn't available right now — check Apple Intelligence in System Settings."
    default:
        return "Apple's model hit an error: \(error.localizedDescription)"
    }
}

@_cdecl("hedos_af_abi_version")
public func hedos_af_abi_version() -> UInt32 {
    2
}

@_cdecl("hedos_af_availability")
public func hedos_af_availability() -> Int32 {
    switch SystemLanguageModel.default.availability {
    case .available:
        return 0
    case .unavailable(let reason):
        switch reason {
        case .appleIntelligenceNotEnabled:
            return 1
        case .modelNotReady:
            return 2
        case .deviceNotEligible:
            return 3
        @unknown default:
            return 2
        }
    }
}

@_cdecl("hedos_af_stream")
public func hedos_af_stream(
    _ requestJSON: UnsafePointer<CChar>?,
    _ context: UnsafeMutableRawPointer?,
    _ callback: HedosEventCallback?
) -> UInt64 {
    guard let callback else { return 0 }
    let ctx = CallbackContext(raw: context, callback: callback)
    guard let requestJSON,
        let request = try? JSONDecoder().decode(
            WireRequest.self, from: Data(bytes: requestJSON, count: strlen(requestJSON)))
    else {
        ctx.emit(.error, "the generation request could not be decoded")
        return 0
    }
    let handle = GenerationTable.shared.reserve()
    let events = GenerationEvents(ctx: ctx, handle: handle)
    let tools: [any FoundationModels.Tool] = (request.tools ?? []).compactMap {
        CapturingTool(tool: $0, events: events)
    }
    let task = Task {
        defer { GenerationTable.shared.finish(handle) }
        var finalText = ""
        func emitDone() async {
            let promptText = request.messages.map(\.content).joined(separator: "\n")
            events.emit(
                .done,
                donePayload(
                    promptTokens: await tokenCount(promptText),
                    completionTokens: await tokenCount(finalText)))
        }
        do {
            let parts = try split(request.messages)
            let session = makeSession(
                instructions: parts.instructions, history: parts.history, tools: tools)
            let options = GenerationOptions(
                sampling: samplingMode(request),
                temperature: request.temperature,
                maximumResponseTokens: request.max_tokens)
            for try await snapshot in session.streamResponse(to: parts.prompt, options: options) {
                try Task.checkCancellation()
                finalText = snapshot.content
                events.emit(.snapshot, snapshot.content)
            }
            await emitDone()
        } catch {
            // A capture ends the generation by cancelling it, so whatever
            // error that surfaces — cancellation, a tool-call error wrapping
            // the capture throw — the turn finished on purpose and ends done.
            if events.hasCaptured {
                await emitDone()
            } else if error is CancellationError {
                events.emit(.cancelled, "")
            } else if let error = error as? LanguageModelSession.GenerationError {
                events.emit(.error, errorMessage(for: error))
            } else if let error = error as? ShimError {
                events.emit(.error, error.text)
            } else {
                events.emit(.error, "Apple's model hit an error: \(error.localizedDescription)")
            }
        }
    }
    GenerationTable.shared.store(handle, task)
    return handle
}

@_cdecl("hedos_af_cancel")
public func hedos_af_cancel(_ handle: UInt64) {
    GenerationTable.shared.cancel(handle)
}
