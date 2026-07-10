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

private func windowedRecord(_ window: Int?) -> ModelRecord {
    var record = Fixtures.gguf()
    record.contextLength = window
    return record
}

@Test func effectiveContextDefaultsToWindowCappedAt32k() {
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(131072), requested: nil)
            == 32768)
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(8192), requested: nil)
            == 8192)
}

@Test func requestedContextClampsToTrainedWindow() {
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(8192), requested: 100000)
            == 8192)
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(8192), requested: 256)
            == 512)
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(131072), requested: 131072)
            == 131072)
}

@Test func windowlessRecordKeeps4096() {
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(nil), requested: nil)
            == 4096)
    #expect(
        LlamaCppAdapter.effectiveContextTokens(record: windowedRecord(nil), requested: 32768)
            == 4096)
}
