import Testing

@testable import HedosKernel

private func chatRecord(runtime: RuntimeID? = nil) -> ModelRecord {
    var record = Fixtures.gguf()
    record.capabilities = [.chat, .complete]
    record.contextLength = 8192
    record.runtime.id = runtime
    return record
}

@Test func adaptersDeclareTheirActualHonoredKeys() {
    let record = chatRecord()
    #expect(
        LlamaCppAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "context_length",
            "repeat_penalty", "frequency_penalty", "presence_penalty", "seed", "stop",
            "response_format",
        ])
    #expect(
        MlxSwiftAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "top_p", "max_tokens", "repeat_penalty", "stop",
        ])
    #expect(
        MlxLmAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "repeat_penalty",
            "seed", "stop",
        ])
    #expect(
        OllamaAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "context_length",
            "stop", "seed", "repeat_penalty", "frequency_penalty", "presence_penalty",
            "response_format",
        ])
    #expect(
        OpenAIEndpointAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "top_p", "max_tokens", "stop", "seed", "frequency_penalty",
            "presence_penalty", "response_format",
        ])
    #expect(
        AppleFoundationAdapter().honoredParamKeys(record, .chat) == [
            "temperature", "max_tokens", "top_p", "top_k", "seed",
        ])
    #expect(LlamaCppAdapter().honoredParamKeys(record, .embed).isEmpty)
}

@Test func profileSchemaIsSubsetOfRuntimeHonoredKeys() {
    let cases: [(RuntimeID, any RuntimeAdapter)] = [
        (.llamaCpp, LlamaCppAdapter()),
        (.mlxSwift, MlxSwiftAdapter()),
        (.mlxLm, MlxLmAdapter()),
        (.ollama, OllamaAdapter()),
        (.openAIEndpoint, OpenAIEndpointAdapter()),
        (.appleFoundation, AppleFoundationAdapter()),
    ]
    for (runtime, adapter) in cases {
        let record = chatRecord(runtime: runtime)
        let schemaKeys = Set(ProfileRegistry.builtin.schema(for: record).map(\.key))
            .subtracting(GatewayParamGuard.structuralKeys)
        let honored = adapter.honoredParamKeys(record, .chat)
        #expect(
            schemaKeys.isSubset(of: honored),
            "\(runtime.rawValue) schema leaks: \(schemaKeys.subtracting(honored))")
    }
}
