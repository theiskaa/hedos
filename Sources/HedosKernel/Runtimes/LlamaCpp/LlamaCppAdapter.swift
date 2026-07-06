import Foundation

public struct LlamaCppAdapter: RuntimeAdapter {
    public var id: String { "llama-cpp" }

    private let governor: MemoryGovernor

    public init(governor: MemoryGovernor = .shared) {
        self.governor = governor
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete else { return false }
        if let runtimeID = record.runtime.id { return runtimeID == id }
        return false
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .gguf, identified.capabilities.contains(.chat) else {
            return nil
        }
        return RuntimeBid(tier: .native, preference: 10)
    }

    public func invoke(
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
            let task = Task {
                await LlamaEngine.shared.run(
                    path: expanded,
                    modelID: record.id,
                    modelName: record.name,
                    footprintMB: record.footprintMB,
                    governor: governor,
                    messages: messages,
                    continuation: continuation)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
