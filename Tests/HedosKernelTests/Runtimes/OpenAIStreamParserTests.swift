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

@Test func parserAccumulatesToolCallFragmentsAndYieldsOnFinish() {
    var parser = OpenAIStreamParser()
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-5","function":{"name":"get_","arguments":""}}]},"finish_reason":null}]}"#
    )
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"time","arguments":"{\"zo"}}]},"finish_reason":null}]}"#
    )
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ne\":\"UTC\"}"}}]},"finish_reason":null}]}"#
    )
    let flushed = parser.parse(
        line: #"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
    #expect(flushed.count == 1)
    guard case .toolCall(let call) = flushed[0] else {
        Issue.record("expected a tool call, got \(flushed)")
        return
    }
    #expect(call.id == "call-5")
    #expect(call.name == "get_time")
    #expect(call.arguments == .object(["zone": .string("UTC")]))

    let done = parser.parse(line: "data: [DONE]")
    guard case .done(let stats) = done.last else {
        Issue.record("expected done")
        return
    }
    #expect(stats?.finishReason == "tool_calls")
}

@Test func truncatedToolArgumentsArePreservedNotZeroed() {
    var parser = OpenAIStreamParser()
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write_file","arguments":"{\"path\": \"/im"}}]}}]}"#
    )
    let flushed = parser.parse(
        line: #"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
    guard case .toolCall(let call) = flushed.first else {
        Issue.record("expected a tool call")
        return
    }
    #expect(call.arguments == .object(["_raw": .string("{\"path\": \"/im")]))
}

@Test func emptyToolArgumentsStayAnEmptyObject() {
    var parser = OpenAIStreamParser()
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c2","function":{"name":"get_time","arguments":""}}]}}]}"#
    )
    let flushed = parser.parse(
        line: #"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
    guard case .toolCall(let call) = flushed.first else {
        Issue.record("expected a tool call")
        return
    }
    #expect(call.arguments == .object([:]))
}

@Test func parserReadsFinishReasonIntoStats() {
    var parser = OpenAIStreamParser()
    _ = parser.parse(
        line: #"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"length"}]}"#)
    let done = parser.parse(line: "data: [DONE]")
    guard case .done(let stats) = done.last else {
        Issue.record("expected done")
        return
    }
    #expect(stats?.finishReason == "length")
}
