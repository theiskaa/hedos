import Foundation
import Testing

@testable import HedosKernel

private func matrixRecord(
    name: String, capabilities: [Capability], runtimeID: RuntimeID? = .llamaCpp,
    sourceKind: SourceKind = .file, state: ModelState = .ready
) -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .text,
        capabilities: capabilities,
        source: ModelSource(kind: sourceKind, path: "/models/\(name)"),
        runtime: RuntimeRef(id: runtimeID, resolved: .auto, tier: .native),
        execution: .stream,
        state: state,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private actor MatrixKernel: ShelfSweepKernel {
    let records: [ModelRecord]
    let chatResult: Result<[CapabilityChunk], KernelError>
    let invokeResult: Result<[CapabilityChunk], KernelError>?

    init(
        records: [ModelRecord],
        chat: Result<[CapabilityChunk], KernelError> = .success([
            .text("hello"), .done(GenerationStats(completionTokens: 4, durationMs: 5)),
        ]),
        invoke: Result<[CapabilityChunk], KernelError>? = nil
    ) {
        self.records = records
        self.chatResult = chat
        self.invokeResult = invoke
    }

    func shelf() async throws -> [ModelRecord] { records }

    func chat(_ modelID: String, messages: [ChatMessage]) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    {
        stream(from: chatResult)
    }

    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    {
        stream(from: invokeResult ?? chatResult)
    }

    private func stream(from result: Result<[CapabilityChunk], KernelError>)
        -> AsyncThrowingStream<CapabilityChunk, Error>
    {
        AsyncThrowingStream { continuation in
            switch result {
            case .success(let chunks):
                for chunk in chunks { continuation.yield(chunk) }
                continuation.finish()
            case .failure(let error):
                continuation.finish(throwing: error)
            }
        }
    }

    func submit(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> String
    { modelID }
    func job(id: String) async throws -> Job? { nil }
    func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        AsyncStream { $0.finish() }
    }
    func cancel(jobID: String) async {}
}

private let healthyInvoke: Result<[CapabilityChunk], KernelError> = .success([
    .toolCall(ToolCall(name: "get_time", arguments: .object([:]))),
    .text("ok"),
    .done(GenerationStats(completionTokens: 4, durationMs: 5)),
])

@Test func classifyReadsTheRecordFields() {
    #expect(ConformanceMatrix.classify(matrixRecord(name: "e", capabilities: [.embed])) == .embedding)
    #expect(ConformanceMatrix.classify(matrixRecord(name: "v", capabilities: [.chat, .see])) == .vision)
    #expect(
        ConformanceMatrix.classify(matrixRecord(name: "w", capabilities: [.transcribe])) == .whisper)
    #expect(ConformanceMatrix.classify(matrixRecord(name: "t", capabilities: [.speak])) == .tts)
    #expect(ConformanceMatrix.classify(matrixRecord(name: "i", capabilities: [.image])) == .diffusion)
    #expect(
        ConformanceMatrix.classify(
            matrixRecord(name: "g", capabilities: [.chat], runtimeID: .llamaCpp)) == .ggufChat)
    #expect(
        ConformanceMatrix.classify(
            matrixRecord(name: "m", capabilities: [.chat], runtimeID: .mlxLm)) == .mlxChat)
    #expect(
        ConformanceMatrix.classify(
            matrixRecord(name: "o", capabilities: [.chat], runtimeID: .ollama)) == .ollamaChat)
    #expect(
        ConformanceMatrix.classify(
            matrixRecord(name: "r", capabilities: [.chat], runtimeID: .openAIEndpoint))
            == .remoteEndpoint)
    #expect(ConformanceMatrix.classify(matrixRecord(name: "x", capabilities: [])) == nil)
}

@Test func applicabilityMarksNonApplicableContracts() {
    #expect(!ConformanceMatrix.applies(.toolsCallable, to: .tts))
    #expect(!ConformanceMatrix.applies(.statsTruthful, to: .remoteEndpoint))
    #expect(!ConformanceMatrix.applies(.cancelClean, to: .diffusion))
    #expect(ConformanceMatrix.applies(.stateHonest, to: .embedding))
    #expect(ConformanceMatrix.applies(.paramsHonored, to: .ggufChat))
}

@Test func healthyChatRecordPassesEveryApplicableContract() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "good", capabilities: [.chat])], invoke: healthyInvoke)
    let cells = await ConformanceMatrix.run(kernel)
    #expect(cells.count == ConformanceContract.allCases.count)
    #expect(!cells.contains { $0.status == .fail })
    let honored = try? #require(cells.first { $0.contract == .paramsHonored })
    #expect(honored?.status == .pass)
}

@Test func leakedThinkTagFailsThinkingSeparatedAndPromotesToReasoning() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "reasoner", capabilities: [.chat])],
        chat: .success([
            .text("<think>2+2</think>"), .text("4"),
            .done(GenerationStats(completionTokens: 4, durationMs: 5)),
        ]),
        invoke: healthyInvoke)
    let cells = await ConformanceMatrix.run(kernel)
    let think = cells.first { $0.contract == .thinkingSeparated }
    #expect(think?.status == .fail)
    #expect(think?.conformanceClass == .ggufReasoning)
    #expect(cells.first { $0.contract == .templateCorrect }?.status == .pass)
}

