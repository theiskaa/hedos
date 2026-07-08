import Foundation

public struct AppleFoundationAdapter: RuntimeAdapter {
    public var id: String { "apple-foundation" }

    private let backend: any AppleFoundationBackend

    public init(backend: any AppleFoundationBackend = SystemFoundationBackend()) {
        self.backend = backend
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .builtin else { return nil }
        return RuntimeBid(tier: .native, preference: 15)
    }

    static func delta(previous: String, current: String) -> String {
        guard current.hasPrefix(previous) else { return current }
        return String(current.dropFirst(previous.count))
    }

    static func messages(from payload: JSONValue, capability: Capability) throws -> [ChatMessage] {
        guard case .object(let object) = payload else {
            throw KernelError.runtimeFailed("chat payload must be an object")
        }
        if capability == .complete {
            guard case .string(let prompt)? = object["prompt"] else {
                throw KernelError.runtimeFailed("complete payload must carry a prompt string")
            }
            return [ChatMessage(role: .user, content: prompt)]
        }
        guard case .array(let entries)? = object["messages"] else {
            throw KernelError.runtimeFailed("chat payload must carry a messages array")
        }
        return entries.compactMap { entry in
            guard case .object(let fields) = entry,
                case .string(let role)? = fields["role"],
                case .string(let content)? = fields["content"],
                let parsed = ChatMessage.Role(rawValue: role)
            else { return nil }
            return ChatMessage(role: parsed, content: content)
        }
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let backend = backend
            let task = Task {
                do {
                    let messages = try Self.messages(from: payload, capability: capability)
                    var temperature: Double?
                    var maxTokens: Int?
                    if case .object(let object) = payload {
                        temperature = object["temperature"]?.doubleValue
                        maxTokens = object["max_tokens"]?.intValue
                    }

                    let started = ContinuousClock.now
                    var ttftMs: Int?
                    var previous = ""
                    var promptTokens: Int?
                    var completionTokens: Int?
                    let stream = backend.stream(
                        messages: messages, temperature: temperature, maxTokens: maxTokens)
                    for try await event in stream {
                        switch event {
                        case .snapshot(let current):
                            let delta = Self.delta(previous: previous, current: current)
                            previous = current
                            guard !delta.isEmpty else { continue }
                            if ttftMs == nil {
                                ttftMs = Int(
                                    (ContinuousClock.now - started) / .milliseconds(1))
                            }
                            continuation.yield(.text(delta))
                        case .done(let prompt, let completion):
                            promptTokens = prompt
                            completionTokens = completion
                        }
                    }
                    let durationMs = Int((ContinuousClock.now - started) / .milliseconds(1))
                    continuation.yield(
                        .done(
                            GenerationStats(
                                promptTokens: promptTokens,
                                completionTokens: completionTokens,
                                durationMs: durationMs,
                                ttftMs: ttftMs)))
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
