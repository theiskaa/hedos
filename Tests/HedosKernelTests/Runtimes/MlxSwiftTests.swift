import Foundation
import MLXLMCommon
import Testing

@testable import HedosKernel

private func mlxTextRecord() -> ModelRecord {
    ModelRecord(
        name: "Llama-3.2-1B-Instruct-4bit",
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/models/huggingface/hub/models--mlx-community--Llama-3.2-1B-Instruct-4bit",
            repo: "mlx-community/Llama-3.2-1B-Instruct-4bit"),
        runtime: RuntimeRef(
            id: "mlx-swift",
            resolved: .auto,
            tier: .native),
        execution: .stream,
        footprintMB: 700,
        state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func mlxSwiftCanServeChatAndCompleteOnly() {
    let adapter = MlxSwiftAdapter()
    let record = mlxTextRecord()
    #expect(adapter.canServe(record, .chat))
    #expect(adapter.canServe(record, .complete))
    #expect(!adapter.canServe(record, .speak))
    #expect(!adapter.canServe(record, .transcribe))
    var other = record
    other.runtime.id = "python:mlx-lm"
    #expect(!adapter.canServe(other, .chat))
}

@Test func mlxSwiftAdapterBidMatrix() {
    let adapter = MlxSwiftAdapter()
    let record = mlxTextRecord()

    let mlxText = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.chat, .complete],
        execution: .stream)
    let bid = adapter.bid(record, mlxText)
    #expect(bid?.tier == .native)
    #expect(bid?.preference == 15)

    let plainText = IdentifiedModel(
        format: .safetensors, modality: .text, capabilities: [.chat, .complete],
        execution: .stream)
    let mlxSpeech = IdentifiedModel(
        format: .mlxSafetensors, modality: .speech, capabilities: [.speak], execution: .stream)
    let ggufText = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat, .complete], execution: .stream)
    let mlxTextNoChat = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.complete], execution: .stream)
    #expect(adapter.bid(record, plainText) == nil)
    #expect(adapter.bid(record, mlxSpeech) == nil)
    #expect(adapter.bid(record, ggufText) == nil)
    #expect(adapter.bid(record, mlxTextNoChat) == nil)
}

@Test func mlxSwiftBidOutranksMlxLmSidecar() throws {
    let mlxSwift = MlxSwiftAdapter()
    let mlxLm = MlxLmAdapter()
    let record = mlxTextRecord()
    let identified = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.chat, .complete],
        execution: .stream)
    let swiftBid = try #require(mlxSwift.bid(record, identified))
    let lmBid = try #require(mlxLm.bid(record, identified))
    #expect(swiftBid.preference < lmBid.preference)
}

@Test func mlxSwiftAdapterReadsMergedParams() {
    let payload: [String: HedosKernel.JSONValue] = [
        "temperature": .double(0.2),
        "top_p": .double(0.85),
        "max_tokens": .int(512),
    ]
    let params = MlxSwiftAdapter.params(from: payload)
    #expect(params.temperature == 0.2)
    #expect(params.topP == 0.85)
    #expect(params.maxTokens == 512)
}

@Test func mlxSwiftAdapterFallsBackWhenParamsAbsent() {
    let params = MlxSwiftAdapter.params(from: [:])
    #expect(params.temperature == 0.7)
    #expect(params.topP == nil)
    #expect(params.maxTokens == 2048)

    let zeroMax = MlxSwiftAdapter.params(from: ["max_tokens": .int(0)])
    #expect(zeroMax.maxTokens == 2048)
}

@Test func thinkSplitterClassifiesASingleCompleteBlockInOnePiece() {
    var splitter = ThinkSplitter()
    let pieces = splitter.feed("before <think>pondering</think> after")
    let flushed = splitter.flush()
    let all = pieces + flushed
    #expect(all == [
        .text("before "),
        .thinking("pondering"),
        .text(" after"),
    ])
}

@Test func thinkSplitterHandlesOpenTagSplitAcrossPieces() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    all += splitter.feed("hello <th")
    all += splitter.feed("ink>wondering</think> world")
    all += splitter.flush()
    #expect(all == [
        .text("hello "),
        .thinking("wondering"),
        .text(" world"),
    ])
}

@Test func thinkSplitterHandlesCloseTagSplitAcrossPieces() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    all += splitter.feed("<think>musing</th")
    all += splitter.feed("ink> done")
    all += splitter.flush()
    #expect(all == [
        .thinking("musing"),
        .text(" done"),
    ])
}

@Test func thinkSplitterHandlesThinkingContentSplitAcrossManyPieces() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    for chunk in ["<think>", "step one, ", "step two, ", "step three", "</think>", "answer"] {
        all += splitter.feed(chunk)
    }
    all += splitter.flush()
    let thinking = all.compactMap { piece -> String? in
        if case .thinking(let value) = piece { return value }
        return nil
    }.joined()
    let text = all.compactMap { piece -> String? in
        if case .text(let value) = piece { return value }
        return nil
    }.joined()
    #expect(thinking == "step one, step two, step three")
    #expect(text == "answer")
}

@Test func thinkSplitterPassesThroughPlainTextUnchanged() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    all += splitter.feed("no tags here, ")
    all += splitter.feed("just plain text.")
    all += splitter.flush()
    let joined = all.compactMap { piece -> String? in
        if case .text(let value) = piece { return value }
        return nil
    }.joined()
    #expect(joined == "no tags here, just plain text.")
    #expect(!all.contains { if case .thinking = $0 { return true } else { return false } })
}

@Test func thinkSplitterRoundTripsMultipleBlocksMinusTags() {
    var splitter = ThinkSplitter()
    let input = "a<think>b</think>c<think>d</think>e"
    var all: [ThinkSplitter.Piece] = []
    for character in input {
        all += splitter.feed(String(character))
    }
    all += splitter.flush()
    let roundTripped = all.map { piece -> String in
        switch piece {
        case .text(let value): value
        case .thinking(let value): value
        }
    }.joined()
    #expect(roundTripped == "abcde")
    #expect(all == [
        .text("a"),
        .thinking("b"),
        .text("c"),
        .thinking("d"),
        .text("e"),
    ])
}

@Test func thinkSplitterFlushEmitsUnterminatedThinkingBuffer() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    all += splitter.feed("before <think>never closes")
    all += splitter.flush()
    #expect(all == [
        .text("before "),
        .thinking("never closes"),
    ])
}

@Test func mlxKernelJSONConvertsTheLibraryShape() {
    let arguments: [String: MLXLMCommon.JSONValue] = [
        "zone": .string("UTC"),
        "count": .int(3),
        "nested": .object(["flag": .bool(true)]),
        "list": .array([.double(1.5), .null]),
    ]
    let converted = MlxSwiftEngine.kernelJSON(arguments)
    #expect(
        converted
            == .object([
                "zone": .string("UTC"),
                "count": .int(3),
                "nested": .object(["flag": .bool(true)]),
                "list": .array([.double(1.5), .null]),
            ]))
}

@Test func mlxSupportsToolsFollowsTheChatTemplateFact() {
    let adapter = MlxSwiftAdapter(
        governor: MemoryGovernor(totalMemoryMB: 262_144), engine: .shared)
    var record = Fixtures.gguf()
    record.hasChatTemplate = true
    #expect(adapter.supportsTools(record))
    record.hasChatTemplate = false
    #expect(!adapter.supportsTools(record))
    record.hasChatTemplate = nil
    #expect(!adapter.supportsTools(record))
}
