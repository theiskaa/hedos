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
        if let topK = object["top_k"]?.intValue {
            params.topK = Int32(topK)
        }
        if let minP = object["min_p"]?.doubleValue {
            params.minP = Float(minP)
        }
        if let repeatPenalty = object["repeat_penalty"]?.doubleValue {
            params.repeatPenalty = Float(repeatPenalty)
        }
        if let frequencyPenalty = object["frequency_penalty"]?.doubleValue {
            params.frequencyPenalty = Float(frequencyPenalty)
        }
        if let presencePenalty = object["presence_penalty"]?.doubleValue {
            params.presencePenalty = Float(presencePenalty)
        }
        if let seed = object["seed"]?.intValue {
            params.seed = UInt32(truncatingIfNeeded: seed)
        }
        params.stop = StopMatcher.strings(from: object["stop"])
        params.jsonGrammar = JSONGrammar.forResponseFormat(object["response_format"])
        if let maxTokens = object["max_tokens"]?.intValue, maxTokens > 0 {
            params.maxTokens = maxTokens
        }
        params.tools = ToolSpec.fromPayloadArray(object["tools"])
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

    func supportsTools(_ record: ModelRecord) -> Bool {
        true
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete else { return [] }
        return [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "context_length",
            "repeat_penalty", "frequency_penalty", "presence_penalty", "seed", "stop",
            "response_format",
        ]
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let path = record.primaryWeightPath ?? record.source.path
            guard case .object(let object) = payload else {
                continuation.finish(
                    throwing: KernelError.payloadInvalid("chat payload must be an object"))
                return
            }
            let messages: [ChatMessage]
            do {
                messages = try ChatMessage.parseAll(from: object)
            } catch {
                continuation.finish(throwing: error)
                return
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
