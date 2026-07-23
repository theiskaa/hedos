// The Swift shim bridging the in-process MLX-Swift runtime to the Rust runtime
// over a flat C ABI, ported from the Swift kernel's MlxSwiftEngine/MlxSwiftAdapter
// (macos-app branch, f85874f). Compiled by the crate's build script on macOS
// into `libHedosMlxShim.dylib`; loaded and driven by
// `runtime/src/adapters/mlx_swift/ffi.rs`.
//
// The ABI:
// - `hedos_mlx_abi_version() -> u32` — this contract's version; currently 1.
// - `hedos_mlx_available() -> i32` — 1 when the in-process engine can run, else 0.
// - `hedos_mlx_stream(request_json, ctx, callback) -> u64` — starts a generation
//   and returns its handle (never 0). The request is
//   `{"model":"<dir>","messages":[…],"tools":[…]?,"temperature"?,"top_p"?,
//   "repeat_penalty"?,"max_tokens"?,"stop":[…]?}`. A message is
//   `{"role","content"}` plus, on an assistant turn, `"tool_calls":
//   [{"id","name","arguments"}]` and, on a tool turn, `"tool_call_id"` and
//   `"tool_name"`. A tool is `{"name","description","parameters"}`. Returns 0
//   without starting a generation when the callback is null or the request fails
//   to decode (one `error` event fires synchronously before the return).
// - `hedos_mlx_cancel(handle)` — requests cooperative cancellation; a terminal
//   event still follows.
//
// Events arrive through the callback as `(ctx, kind, payload)` on an arbitrary
// thread, the payload pointer valid only during the call. Every generation ends
// with exactly one terminal event — done, error, or cancelled — after which the
// shim never touches `ctx` again, so the caller frees it on the terminal event.
// Unlike the Apple bridge there is no capture-and-cancel: the model emits tool
// calls inline (a `tool_call` event) and the generation continues; the turn ends
// with `done` carrying `finish_reason:"tool_calls"` when any call was made.
//
// The shim owns the think-splitting and stop-matching (ports of the Swift
// kernel's ThinkSplitter/StopMatcher), so it streams already-separated visible
// text and reasoning, and halts generation early on a stop match.

import Foundation
import MLX
import MLXLLM
import MLXLMCommon

public typealias HedosEventCallback = @convention(c) (
    UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?
) -> Void

// The callback event kinds. `text`/`thinking`/`status` carry a UTF-8 string;
// `toolCall` a `{"name","arguments":{…}}` object; `done` carries
// `{"prompt_tokens":n|null,"completion_tokens":n|null,"load_ms":n|null,
// "finish_reason":s|null,"token_counts_estimated":bool}`; `error` a
// human-readable message; `cancelled` an empty payload. The first four are the
// non-terminal kinds.
private enum Event: Int32 {
    case text = 0
    case thinking = 1
    case toolCall = 2
    case status = 3
    case done = 4
    case error = 5
    case cancelled = 6

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

// The event channel for one generation: every emit passes through one lock, and
// nothing emits after the terminal event — the Rust side frees the callback
// context on the terminal, so a late emit would touch freed memory. The
// callback is invoked under the lock and must never block or re-enter the shim;
// the Rust callback is a non-blocking channel send.
private final class GenerationEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var closed = false
    private let ctx: CallbackContext

    init(ctx: CallbackContext) {
        self.ctx = ctx
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
}

// Arbitrary JSON as it appears in tool parameters and call arguments.
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
            // Tried before Double so a full-range i64 argument survives without
            // rounding through a double.
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

    // An assistant turn's tool calls inlined as `<tool_call>{json}</tool_call>`
    // blocks after its content, matching the Swift `inlinedToolTranscript` — the
    // form the chat templates expect for an assistant that called tools.
    var inlinedAssistantContent: String {
        guard role == "assistant", let calls = tool_calls, !calls.isEmpty else { return content }
        let blocks = calls.map { call -> String in
            let object: [String: Any] = [
                "name": call.name,
                "arguments": (call.arguments ?? .object([:])).anyValue,
            ]
            let json =
                (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]))
                .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            return "<tool_call>\(json)</tool_call>"
        }
        return ([content] + blocks).filter { !$0.isEmpty }.joined(separator: "\n")
    }
}

private struct WireRequest: Decodable {
    let model: String
    let messages: [WireMessage]
    let tools: [WireTool]?
    let temperature: Double?
    let top_p: Double?
    let repeat_penalty: Double?
    let max_tokens: Int?
    let stop: [String]?
}

// A piece of separated output: visible text or reasoning.
private enum Piece {
    case text(String)
    case thinking(String)
}

