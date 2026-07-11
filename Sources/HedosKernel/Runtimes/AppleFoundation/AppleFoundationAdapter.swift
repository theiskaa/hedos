import Foundation

struct AppleFoundationAdapter: RuntimeAdapter {
    var id: RuntimeID { .appleFoundation }

    private let backend: any AppleFoundationBackend
    private let registry: Registry?

    init(
        backend: any AppleFoundationBackend = SystemFoundationBackend(), registry: Registry? = nil
    ) {
        self.backend = backend
        self.registry = registry
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .builtin else { return nil }
        return RuntimeBid(tier: .native, preference: BidPreference.appleFoundation)
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete else { return [] }
        return ["temperature", "max_tokens", "top_p", "top_k", "seed"]
    }

    static func delta(previous: String, current: String) -> String {
        guard current.hasPrefix(previous) else { return "" }
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

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let backend = backend
            let registry = registry
            let recordID = record.id
            let task = Task {
                do {
                    let messages = try Self.messages(from: payload, capability: capability)
                    var temperature: Double?
                    var topP: Double?
                    var topK: Int?
                    var seed: UInt64?
                    var maxTokens: Int?
                    if case .object(let object) = payload {
                        temperature = object["temperature"]?.doubleValue
                        topP = object["top_p"]?.doubleValue
                        topK = object["top_k"]?.intValue
                        if let seedValue = object["seed"]?.intValue {
                            seed = UInt64(truncatingIfNeeded: seedValue)
                        }
                        maxTokens = object["max_tokens"]?.intValue
                    }
                    if topP != nil, topK != nil {
                        throw KernelError.payloadInvalid(
                            "Apple Intelligence honors either top_p or top_k, not both")
                    }

                    let started = ContinuousClock.now
                    var previous = ""
                    var promptTokens: Int?
                    var completionTokens: Int?
                    let stream = backend.stream(
                        messages: messages, temperature: temperature, topP: topP, topK: topK,
                        seed: seed, maxTokens: maxTokens, tools: [], resultProvider: nil)
                    for try await event in stream {
                        switch event {
                        case .snapshot(let current):
                            let delta = Self.delta(previous: previous, current: current)
                            previous = current
                            guard !delta.isEmpty else { continue }
                            continuation.yield(.text(delta))
                        case .toolCalled(let call, _):
                            continuation.yield(.status("tool: " + call.name))
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
                                tokenCountsEstimated: true)))
                    await Self.markReachable(recordID, registry: registry)
                    continuation.finish()
                } catch where Task.isCancelled {
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    if backend.availability() != .available {
                        await Self.markUnreachable(recordID, registry: registry)
                    }
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func markUnreachable(_ id: String, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == .builtin,
            record.state != .missing
        else { return }
        _ = try? await registry.setStateIfPresent(id: id, to: .missing)
    }

    private static func markReachable(_ id: String, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == .builtin,
            record.state != .ready
        else { return }
        _ = try? await registry.setStateIfPresent(id: id, to: .ready)
    }
}
