import Foundation

public protocol PipelineBackend: Sendable {
    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
}
