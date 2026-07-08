import Foundation
import Testing

@testable import HedosKernel

private func textStream(_ deltas: [String]) -> AsyncThrowingStream<CapabilityChunk, Error> {
    AsyncThrowingStream { continuation in
        for delta in deltas { continuation.yield(.text(delta)) }
        continuation.yield(.done(nil))
        continuation.finish()
    }
}

private func audioStream(_ frames: Int) -> AsyncThrowingStream<CapabilityChunk, Error> {
    AsyncThrowingStream { continuation in
        for _ in 0..<frames {
            continuation.yield(
                .audio(AudioFrame(data: Data([0, 0, 0, 0]), sampleRate: 24000)))
        }
        continuation.yield(.done(nil))
        continuation.finish()
    }
}

final class FakePipelineBackend: PipelineBackend, @unchecked Sendable {
    struct Call: Sendable {
        let modelID: String
        let capability: Capability
        let payload: JSONValue
    }

    private let lock = NSLock()
    private(set) var calls: [Call] = []
    var cancelledJobs: [String] = []

    var transcribeText: [String] = ["hello there"]
    var chatDeltas: [String] = ["Hi. ", "How are you?"]
    var speakFramesPerSentence = 2
    var jobArtifacts: [String] = ["artifact-1"]
    var jobFailure: String?
    var chatHang = false

    func record(_ call: Call) {
        lock.lock()
        calls.append(call)
        lock.unlock()
    }

    var speakCallCount: Int {
        lock.lock(); defer { lock.unlock() }
        return calls.filter { $0.capability == .speak }.count
    }

    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    {
        record(Call(modelID: modelID, capability: capability, payload: payload))
        switch capability {
        case .transcribe: return textStream(transcribeText)
        case .speak: return audioStream(speakFramesPerSentence)
        case .chat, .complete:
            if chatHang {
                return AsyncThrowingStream { continuation in
                    Task {
                        try? await Task.sleep(for: .seconds(30))
                        continuation.finish()
                    }
                }
            }
            return textStream(chatDeltas)
        default: return textStream([])
        }
    }

    func submit(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> String
    {
        record(Call(modelID: modelID, capability: capability, payload: payload))
        return "job-fake"
    }

    func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        let failure = jobFailure
        let artifacts = jobArtifacts
        return AsyncStream { continuation in
            continuation.yield(.running)
            if let failure {
                continuation.yield(.failed(message: failure))
            } else {
                continuation.yield(.done(result: artifacts))
            }
            continuation.finish()
        }
    }

    func recordCancel(_ jobID: String) {
        lock.lock()
        cancelledJobs.append(jobID)
        lock.unlock()
    }

    func cancel(jobID: String) async {
        recordCancel(jobID)
    }

    func artifactData(id: String) async throws -> Data? { Data([1, 2, 3]) }
}

private func collect(_ stream: AsyncStream<PipelineEvent>) async -> [PipelineEvent] {
    var events: [PipelineEvent] = []
    for await event in stream { events.append(event) }
    return events
}

@Test func chatOnlyStreamsTextDeltasInOrder() async throws {
    let backend = FakePipelineBackend()
    let runner = PipelineRunnerFactory.textToText(
        index: 0, modelID: "chat", capability: .chat, params: [:], backend: backend)
    let events = await collect(
        PipelineExecutor(stages: [runner]).run(input: .text("hi")))
    let deltas = events.compactMap { event -> String? in
        if case .delta(_, let d) = event { return d }
        return nil
    }
    #expect(deltas == ["Hi. ", "How are you?"])
    if case .completed = events.last {} else { Issue.record("should complete") }
    if case .object(let payload) = backend.calls[0].payload,
        case .array(let messages)? = payload["messages"], case .object(let first) = messages[0]
    {
        #expect(first["content"] == .string("hi"))
    } else {
        Issue.record("chat payload should carry the user message")
    }
}

