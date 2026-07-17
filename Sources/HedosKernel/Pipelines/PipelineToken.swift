import Foundation

public enum PipelineInput: Sendable {
    case audio([Float])
    case text(String)
}

public enum PipelineToken: Sendable, Hashable {
    case text(String)
    case audioPCM([Float])
    case audioFrame(AudioFrame)
}

public enum PipelineEvent: Sendable {
    case stageStarted(index: Int, capability: Capability)
    case status(index: Int, String)
    case delta(index: Int, String)
    case transcript(index: Int, String)
    case audio(AudioFrame)
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
