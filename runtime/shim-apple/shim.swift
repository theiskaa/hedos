// The Swift shim bridging Apple's FoundationModels framework to the Rust
// runtime over a flat C ABI, ported from the Swift kernel's
// SystemFoundationBackend (macos-app branch, f85874f). Compiled by the
// crate's build script (feature `apple-foundation`) into
// `libhedos_apple_shim.dylib`; loaded and driven by
// `runtime/src/adapters/apple_foundation/ffi.rs`.
//
// The ABI:
// - `hedos_af_abi_version() -> u32` — this contract's version; currently 1.
// - `hedos_af_availability() -> i32` — 0 available, 1 Apple Intelligence not
//   enabled, 2 model not ready (still downloading), 3 device not eligible.
// - `hedos_af_stream(request_json, ctx, callback) -> u64` — starts a
//   generation and returns its handle (never 0). The request is
//   `{"messages":[{"role","content"},…],"temperature","top_p","top_k",
//   "seed","max_tokens"}` with every option omittable. Returns 0 without
//   starting one when the callback is null (no event fires) or the request
//   fails to decode (one `error` event fires synchronously on the calling
//   thread, before the return — the only re-entrant emit; every other event
//   arrives later, from the generation task).
// - `hedos_af_cancel(handle)` — requests cooperative cancellation; a
//   terminal event still follows, though not necessarily `cancelled` (a
//   generation already past its stream loop finishes as `done` or `error`).
//
// Events arrive through the callback as `(ctx, kind, payload)` on an
// arbitrary thread, the payload pointer valid only during the call (see
// `Event` for the kinds). Every generation ends with exactly one terminal
// event — done, error, or cancelled — after which the shim never touches
// `ctx` again, so the caller frees it on the terminal event.

import Foundation
import FoundationModels

public typealias HedosEventCallback = @convention(c) (
    UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?
) -> Void

// The callback event kinds. `snapshot` is the only non-terminal event and
// carries the full reply text so far (cumulative, not a delta); `done`
// carries `{"prompt_tokens":n|null,"completion_tokens":n|null}`; `error` a
// human-readable message; `cancelled` an empty payload.
private enum Event: Int32 {
    case snapshot = 0
    case done = 1
    case error = 2
    case cancelled = 3
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

private struct WireMessage: Decodable {
    let role: String
    let content: String
}

private struct WireRequest: Decodable {
    let messages: [WireMessage]
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

// System turns merge into the session instructions; the last user turn is the
// prompt; everything else becomes transcript history.
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
    guard let lastUser = conversation.lastIndex(where: { $0.role == "user" }) else {
        throw ShimError(text: "Apple Intelligence needs a user message to answer")
    }
    let prompt = conversation[lastUser].content
    conversation.remove(at: lastUser)
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

private func makeSession(instructions: String?, history: [WireMessage]) -> LanguageModelSession {
    guard !history.isEmpty else {
        return LanguageModelSession(model: .default, instructions: instructions)
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
        // This bridge wires no tools, so a stray tool-result turn reads as
        // user-provided context rather than a transcript tool entry.
        if message.role == "assistant" {
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
    return LanguageModelSession(model: .default, transcript: Transcript(entries: entries))
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
    1
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
    let task = Task {
        defer { GenerationTable.shared.finish(handle) }
        do {
            let parts = try split(request.messages)
            let session = makeSession(instructions: parts.instructions, history: parts.history)
            let options = GenerationOptions(
                sampling: samplingMode(request),
                temperature: request.temperature,
                maximumResponseTokens: request.max_tokens)
            var finalText = ""
            for try await snapshot in session.streamResponse(to: parts.prompt, options: options) {
                try Task.checkCancellation()
                finalText = snapshot.content
                ctx.emit(.snapshot, snapshot.content)
            }
            let promptText = request.messages.map(\.content).joined(separator: "\n")
            ctx.emit(
                .done,
                donePayload(
                    promptTokens: await tokenCount(promptText),
                    completionTokens: await tokenCount(finalText)))
        } catch is CancellationError {
            ctx.emit(.cancelled, "")
        } catch let error as LanguageModelSession.GenerationError {
            ctx.emit(.error, errorMessage(for: error))
        } catch let error as ShimError {
            ctx.emit(.error, error.text)
        } catch {
            ctx.emit(.error, "Apple's model hit an error: \(error.localizedDescription)")
        }
    }
    GenerationTable.shared.store(handle, task)
    return handle
}

@_cdecl("hedos_af_cancel")
public func hedos_af_cancel(_ handle: UInt64) {
    GenerationTable.shared.cancel(handle)
}
