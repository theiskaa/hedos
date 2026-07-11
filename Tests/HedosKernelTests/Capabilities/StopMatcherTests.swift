import Testing

@testable import HedosKernel

@Test func stopMatcherPassesThroughWhenInactive() {
    var matcher = StopMatcher([])
    #expect(matcher.feed("hello world") == "hello world")
    #expect(!matcher.stopped)
    #expect(matcher.flush() == "")
}

@Test func stopMatcherTruncatesBeforeMatchWithinOneChunk() {
    var matcher = StopMatcher(["END"])
    #expect(matcher.feed("keep going END and more") == "keep going ")
    #expect(matcher.stopped)
    #expect(matcher.feed("ignored") == "")
    #expect(matcher.flush() == "")
}

@Test func stopMatcherHoldsBackPartialSuffixAcrossChunks() {
    var matcher = StopMatcher(["\n\n"])
    #expect(matcher.feed("line one\n") == "line one")
    #expect(!matcher.stopped)
    #expect(matcher.feed("\n") == "")
    #expect(matcher.stopped)
}

@Test func stopMatcherEmitsHeldSuffixWhenNoMatchArrives() {
    var matcher = StopMatcher(["STOP"])
    #expect(matcher.feed("almost ST") == "almost ")
    #expect(matcher.feed("ay") == "STay")
    #expect(!matcher.stopped)
    #expect(matcher.flush() == "")
}

@Test func stopMatcherFlushesTailWhenStreamEndsMidHold() {
    var matcher = StopMatcher(["STOP"])
    #expect(matcher.feed("done ST") == "done ")
    #expect(matcher.flush() == "ST")
}

@Test func stopMatcherStringsDecodeFromWireShapes() {
    #expect(StopMatcher.strings(from: .string("x")) == ["x"])
    #expect(StopMatcher.strings(from: .array([.string("a"), .int(3), .string("b")])) == ["a", "b"])
    #expect(StopMatcher.strings(from: nil) == [])
}