@Test func transcribeChatSpeakOrdersEventsAcrossEdges() async throws {
    let backend = FakePipelineBackend()
    let runners = [
        PipelineRunnerFactory.transcribe(
            index: 0, modelID: "asr", params: [:], sampleRate: 16000, backend: backend),
        PipelineRunnerFactory.textToText(
            index: 1, modelID: "chat", capability: .chat, params: [:], backend: backend),
        PipelineRunnerFactory.speak(
            index: 2, modelID: "tts", params: [:], voice: "af_heart", backend: backend),
    ]
    let events = await collect(
        PipelineExecutor(stages: runners).run(input: .audio([0.1, 0.2, 0.3])))

    let transcript = events.compactMap { event -> String? in
        if case .transcript(_, let t) = event { return t }
        return nil
    }
    #expect(transcript == ["hello there"])
    #expect(events.contains { if case .audio = $0 { return true }; return false })
    if case .completed = events.last {} else { Issue.record("should complete") }

    let capsInOrder = events.compactMap { event -> Capability? in
        if case .stageStarted(_, let c) = event { return c }
        return nil
    }
    #expect(capsInOrder.contains(.transcribe))
    #expect(capsInOrder.contains(.speak))

    let transcribeCall = backend.calls.first { $0.capability == .transcribe }
    if case .object(let payload)? = transcribeCall?.payload {
        #expect(payload["pcm"] != nil)
        #expect(payload["sampleRate"] == .int(16000))
    } else {
        Issue.record("transcribe should receive pcm + sampleRate")
    }
}

@Test func textToSpeakUsesSentenceChunking() async throws {
    let backend = FakePipelineBackend()
    backend.chatDeltas = ["First sentence. ", "Second sentence. ", "Third one."]
    let runners = [
        PipelineRunnerFactory.textToText(
            index: 0, modelID: "chat", capability: .chat, params: [:], backend: backend),
        PipelineRunnerFactory.speak(
            index: 1, modelID: "tts", params: [:], voice: "af_heart", backend: backend),
    ]
    _ = await collect(PipelineExecutor(stages: runners).run(input: .text("go")))
    #expect(backend.speakCallCount >= 2)
}

@Test func imageTailAggregatesTextThenEmitsArtifact() async throws {
    let backend = FakePipelineBackend()
    let runners = [
        PipelineRunnerFactory.textToText(
            index: 0, modelID: "chat", capability: .chat, params: [:], backend: backend),
        PipelineRunnerFactory.image(
            index: 1, modelID: "flux", params: [:], backend: backend),
    ]
    let events = await collect(PipelineExecutor(stages: runners).run(input: .text("draw")))
    let artifacts = events.compactMap { event -> String? in
        if case .artifact(let id) = event { return id }
        return nil
    }
    #expect(artifacts == ["artifact-1"])
}

@Test func imageJobFailurePropagatesAsFailedEvent() async throws {
    let backend = FakePipelineBackend()
    backend.jobFailure = "diffusion exploded"
    let runner = PipelineRunnerFactory.image(
        index: 0, modelID: "flux", params: [:], backend: backend)
    let events = await collect(PipelineExecutor(stages: [runner]).run(input: .text("boom")))
    let failed = events.compactMap { event -> String? in
        if case .failed(let m) = event { return m }
        return nil
    }
    #expect(failed.count == 1)
    #expect(failed[0].contains("diffusion exploded"))
}

@Test func cancellationTearsDownAndCancelsJob() async throws {
    let backend = FakePipelineBackend()
    backend.jobFailure = nil
    let slowRunner = PipelineStageRunner(
        index: 0, capability: .image, input: .text, output: .image
    ) { _, downstream, _ in
        let jobID = try await backend.submit("flux", .image, payload: .null)
        try await withTaskCancellationHandler {
            try await Task.sleep(for: .seconds(30))
            downstream(.artifact(jobID))
        } onCancel: {
            Task { await backend.cancel(jobID: jobID) }
        }
    }
    let stream = PipelineExecutor(stages: [slowRunner]).run(input: .text("x"))
    let consume = Task { await collect(stream) }
    try await Task.sleep(for: .milliseconds(200))
    consume.cancel()
    _ = await consume.value
    try await Task.sleep(for: .milliseconds(200))
    #expect(backend.cancelledJobs == ["job-fake"])
}
