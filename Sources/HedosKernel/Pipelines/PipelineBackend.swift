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

public struct PipelineStageRunner: Sendable {
    public typealias Downstream = @Sendable (PipelineToken) -> Void
    public typealias Body = @Sendable (
        _ upstream: AsyncStream<PipelineToken>,
        _ downstream: Downstream,
        _ sink: PipelineEventSink
    ) async throws -> Void

    public let index: Int
    public let capability: Capability
    public let input: PipelinePort
    public let output: PipelinePort
    public let run: Body

    public init(
        index: Int, capability: Capability, input: PipelinePort, output: PipelinePort,
        run: @escaping Body
    ) {
        self.index = index
        self.capability = capability
        self.input = input
        self.output = output
        self.run = run
    }
}
