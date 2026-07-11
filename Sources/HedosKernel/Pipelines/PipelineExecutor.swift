import Foundation

public struct PipelineExecutor: Sendable {
    private let stages: [PipelineStageRunner]

    public init(stages: [PipelineStageRunner]) {
        self.stages = stages
    }

    public func run(input: PipelineInput) -> AsyncStream<PipelineEvent> {
        let stages = stages
        return AsyncStream { continuation in
            let task = Task {
                await Self.execute(stages: stages, input: input, continuation: continuation)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func execute(
        stages: [PipelineStageRunner], input: PipelineInput,
        continuation: AsyncStream<PipelineEvent>.Continuation
    ) async {
        guard !stages.isEmpty else {
            continuation.yield(.completed)
            continuation.finish()
            return
        }

        let sink = PipelineEventSink { continuation.yield($0) }

        var channels: [(stream: AsyncStream<PipelineToken>, feed: AsyncStream<PipelineToken>.Continuation)] = []
        for _ in stages {
            let (stream, feed) = AsyncStream.makeStream(of: PipelineToken.self)
            channels.append((stream, feed))
        }

        switch input {
        case .audio(let samples):
            channels[0].feed.yield(.audioPCM(samples))
        case .text(let text):
            channels[0].feed.yield(.text(text))
        }
        channels[0].feed.finish()

        do {
            try await withThrowingTaskGroup(of: Void.self) { group in
                for (offset, stage) in stages.enumerated() {
                    let upstream = channels[offset].stream
                    let isTerminal = offset == stages.count - 1
                    let nextFeed = isTerminal ? nil : channels[offset + 1].feed
                    group.addTask {
                        sink(.stageStarted(index: stage.index, capability: stage.capability))
                        let downstream: PipelineStageRunner.Downstream = { token in
                            if let nextFeed {
                                nextFeed.yield(token)
                            } else {
                                switch token {
                                case .audioFrame(let frame):
                                    sink(.audio(frame))
                                case .artifact(let id):
                                    sink(.artifact(id: id))
                                case .vector(let values):
                                    sink(.vector(values))
                                case .text, .audioPCM, .image:
                                    break
                                }
                            }
                        }
                        defer { nextFeed?.finish() }
                        try await stage.run(upstream, downstream, sink)
                    }
                }
                try await group.waitForAll()
            }
            try Task.checkCancellation()
            continuation.yield(.completed)
        } catch is CancellationError {
            for channel in channels {
                channel.feed.finish()
            }
            continuation.yield(.cancelled)
        } catch {
            for channel in channels {
                channel.feed.finish()
            }
            continuation.yield(.failed(Self.message(for: error)))
        }
        continuation.finish()
    }

    private static func message(for error: any Error) -> String {
        if let kernel = error as? KernelError {
            return kernel.errorDescription ?? String(describing: error)
        }
        return error.localizedDescription
    }
}
