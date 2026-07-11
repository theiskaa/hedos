import Testing

@testable import HedosKernel

@Test func thinkSplitterSeparatesStartThinkingPair() {
    var splitter = ThinkSplitter()
    let pieces =
        splitter.feed("hi <|START_THINKING|>planning<|END_THINKING|> done") + splitter.flush()
    #expect(
        pieces == [
            .text("hi "),
            .thinking("planning"),
            .text(" done"),
        ])
}

@Test func thinkSplitterPicksTheEarliestOpenTagAmongPairs() {
    var splitter = ThinkSplitter()
    let pieces = splitter.feed("a <think>x</think> b <|START_THINKING|>y<|END_THINKING|> c")
        + splitter.flush()
    #expect(
        pieces == [
            .text("a "), .thinking("x"), .text(" b "), .thinking("y"), .text(" c"),
        ])
}

@Test func thinkSplitterHoldsBackAmbiguousSuffixForLongerTag() {
    var splitter = ThinkSplitter()
    var all: [ThinkSplitter.Piece] = []
    all += splitter.feed("keep <|START_")
    all += splitter.feed("THINKING|>secret<|END_THINKING|>")
    all += splitter.flush()
    #expect(all == [.text("keep "), .thinking("secret")])
}

@Test func thinkSplitterUnterminatedBlockFlushesAsThinking() {
    var splitter = ThinkSplitter()
    let pieces = splitter.feed("visible <think>still going") + splitter.flush()
    #expect(pieces == [.text("visible "), .thinking("still going")])
}

@Test func separatingStreamRoutesTextThroughTheSplitter() async throws {
    let upstream = AsyncThrowingStream<CapabilityChunk, Error> { continuation in
        continuation.yield(.text("visible <think>hidden"))
        continuation.yield(.text("</think> more"))
        continuation.yield(.done(GenerationStats()))
        continuation.finish()
    }
    var texts: [String] = []
    var thoughts: [String] = []
    var sawDone = false
    for try await chunk in ThinkSplitter.separating(upstream) {
        switch chunk {
        case .text(let value): texts.append(value)
        case .thinking(let value): thoughts.append(value)
        case .done: sawDone = true
        default: break
        }
    }
    #expect(texts.joined() == "visible  more")
    #expect(thoughts.joined() == "hidden")
    #expect(sawDone)
}

@Test func separatingStreamIsIdempotentWhenAlreadySplit() async throws {
    let upstream = AsyncThrowingStream<CapabilityChunk, Error> { continuation in
        continuation.yield(.thinking("hidden"))
        continuation.yield(.text("visible answer"))
        continuation.yield(.done(GenerationStats()))
        continuation.finish()
    }
    var texts: [String] = []
    var thoughts: [String] = []
    for try await chunk in ThinkSplitter.separating(upstream) {
        switch chunk {
        case .text(let value): texts.append(value)
        case .thinking(let value): thoughts.append(value)
        default: break
        }
    }
    #expect(texts.joined() == "visible answer")
    #expect(thoughts.joined() == "hidden")
}

@Test func hasVisibleTagsDetectsEverySupportedPair() {
    #expect(ThinkSplitter.hasVisibleTags(in: "leaked <think> here"))
    #expect(ThinkSplitter.hasVisibleTags(in: "leaked <|START_THINKING|> here"))
    #expect(ThinkSplitter.hasVisibleTags(in: "closes </think>"))
    #expect(ThinkSplitter.hasVisibleTags(in: "ends <|END_THINKING|>"))
    #expect(!ThinkSplitter.hasVisibleTags(in: "clean visible answer"))
}

@Test func thinkSplitterKeepsGraphemesWholeAcrossABoundary() {
    var splitter = ThinkSplitter()
    let pieces = splitter.feed("hi 👋<think>🤔</think>bye 🎉") + splitter.flush()
    #expect(
        pieces == [
            .text("hi 👋"), .thinking("🤔"), .text("bye 🎉"),
        ])
}
