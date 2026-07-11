import Foundation

struct MlxSwiftAdapter: RuntimeAdapter {
    var id: RuntimeID { .mlxSwift }

    private let governor: MemoryGovernor
    private let engine: MlxSwiftEngine

    init(governor: MemoryGovernor = .shared, engine: MlxSwiftEngine = .shared) {
        self.governor = governor
        self.engine = engine
    }

    static func params(from object: [String: JSONValue]) -> MlxSwiftEngine.GenerationParams {
        var params = MlxSwiftEngine.GenerationParams()
        if let temperature = object["temperature"]?.doubleValue {
            params.temperature = Float(temperature)
        }
        if let topP = object["top_p"]?.doubleValue {
            params.topP = Float(topP)
        }
        if let repeatPenalty = object["repeat_penalty"]?.doubleValue {
            params.repeatPenalty = Float(repeatPenalty)
        }
        params.stop = StopMatcher.strings(from: object["stop"])
        if let maxTokens = object["max_tokens"]?.intValue, maxTokens > 0 {
            params.maxTokens = maxTokens
        }
        return params
    }

    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        record.contextLength
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete else { return false }
        if let runtimeID = record.runtime.id { return runtimeID == id }
        return false
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .mlxSafetensors,
            identified.modality == .text,
            identified.capabilities.contains(.chat)
        else { return nil }
        return RuntimeBid(tier: .native, preference: BidPreference.mlxSwift)
    }

    func supportsTools(_ record: ModelRecord) -> Bool {
        record.hasChatTemplate ?? false
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete else { return [] }
        return ["temperature", "top_p", "max_tokens", "repeat_penalty", "stop"]
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
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
            let directory = SidecarModelPaths.resolve(record).snapshot
            let governor = governor
            let engine = engine
            let params = Self.params(from: object)
            let tools = ToolSpec.fromPayloadArray(object["tools"])
            let task = Task {
                await engine.run(
                    path: directory,
                    modelID: record.id,
                    modelName: record.name,
                    footprintMB: record.footprintMB,
                    governor: governor,
                    messages: messages,
                    params: params,
                    tools: tools,
                    continuation: continuation)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
