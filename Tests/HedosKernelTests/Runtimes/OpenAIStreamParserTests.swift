import Foundation
import Testing

@testable import HedosKernel

@Test func parsesDeltaContent() {
    var parser = OpenAIStreamParser()
    let chunks = parser.parse(
        line: #"data: {"choices": [{"delta": {"content": "Hello"}}]}"#)
    #expect(chunks == [.text("Hello")])
}

@Test func parsesReasoningContentAsThinking() {
    var parser = OpenAIStreamParser()
    let chunks = parser.parse(
        line: #"data: {"choices": [{"delta": {"reasoning_content": "hmm"}}]}"#)
    #expect(chunks == [.thinking("hmm")])
}

@Test func doneEmitsStatsOnLiteralDONE() {
    var parser = OpenAIStreamParser()
    _ = parser.parse(
        line: #"data: {"choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 4}}"#)
    let chunks = parser.parse(line: "data: [DONE]")
    guard case .done(let stats)? = chunks.first else {
        Issue.record("expected a done chunk")
        return
    }
    #expect(stats?.promptTokens == 7)
    #expect(stats?.completionTokens == 4)
    #expect(stats?.durationMs != nil)
}

@Test func blankAndNonDataLinesIgnored() {
    var parser = OpenAIStreamParser()
    #expect(parser.parse(line: "").isEmpty)
    #expect(parser.parse(line: "event: ping").isEmpty)
    #expect(parser.parse(line: ": comment").isEmpty)
}

@Test func malformedJSONSkippedWithoutThrow() {
    var parser = OpenAIStreamParser()
    #expect(parser.parse(line: "data: {broken json").isEmpty)
}

@Test func emptyDeltaProducesNoChunks() {
    var parser = OpenAIStreamParser()
    #expect(
        parser.parse(line: #"data: {"choices": [{"delta": {"role": "assistant"}}]}"#).isEmpty)
    #expect(parser.parse(line: #"data: {"choices": [{"delta": {"content": ""}}]}"#).isEmpty)
}
