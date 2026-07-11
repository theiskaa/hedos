import Foundation
import Testing

@testable import HedosKernel

private func sweepRecord(
    name: String,
    capabilities: [Capability],
    state: ModelState = .ready,
    sourceKind: SourceKind = .file,
    runtimeID: RuntimeID? = nil
) -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .text,
        capabilities: capabilities,
        source: ModelSource(kind: sourceKind, path: "/tmp/hedos-sweep/\(name)"),
        runtime: RuntimeRef(id: runtimeID, resolved: runtimeID == nil ? .unresolved : .auto),
        execution: capabilities.contains(.image) ? .job : .stream,
        state: state,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private actor FakeSweepKernel: ShelfSweepKernel {
    struct StreamOutcome {
        var chunks: [CapabilityChunk] = [.done(nil)]
        var error: KernelError?
        var delayMs: UInt64 = 5
        var hangs = false
    }

    struct JobOutcome {
        var events: [JobEvent] = [.done(result: [])]
        var finalJob: Job?
        var hangs = false
    }

    private let records: [ModelRecord]
    private let chatOutcomes: [String: StreamOutcome]
    private let invokeOutcomes: [String: StreamOutcome]
    private let jobOutcomes: [String: JobOutcome]
    private(set) var shelfCallCount = 0
    private(set) var submitCallCount = 0
    private(set) var cancelledJobIDs: [String] = []

    private let shelfError: KernelError?

    init(
        records: [ModelRecord],
        shelfError: KernelError? = nil,
        chatOutcomes: [String: StreamOutcome] = [:],
        invokeOutcomes: [String: StreamOutcome] = [:],
        jobOutcomes: [String: JobOutcome] = [:]
    ) {
        self.records = records
        self.shelfError = shelfError
        self.chatOutcomes = chatOutcomes
        self.invokeOutcomes = invokeOutcomes
        self.jobOutcomes = jobOutcomes
    }

    func shelf() async throws -> [ModelRecord] {
        shelfCallCount += 1
        if let shelfError { throw shelfError }
        return records
    }

    func chat(_ modelID: String, messages: [ChatMessage]) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        try await makeStream(modelID, table: chatOutcomes)
    }

    func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try await makeStream(modelID, table: invokeOutcomes)
    }

    private func makeStream(
        _ modelID: String, table: [String: StreamOutcome]
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let outcome = table[modelID] else {
            throw KernelError.modelNotFound(modelID)
        }
        if outcome.hangs {
            return AsyncThrowingStream { continuation in
                let task = Task {
                    try? await Task.sleep(for: .seconds(3600))
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        try await Task.sleep(nanoseconds: outcome.delayMs * 1_000_000)
        return AsyncThrowingStream { continuation in
            if let error = outcome.error {
                continuation.finish(throwing: error)
                return
            }
            for chunk in outcome.chunks {
                continuation.yield(chunk)
            }
            continuation.finish()
        }
    }

    func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String {
        submitCallCount += 1
        return modelID
    }

    func job(id: String) async throws -> Job? {
        jobOutcomes[id]?.finalJob
    }

    func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        let outcome = jobOutcomes[id]
        if outcome?.hangs == true {
            return AsyncStream { continuation in
                let task = Task {
                    try? await Task.sleep(for: .seconds(3600))
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        let events = outcome?.events ?? []
        return AsyncStream { continuation in
            for event in events {
                continuation.yield(event)
            }
            continuation.finish()
        }
    }

    func cancel(jobID: String) async {
        cancelledJobIDs.append(jobID)
    }
}

@Test func shelfSweepReportsParityFromChatGeneration() async throws {
    let separated = sweepRecord(name: "sep", capabilities: [.chat, .complete])
    let leaky = sweepRecord(name: "leaky", capabilities: [.chat])
    let kernel = FakeSweepKernel(
        records: [separated, leaky],
        chatOutcomes: [
            separated.id: .init(chunks: [
                .thinking("reasoning"),
                .status(ChatMLPrompt.noTemplateNotice),
                .text("hi there"),
                .done(GenerationStats()),
            ]),
            leaky.id: .init(chunks: [
                .text("<think>oops</think> hi"),
                .done(GenerationStats()),
            ]),
        ],
        invokeOutcomes: [
            separated.id: .init(chunks: [.text("4"), .done(GenerationStats())])
        ])
    let results = await ShelfSweep.run(kernel)

    let sep = results.first { $0.model == separated.displayName }
    #expect(sep?.parity?.thinkingSeparated == true)
    #expect(sep?.parity?.templateNoticeFired == true)
    #expect(sep?.parity?.promptCompleteOK == true)
    #expect(sep?.parity?.statsReported == true)

    let leakyResult = results.first { $0.model == leaky.displayName }
    #expect(leakyResult?.parity?.thinkingSeparated == false)
    #expect(leakyResult?.parity?.templateNoticeFired == false)

    let rendered = SweepReport.render(results)
    #expect(rendered.contains("think:ok"))
    #expect(rendered.contains("think:no"))
}

@Test func shelfSweepClassifiesPassFailAndSkip() async throws {
    let chatOk = sweepRecord(name: "chat-ok", capabilities: [.chat])
    let chatFail = sweepRecord(name: "chat-fail", capabilities: [.chat])
    let imageOk = sweepRecord(name: "image-ok", capabilities: [.image])
    let imageFail = sweepRecord(name: "image-fail", capabilities: [.image])
    let speakOk = sweepRecord(name: "speak-ok", capabilities: [.speak])
    let transcribeOk = sweepRecord(name: "transcribe-ok", capabilities: [.transcribe])
    let endpointModel = sweepRecord(
        name: "endpoint-model", capabilities: [.chat], sourceKind: .endpoint)
    let notReadyModel = sweepRecord(
        name: "not-ready-model", capabilities: [.chat], state: .unresolved)
    let ollamaDown = sweepRecord(
        name: "ollama-down", capabilities: [.chat], runtimeID: "ollama")
    let noCapability = sweepRecord(name: "embed-only", capabilities: [.embed])

    let kernel = FakeSweepKernel(
        records: [
            chatOk, chatFail, imageOk, imageFail, speakOk, transcribeOk, endpointModel,
            notReadyModel, ollamaDown, noCapability,
        ],
        chatOutcomes: [
            chatOk.id: .init(chunks: [.text("hi"), .done(nil)]),
            chatFail.id: .init(error: .runtimeFailed("boom")),
            ollamaDown.id: .init(error: .runtimeUnavailable(hint: "ollama isn't running")),
        ],
        invokeOutcomes: [
            speakOk.id: .init(chunks: [.done(nil)]),
            transcribeOk.id: .init(chunks: [.text("hello"), .done(nil)]),
        ],
        jobOutcomes: [
            imageOk.id: .init(events: [.running, .done(result: ["artifact-1"])]),
            imageFail.id: .init(events: [.running, .failed(message: "diffusion blew up")]),
        ])

    let results = await ShelfSweep.run(
        kernel, includeImage: true,
        transcribeFixture: URL(fileURLWithPath: "/tmp/hedos-sweep/fixture.wav"))
    let byModel = Dictionary(uniqueKeysWithValues: results.map { ($0.model, $0) })

    let chatOkResult = try #require(byModel["chat-ok"])
    #expect(chatOkResult.status == .pass)
    #expect(chatOkResult.capability == .chat)
    #expect(chatOkResult.durationMs > 0)
    #expect(chatOkResult.reason == nil)

    let chatFailResult = try #require(byModel["chat-fail"])
    #expect(chatFailResult.status == .fail)
    #expect(chatFailResult.capability == .chat)
    #expect(chatFailResult.reason == "boom")
    #expect(chatFailResult.durationMs > 0)

    let imageOkResult = try #require(byModel["image-ok"])
    #expect(imageOkResult.status == .pass)
    #expect(imageOkResult.capability == .image)

    let imageFailResult = try #require(byModel["image-fail"])
    #expect(imageFailResult.status == .fail)
    #expect(imageFailResult.capability == .image)
    #expect(imageFailResult.reason == "diffusion blew up")

    let speakOkResult = try #require(byModel["speak-ok"])
    #expect(speakOkResult.status == .pass)
    #expect(speakOkResult.capability == .speak)

    let transcribeOkResult = try #require(byModel["transcribe-ok"])
    #expect(transcribeOkResult.status == .pass)
    #expect(transcribeOkResult.capability == .transcribe)

    let endpointResult = try #require(byModel["endpoint-model"])
    #expect(endpointResult.status == .skip)
    #expect(endpointResult.reason == "endpoint runtime")
    #expect(endpointResult.durationMs == 0)

    let notReadyResult = try #require(byModel["not-ready-model"])
    #expect(notReadyResult.status == .skip)
    #expect(notReadyResult.reason == "not ready")

    let ollamaDownResult = try #require(byModel["ollama-down"])
    #expect(ollamaDownResult.status == .skip)
    #expect(ollamaDownResult.reason == "ollama isn't running")

    let noCapabilityResult = try #require(byModel["embed-only"])
    #expect(noCapabilityResult.status == .skip)
    #expect(noCapabilityResult.reason == "no sweepable capability")

    #expect(await kernel.shelfCallCount == 1)
}

@Test func shelfSweepIncludesEndpointsWhenRequested() async throws {
    let endpointModel = sweepRecord(
        name: "endpoint-model", capabilities: [.chat], sourceKind: .endpoint)
    let kernel = FakeSweepKernel(
        records: [endpointModel],
        chatOutcomes: [endpointModel.id: .init(chunks: [.done(nil)])])

    let results = await ShelfSweep.run(kernel, includeEndpoints: true, transcribeFixture: nil)
    let result = try #require(results.first)
    #expect(result.status == .pass)
    #expect(result.capability == .chat)
}

@Test func shelfSweepFallsBackToJobLookupWhenStreamEndsWithoutTerminalEvent() async throws {
    let imageSilentFail = sweepRecord(name: "image-silent-fail", capabilities: [.image])
    let kernel = FakeSweepKernel(
        records: [imageSilentFail],
        jobOutcomes: [
            imageSilentFail.id: .init(
                events: [.running],
                finalJob: Job(
                    id: imageSilentFail.id, modelID: imageSilentFail.id, capability: .image,
                    payload: .null, state: .failed, error: "silent failure"))
        ])

    let results = await ShelfSweep.run(kernel, includeImage: true, transcribeFixture: nil)
    let result = try #require(results.first)
    #expect(result.status == .fail)
    #expect(result.reason == "silent failure")
}

@Test func shelfSweepFailsTranscribeWithoutAFixture() async throws {
    let transcribeOk = sweepRecord(name: "transcribe-ok", capabilities: [.transcribe])
    let kernel = FakeSweepKernel(
        records: [transcribeOk],
        invokeOutcomes: [transcribeOk.id: .init(chunks: [.done(nil)])])

    let results = await ShelfSweep.run(kernel, transcribeFixture: nil)
    let result = try #require(results.first)
    #expect(result.status == .fail)
    #expect(result.reason == "no transcribe fixture available")
}

@Test func shelfSweepNeverTriggersADownloadPath() async throws {
    let chatOk = sweepRecord(name: "chat-ok", capabilities: [.chat])
    let kernel = FakeSweepKernel(
        records: [chatOk], chatOutcomes: [chatOk.id: .init(chunks: [.done(nil)])])

    _ = await ShelfSweep.run(kernel, transcribeFixture: nil)
    #expect(await kernel.shelfCallCount == 1)
}

@Test func shelfSweepTimesOutAHangingModelWithoutWedgingTheRun() async throws {
    let hangingModel = sweepRecord(name: "hangs-forever", capabilities: [.chat])
    let chatOk = sweepRecord(name: "chat-ok", capabilities: [.chat])
    let kernel = FakeSweepKernel(
        records: [hangingModel, chatOk],
        chatOutcomes: [
            hangingModel.id: .init(hangs: true),
            chatOk.id: .init(chunks: [.done(nil)]),
        ])

    let results = await ShelfSweep.run(
        kernel, transcribeFixture: nil, perModelTimeout: .milliseconds(200))
    let byModel = Dictionary(uniqueKeysWithValues: results.map { ($0.model, $0) })

    let hung = try #require(byModel["hangs-forever"])
    #expect(hung.status == .fail)
    #expect(hung.capability == .chat)
    #expect(hung.reason?.contains("timed out") == true)

    let ok = try #require(byModel["chat-ok"])
    #expect(ok.status == .pass)
    #expect(results.count == 2)
}

@Test func shelfSweepCancelsHangingImageJobOnTimeoutWithoutWedgingTheRun() async throws {
    let hangingImage = sweepRecord(name: "image-hangs-forever", capabilities: [.image])
    let chatOk = sweepRecord(name: "chat-ok", capabilities: [.chat])
    let kernel = FakeSweepKernel(
        records: [hangingImage, chatOk],
        chatOutcomes: [chatOk.id: .init(chunks: [.done(nil)])],
        jobOutcomes: [hangingImage.id: .init(hangs: true)])

    let results = await ShelfSweep.run(
        kernel, includeImage: true, transcribeFixture: nil, perModelTimeout: .milliseconds(200))
    let byModel = Dictionary(uniqueKeysWithValues: results.map { ($0.model, $0) })

    let hung = try #require(byModel["image-hangs-forever"])
    #expect(hung.status == .fail)
    #expect(hung.capability == .image)
    #expect(hung.reason?.contains("timed out") == true)

    let ok = try #require(byModel["chat-ok"])
    #expect(ok.status == .pass)
    #expect(results.count == 2)

    #expect(await kernel.cancelledJobIDs == [hangingImage.id])
}

@Test func shelfSweepSkipsImageByDefaultAndRunsItWhenOptedIn() async throws {
    let imageModel = sweepRecord(name: "image-model", capabilities: [.image])
    let kernel = FakeSweepKernel(
        records: [imageModel],
        jobOutcomes: [imageModel.id: .init(events: [.running, .done(result: ["artifact-1"])])])

    let skippedResults = await ShelfSweep.run(kernel, transcribeFixture: nil)
    let skipped = try #require(skippedResults.first)
    #expect(skipped.status == .skip)
    #expect(skipped.capability == .image)
    #expect(skipped.reason == "image generation not run in sweep — pass --include-image")
    #expect(await kernel.submitCallCount == 0)

    let includedResults = await ShelfSweep.run(
        kernel, includeImage: true, transcribeFixture: nil)
    let included = try #require(includedResults.first)
    #expect(included.status == .pass)
    #expect(included.capability == .image)
    #expect(await kernel.submitCallCount == 1)
}

@Test func unreadableShelfReportsAFailureRowInsteadOfACleanSweep() async {
    let kernel = FakeSweepKernel(
        records: [], shelfError: KernelError.runtimeFailed("models.json unreadable"))

    let results = await ShelfSweep.run(kernel, transcribeFixture: nil)

    #expect(results.count == 1)
    #expect(results[0].model == "shelf")
    #expect(results[0].status == .fail)
    #expect(results[0].reason?.contains("registry unreadable") == true)
}