@Test func rawControlTokenFailsTemplateCorrect() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "leaky", capabilities: [.chat])],
        chat: .success([
            .text("<|im_start|>assistant hi"),
            .done(GenerationStats(completionTokens: 4, durationMs: 5)),
        ]),
        invoke: healthyInvoke)
    let cells = await ConformanceMatrix.run(kernel)
    #expect(cells.first { $0.contract == .templateCorrect }?.status == .fail)
}

@Test func overspentTokenBudgetFailsParamsHonored() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "greedy", capabilities: [.chat])],
        invoke: .success([.done(GenerationStats(completionTokens: 99, durationMs: 5))]))
    let cells = await ConformanceMatrix.run(kernel)
    #expect(cells.first { $0.contract == .paramsHonored }?.status == .fail)
}

@Test func zeroStatsFailsStatsTruthful() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "silent", capabilities: [.chat])],
        chat: .success([.text("hi"), .done(GenerationStats(completionTokens: 0, durationMs: 0))]),
        invoke: healthyInvoke)
    let cells = await ConformanceMatrix.run(kernel)
    #expect(cells.first { $0.contract == .statsTruthful }?.status == .fail)
}

@Test func smokeFailureFailsEveryApplicableCell() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "broken", capabilities: [.chat])],
        chat: .failure(.runtimeFailed("engine crashed")))
    let cells = await ConformanceMatrix.run(kernel)
    let applicable = cells.filter { $0.status != .skip }
    #expect(!applicable.isEmpty)
    #expect(applicable.allSatisfy { $0.status == .fail })
    #expect(applicable.allSatisfy { $0.reason == "engine crashed" })
}

@Test func jobBasedImageModelPassesStateHonestViaTheJobPath() async {
    let image = ModelRecord(
        name: "sdxl", modality: .image, capabilities: [.image],
        source: ModelSource(kind: .huggingfaceCache, path: "/models/sdxl"),
        runtime: RuntimeRef(id: "python:diffusers", resolved: .auto, tier: .managed),
        execution: .job, state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
    let kernel = MatrixKernel(records: [image])
    let cells = await ConformanceMatrix.run(kernel)
    let state = cells.first { $0.contract == .stateHonest }
    #expect(state?.status == .pass)
}

@Test func notApplicableContractsAreSkippedNotAbsent() async {
    let kernel = MatrixKernel(records: [matrixRecord(name: "tts", capabilities: [.speak])])
    let cells = await ConformanceMatrix.run(kernel)
    let tools = try? #require(cells.first { $0.contract == .toolsCallable })
    #expect(tools?.status == .skip)
    #expect(tools?.reason == "not applicable")
}

@Test func baselineCatchesARegression() {
    let passing = ConformanceCell(
        model: "m", conformanceClass: .ggufChat, contract: .paramsHonored, status: .pass,
        durationMs: 1)
    let baseline = ConformanceBaseline.from([passing])
    let regressed = ConformanceCell(
        model: "m", conformanceClass: .ggufChat, contract: .paramsHonored, status: .fail,
        durationMs: 1, reason: "broke")
    #expect(baseline.regressions(in: [regressed]).count == 1)
    #expect(baseline.regressions(in: [passing]).isEmpty)
    let vanished = baseline.regressions(in: [])
    #expect(vanished.count == 1)
    #expect(vanished.first?.reason == "no longer present on the shelf")
}

@Test func paramsHonoredSkipsWhenNoTokenCountIsReported() async {
    let kernel = MatrixKernel(
        records: [matrixRecord(name: "usageless", capabilities: [.chat])],
        invoke: .success([.text("ok"), .done(GenerationStats(durationMs: 5))]))
    let cells = await ConformanceMatrix.run(kernel)
    #expect(cells.first { $0.contract == .paramsHonored }?.status == .skip)
}

@Test func baselineRoundTripsThroughTheKernelDirectory() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let cells = [
        ConformanceCell(
            model: "m", conformanceClass: .mlxChat, contract: .stateHonest, status: .pass,
            durationMs: 2)
    ]
    try ConformanceBaseline.from(cells).save(kernelDirectory: dir)
    let loaded = try #require(ConformanceBaseline.load(kernelDirectory: dir))
    #expect(loaded.passing.count == 1)
}

@Test func environmentalGapsReportWhisperAndKokoro() {
    let bare = EnvironmentalGaps.open(shelf: [])
    #expect(bare.contains { $0.contains("whisper") })
    #expect(bare.contains { $0.contains("kokoro") })

    let withWhisper = EnvironmentalGaps.open(shelf: [
        matrixRecord(name: "whisper", capabilities: [.transcribe], runtimeID: .whisperCpp)
    ])
    #expect(!withWhisper.contains { $0.contains("whisper") })
}