// Splits `<think>…</think>` (and the Cohere `<|START_THINKING|>…`) spans out of
// the stream, holding back any partial open/close tag until enough arrives to
// decide. Port of the Swift kernel's ThinkSplitter.
private struct ThinkSplitter {
    private struct TagPair {
        let open: String
        let close: String
    }
    private static let pairs: [TagPair] = [
        TagPair(open: "<think>", close: "</think>"),
        TagPair(open: "<|START_THINKING|>", close: "<|END_THINKING|>"),
    ]
    private static let openTags = pairs.map(\.open)

    private enum Mode {
        case text
        case thinking(close: String)
    }
    private var mode: Mode = .text
    private var buffer = ""

    mutating func feed(_ chunk: String) -> [Piece] {
        buffer += chunk
        var output: [Piece] = []
        outer: while true {
            switch mode {
            case .text:
                var earliest: (range: Range<String.Index>, close: String)?
                for pair in Self.pairs {
                    guard let range = buffer.range(of: pair.open) else { continue }
                    if earliest == nil || range.lowerBound < earliest!.range.lowerBound {
                        earliest = (range, pair.close)
                    }
                }
                guard let found = earliest else {
                    let emit = Self.emittablePrefix(of: buffer, holdingForAny: Self.openTags)
                    if !emit.isEmpty {
                        output.append(.text(emit))
                        buffer.removeFirst(emit.count)
                    }
                    break outer
                }
                let before = String(buffer[buffer.startIndex..<found.range.lowerBound])
                if !before.isEmpty { output.append(.text(before)) }
                buffer.removeSubrange(buffer.startIndex..<found.range.upperBound)
                mode = .thinking(close: found.close)
            case .thinking(let close):
                guard let range = buffer.range(of: close) else {
                    let emit = Self.emittablePrefix(of: buffer, holdingForAny: [close])
                    if !emit.isEmpty {
                        output.append(.thinking(emit))
                        buffer.removeFirst(emit.count)
                    }
                    break outer
                }
                let before = String(buffer[buffer.startIndex..<range.lowerBound])
                if !before.isEmpty { output.append(.thinking(before)) }
                buffer.removeSubrange(buffer.startIndex..<range.upperBound)
                mode = .text
            }
        }
        return output
    }

    mutating func flush() -> [Piece] {
        guard !buffer.isEmpty else { return [] }
        let piece: Piece
        switch mode {
        case .thinking: piece = .thinking(buffer)
        case .text: piece = .text(buffer)
        }
        buffer = ""
        return [piece]
    }

    private static func emittablePrefix(of buffer: String, holdingForAny tags: [String]) -> String {
        let longest = tags.map(\.count).max() ?? 1
        let maxHold = Swift.min(longest - 1, buffer.count)
        guard maxHold > 0 else { return buffer }
        for length in stride(from: maxHold, through: 1, by: -1) {
            let suffix = String(buffer.suffix(length))
            if tags.contains(where: { $0.hasPrefix(suffix) }) {
                return String(buffer.dropLast(length))
            }
        }
        return buffer
    }
}

// Holds back a growing suffix that could begin a stop string, emitting the safe
// prefix; once a stop string completes, the rest is dropped and generation
// halts. Port of the Swift kernel's StopMatcher.
private struct StopMatcher {
    private let stops: [String]
    private var buffer = ""
    private(set) var stopped = false

    init(_ stops: [String]) {
        self.stops = stops.filter { !$0.isEmpty }
    }

    var isActive: Bool { !stops.isEmpty }

    mutating func feed(_ chunk: String) -> String {
        guard isActive, !stopped else { return stopped ? "" : chunk }
        buffer += chunk
        if let match = earliestStop() {
            let emit = String(buffer[buffer.startIndex..<match.lowerBound])
            buffer = ""
            stopped = true
            return emit
        }
        let held = heldSuffixLength()
        let emit = String(buffer.dropLast(held))
        buffer = String(buffer.suffix(held))
        return emit
    }

    mutating func flush() -> String {
        guard !stopped else { return "" }
        let emit = buffer
        buffer = ""
        return emit
    }

    private func earliestStop() -> Range<String.Index>? {
        var earliest: Range<String.Index>?
        for stop in stops {
            guard let range = buffer.range(of: stop) else { continue }
            if earliest == nil || range.lowerBound < earliest!.lowerBound {
                earliest = range
            }
        }
        return earliest
    }

    private func heldSuffixLength() -> Int {
        let longest = (stops.map(\.count).max() ?? 1) - 1
        let cap = Swift.min(longest, buffer.count)
        guard cap > 0 else { return 0 }
        for length in stride(from: cap, through: 1, by: -1) {
            let suffix = String(buffer.suffix(length))
            if stops.contains(where: { $0.hasPrefix(suffix) }) { return length }
        }
        return 0
    }
}

private enum ChatMLPrompt {
    static let noTemplateNotice = "this model has no chat template — using a generic format"

