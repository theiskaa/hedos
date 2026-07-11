import Foundation

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
