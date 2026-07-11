import Foundation

public enum PipelineToken: Sendable, Hashable {
    case text(String)
    case audioPCM([Float])
    case audioFrame(AudioFrame)
    case image(Data)
    case artifact(String)
    case vector([Double])
}

public enum PipelineEvent: Sendable {
    case stageStarted(index: Int, capability: Capability)
    case status(index: Int, String)
    case delta(index: Int, String)
    case transcript(index: Int, String)
    case audio(AudioFrame)
    case artifact(id: String)
    case vector([Double])
    case completed
    case cancelled
    case failed(String)
}

public struct PipelineEventSink: Sendable {
    let emit: @Sendable (PipelineEvent) -> Void

    public init(emit: @escaping @Sendable (PipelineEvent) -> Void) {
        self.emit = emit
    }

    public func callAsFunction(_ event: PipelineEvent) {
        emit(event)
    }
}