    static func render(_ messages: [WireMessage]) -> String {
        var prompt = ""
        for message in messages {
            prompt += "<|im_start|>\(message.role)\n\(message.content)<|im_end|>\n"
        }
        prompt += "<|im_start|>assistant\n"
        return prompt
    }
}

// Serializes generations across the whole shim: MLX's device and default
// stream are a shared, single context, so two generations running their token
// loops at once would corrupt each other — a port of the Swift engine's
// `generationSlot`. The Rust governor's GPU gate lets two same-model
// generations co-hold, so this slot, not the gate, is what enforces
// one-at-a-time here. Held across the whole run (load included); a queued
// generation waits its turn, and cancelling it while queued unblocks the next.
private actor GenerationSlot {
    static let shared = GenerationSlot()
    private var busy = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if !busy {
            busy = true
            return
        }
        await withCheckedContinuation { waiters.append($0) }
    }

    func release() {
        if waiters.isEmpty {
            busy = false
        } else {
            waiters.removeFirst().resume()
        }
    }
}

// A single loaded model container, cached by its directory so back-to-back
// requests for the same model reuse it; switching models evicts the previous.
private actor ModelCache {
    static let shared = ModelCache()
    private var loadedPath: String?
    private var container: ModelContainer?

    func container(forPath path: String) async throws -> ModelContainer {
        if loadedPath == path, let container {
            return container
        }
        container = nil
        loadedPath = nil
        MLX.GPU.clearCache()
        let configuration = ModelConfiguration(directory: URL(fileURLWithPath: path))
        let loaded = try await LLMModelFactory.shared.loadContainer(configuration: configuration)
        container = loaded
        loadedPath = path
        return loaded
    }
}

private func generateParameters(_ request: WireRequest) -> GenerateParameters {
    var parameters = GenerateParameters(
        maxTokens: (request.max_tokens ?? 0) > 0 ? request.max_tokens : nil,
        temperature: Float(request.temperature ?? 0.7),
        topP: Float(request.top_p ?? 1.0))
    if let repeatPenalty = request.repeat_penalty {
        parameters.repetitionPenalty = Float(repeatPenalty)
    }
    return parameters
}

private func toolCallPayload(name: String, arguments: [String: MLXLMCommon.JSONValue]) -> String {
    let object: [String: Any] = ["name": name, "arguments": arguments.mapValues(anyJSON)]
    return (try? JSONSerialization.data(withJSONObject: object))
        .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
}

private func anyJSON(_ value: MLXLMCommon.JSONValue) -> Any {
    switch value {
    case .null: return NSNull()
    case .bool(let flag): return flag
    case .int(let number): return NSNumber(value: number)
    case .double(let number): return number
    case .string(let text): return text
    case .array(let values): return values.map(anyJSON)
    case .object(let fields): return fields.mapValues(anyJSON)
    }
}

private func donePayload(
    promptTokens: Int?, completionTokens: Int?, loadMs: Int?, finishReason: String?,
    estimated: Bool
) -> String {
    var object: [String: Any] = ["token_counts_estimated": estimated]
    object["prompt_tokens"] = promptTokens.map { $0 as Any } ?? NSNull()
    object["completion_tokens"] = completionTokens.map { $0 as Any } ?? NSNull()
    object["load_ms"] = loadMs.map { $0 as Any } ?? NSNull()
    object["finish_reason"] = finishReason.map { $0 as Any } ?? NSNull()
    return (try? JSONSerialization.data(withJSONObject: object))
        .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
}

