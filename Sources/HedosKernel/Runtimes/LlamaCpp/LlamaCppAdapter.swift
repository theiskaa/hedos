import Foundation

struct LlamaCppAdapter: RuntimeAdapter {
    var id: RuntimeID { .llamaCpp }

    private let governor: MemoryGovernor
    private let engine: LlamaEngine

    init(governor: MemoryGovernor = .shared, engine: LlamaEngine = .shared) {
        self.governor = governor
        self.engine = engine
    }

    static func effectiveContextTokens(record: ModelRecord, requested: Int?) -> Int {
        let base = record.contextLength.flatMap { $0 > 0 ? $0 : nil } ?? 4096
        let cappedDefault = min(base, 32768)
        let lower = min(512, base)
        return min(max(requested ?? cappedDefault, lower), base)
    }

    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        Self.effectiveContextTokens(record: record, requested: requested)
    }

    static func params(from object: [String: JSONValue]) -> LlamaEngine.GenerationParams {
        var params = LlamaEngine.GenerationParams()
        if let temperature = object["temperature"]?.doubleValue {
            params.temperature = Float(temperature)
        }
        if let topP = object["top_p"]?.doubleValue {
            params.topP = Float(topP)
        }
        if let maxTokens = object["max_tokens"]?.intValue, maxTokens > 0 {
            params.maxTokens = maxTokens
        }
        return params
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete else { return false }
        if let runtimeID = record.runtime.id { return runtimeID == id }
        return false
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .gguf, identified.capabilities.contains(.chat) else {
            return nil
        }
        return RuntimeBid(tier: .native, preference: BidPreference.llamaCpp)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let path = record.primaryWeightPath ?? record.source.path
            guard case .object(let object) = payload,
                case .array(let rawMessages)? = object["messages"]
            else {
                continuation.finish(
                    throwing: KernelError.runtimeFailed("chat payload must carry messages"))
                return
            }
            let messages = rawMessages.compactMap { value -> ChatMessage? in
                guard case .object(let fields) = value,
                    case .string(let role)? = fields["role"],
                    case .string(let content)? = fields["content"],
                    let parsedRole = ChatMessage.Role(rawValue: role)
                else { return nil }
                return ChatMessage(role: parsedRole, content: content)
            }
            let expanded = (path as NSString).expandingTildeInPath
            let governor = governor
            let engine = engine
            let params = Self.params(from: object)
            let contextTokens = Self.effectiveContextTokens(
                record: record, requested: object["context_length"]?.intValue)
            let task = Task {
                await engine.run(
                    path: expanded,
                    modelID: record.id,
                    modelName: record.name,
                    footprintMB: record.footprintMB,
                    contextTokens: contextTokens,
                    governor: governor,
                    messages: messages,
                    params: params,
                    continuation: continuation)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
