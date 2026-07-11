import Foundation

public protocol PipelineBackend: Sendable {
    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    func submit(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> String
    func jobEvents(id: String) async -> AsyncStream<JobEvent>
    func cancel(jobID: String) async
    func artifactData(id: String) async throws -> Data?
}
