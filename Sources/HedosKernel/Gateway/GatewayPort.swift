import Foundation

public protocol GatewayPort: Sendable {
    func shelf() async throws -> [ModelRecord]
    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    func submit(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> String
    func job(id: String) async throws -> Job?
    func jobEvents(id: String) async -> AsyncStream<JobEvent>
    func cancel(jobID: String) async
    func voices(_ modelID: String) async throws -> [String]
    func artifactData(id: String) async throws -> Data?
    func admissionState(modelID: String, footprintMB: Int?, kind: GatewayWorkKind) async
        -> GatewayAdmissionState
    func pipelines() async -> [Pipeline]
    func pipeline(id: String) async -> Pipeline?
    func runPipeline(id: String, input: PipelineInput) async throws
        -> AsyncStream<PipelineEvent>
}
