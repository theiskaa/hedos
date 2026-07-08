import Foundation
import Testing

@testable import HedosKernel

@Test func llamaAdapterReadsMergedParams() {
    let payload: [String: JSONValue] = [
        "temperature": .double(0.2),
        "top_p": .double(0.85),
        "max_tokens": .int(512),
    ]
    let params = LlamaCppAdapter.params(from: payload)
    #expect(params.temperature == 0.2)
    #expect(params.topP == 0.85)
    #expect(params.maxTokens == 512)
}

@Test func llamaAdapterFallsBackWhenParamsAbsent() {
    let params = LlamaCppAdapter.params(from: [:])
    #expect(params.temperature == 0.7)
    #expect(params.topP == nil)
    #expect(params.maxTokens == 2048)

    let zeroMax = LlamaCppAdapter.params(from: ["max_tokens": .int(0)])
    #expect(zeroMax.maxTokens == 2048)
}