private func runGeneration(request: WireRequest, events: GenerationEvents) async {
    await GenerationSlot.shared.acquire()
    do {
        guard !request.messages.isEmpty else {
            throw ShimError(text: "chat produced no messages")
        }
        let loadStart = Date()
        let container = try await ModelCache.shared.container(forPath: request.model)
        let loadMs = Int(Date().timeIntervalSince(loadStart) * 1000)
        let parameters = generateParameters(request)
        let tools = request.tools ?? []
        let messages = request.messages

        var splitter = ThinkSplitter()
        var stopMatcher = StopMatcher(request.stop ?? [])
        var stoppedByMatch = false
        var emittedCharacters = 0
        var promptTokenCount = 0
        var completionTokenCount = 0
        var sawToolCall = false

        func emitText(_ text: String) {
            guard !text.isEmpty else { return }
            guard stopMatcher.isActive else {
                events.emit(.text, text)
                emittedCharacters += text.count
                return
            }
            let safe = stopMatcher.feed(text)
            if !safe.isEmpty {
                events.emit(.text, safe)
                emittedCharacters += safe.count
            }
            if stopMatcher.stopped { stoppedByMatch = true }
        }

        let stream: AsyncStream<Generation> = try await container.perform { context in
            let input: LMInput
            if context.tokenizer.hasChatTemplate {
                let chat: [Chat.Message] = messages.map { message in
                    switch message.role {
                    case "system": return .system(message.content)
                    case "assistant": return .assistant(message.inlinedAssistantContent)
                    case "tool": return .tool(message.content)
                    default: return .user(message.content)
                    }
                }
                let libraryTools: [[String: Any]]? =
                    tools.isEmpty
                    ? nil
                    : tools.map { spec in
                        [
                            "type": "function",
                            "function": [
                                "name": spec.name,
                                "description": spec.description ?? "",
                                "parameters": (spec.parameters ?? .object([:])).anyValue,
                            ] as [String: Any],
                        ]
                    }
                input = try await context.processor.prepare(
                    input: UserInput(chat: chat, tools: libraryTools))
            } else {
                events.emit(.status, ChatMLPrompt.noTemplateNotice)
                input = try await context.processor.prepare(
                    input: UserInput(prompt: .text(ChatMLPrompt.render(messages))))
            }
            let caches = context.model.newCache(parameters: parameters)
            return try MLXLMCommon.generate(
                input: input, cache: caches, parameters: parameters, context: context)
        }

        consume: for await generation in stream {
            try Task.checkCancellation()
            switch generation {
            case .chunk(let text):
                for piece in splitter.feed(text) {
                    switch piece {
                    case .text(let value):
                        emitText(value)
                        if stoppedByMatch { break consume }
                    case .thinking(let value):
                        events.emit(.thinking, value)
                    }
                }
            case .info(let info):
                promptTokenCount = info.promptTokenCount
                completionTokenCount = info.generationTokenCount
            case .toolCall(let call):
                sawToolCall = true
                events.emit(
                    .toolCall,
                    toolCallPayload(name: call.function.name, arguments: call.function.arguments))
            }
        }

        if !stoppedByMatch {
            for piece in splitter.flush() {
                switch piece {
                case .text(let value): emitText(value)
                case .thinking(let value): events.emit(.thinking, value)
                }
            }
            if stopMatcher.isActive, !stoppedByMatch {
                let tail = stopMatcher.flush()
                if !tail.isEmpty { events.emit(.text, tail) }
            }
        }

        let missedTerminalInfo = completionTokenCount == 0 && stoppedByMatch
        let completion =
            missedTerminalInfo
            ? max(1, emittedCharacters / 4)
            : (completionTokenCount == 0 ? nil : completionTokenCount)
        let finishReason = sawToolCall ? "tool_calls" : (stoppedByMatch ? "stop" : nil)
        events.emit(
            .done,
            donePayload(
                promptTokens: promptTokenCount == 0 ? nil : promptTokenCount,
                completionTokens: completion,
                loadMs: loadMs,
                finishReason: finishReason,
                estimated: missedTerminalInfo))
    } catch is CancellationError {
        events.emit(.cancelled, "")
    } catch let error as ShimError {
        events.emit(.error, error.text)
    } catch {
        events.emit(.error, "MLX generation failed: \(error.localizedDescription)")
    }
    await GenerationSlot.shared.release()
}

@_cdecl("hedos_mlx_abi_version")
public func hedos_mlx_abi_version() -> UInt32 {
    1
}

@_cdecl("hedos_mlx_available")
public func hedos_mlx_available() -> Int32 {
    // MLX loads its Metal kernels from `mlx.metallib` colocated with this dylib
    // (its C++ core finds the containing image via `dladdr`, then looks beside
    // it). A missing metallib makes the first GPU op throw a C++ exception that
    // would abort the host — uncatchable across the FFI — so gate availability
    // on the metallib actually being present, letting the adapter yield to the
    // Python sidecar instead of resolving to a runtime that would crash.
    var info = Dl_info()
    let probe = unsafeBitCast(
        hedos_mlx_available as @convention(c) () -> Int32, to: UnsafeMutableRawPointer.self)
    guard dladdr(probe, &info) != 0, let namePointer = info.dli_fname else { return 0 }
    let directory = (String(cString: namePointer) as NSString).deletingLastPathComponent
    let metallib = (directory as NSString).appendingPathComponent("mlx.metallib")
    return FileManager.default.fileExists(atPath: metallib) ? 1 : 0
}

@_cdecl("hedos_mlx_stream")
public func hedos_mlx_stream(
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
    let events = GenerationEvents(ctx: ctx)
    let task = Task {
        defer { GenerationTable.shared.finish(handle) }
        await runGeneration(request: request, events: events)
    }
    GenerationTable.shared.store(handle, task)
    return handle
}

@_cdecl("hedos_mlx_cancel")
public func hedos_mlx_cancel(_ handle: UInt64) {
    GenerationTable.shared.cancel(handle)
}
