import Foundation
import Testing

@testable import HedosKernel

private func runtimeRecord(_ runtimeID: String?, window: Int?) -> ModelRecord {
    var record = Fixtures.gguf()
    if let runtimeID {
        record.runtime = RuntimeRef(id: runtimeID, resolved: .auto, tier: .native)
    }
    record.contextLength = window
    return record
}

@Test func estimationDividesCharactersByFourRoundingUp() {
    #expect(ContextBudget.estimatedTokens(characters: 0) == 0)
    #expect(ContextBudget.estimatedTokens(characters: 1) == 1)
    #expect(ContextBudget.estimatedTokens(characters: 4) == 1)
    #expect(ContextBudget.estimatedTokens(characters: 5) == 2)
    #expect(ContextBudget.estimatedTokens(characters: 4000) == 1000)
}

@Test func effectiveWindowFollowsEachRuntime() {
    #expect(
        ContextBudget.effectiveWindow(for: runtimeRecord("llama-cpp", window: 131072)) == 32768)
    #expect(ContextBudget.effectiveWindow(for: runtimeRecord("llama-cpp", window: nil)) == 4096)
    #expect(ContextBudget.effectiveWindow(for: runtimeRecord("ollama", window: 8192)) == 8192)
    #expect(ContextBudget.effectiveWindow(for: runtimeRecord("ollama", window: nil)) == nil)
    #expect(
        ContextBudget.effectiveWindow(for: runtimeRecord("mlx-swift", window: 40960)) == 40960)
    #expect(
        ContextBudget.effectiveWindow(for: runtimeRecord("python:mlx-lm", window: 40960)) == 40960)
    #expect(
        ContextBudget.effectiveWindow(for: runtimeRecord("generic:openai-server", window: nil))
            == nil)

    var builtin = ModelRecord(
        name: "Apple Intelligence", modality: .text, capabilities: [.chat, .complete],
        source: ModelSource(kind: .builtin, path: "framework://FoundationModels"))
    builtin.runtime = RuntimeRef(id: "apple-foundation", resolved: .auto, tier: .native)
    #expect(ContextBudget.effectiveWindow(for: builtin) == 4096)
}

@Test func knobOverrideFeedsTheWindow() {
    let record = runtimeRecord("llama-cpp", window: 131072)
    #expect(
        ContextBudget.effectiveWindow(for: record, requestedContextLength: 65536) == 65536)

    let ollama = runtimeRecord("ollama", window: nil)
    #expect(
        ContextBudget.effectiveWindow(for: ollama, requestedContextLength: 8192) == 8192)
}

@Test func storedOverrideIsNormalizedAgainstItsSpec() {
    var record = runtimeRecord("llama-cpp", window: 8192)
    record.params = [
        ParamSpec(
            key: "context_length", type: .int,
            range: [.int(512), .int(8192)])
    ]
    record.paramValues["context_length"] = .int(65536)
    #expect(ContextBudget.storedContextLength(of: record) == 8192)

    record.params = []
    #expect(ContextBudget.storedContextLength(of: record) == nil)
}

@Test func nonPositiveWindowYieldsNoBudget() {
    #expect(ContextBudget.effectiveWindow(for: runtimeRecord("ollama", window: 0)) == nil)
    #expect(ContextBudget.effectiveWindow(for: runtimeRecord("mlx-swift", window: -5)) == nil)
    #expect(
        ContextBudget.effectiveWindow(
            for: runtimeRecord("ollama", window: nil), requestedContextLength: 0) == nil)
}

@Test func assessRefusesAtTheCompletionFloor() {
    let window = 1024
    let fitting = ContextBudget.assess(
        promptCharacters: (window - 256) * 4, window: window, requestedMaxTokens: nil)
    #expect(fitting == .fits(clampedMaxTokens: 256))

    let exceeding = ContextBudget.assess(
        promptCharacters: (window - 255) * 4, window: window, requestedMaxTokens: nil)
    #expect(exceeding == .exceeds(estimated: window - 255, window: window))
}

@Test func assessClampsMaxTokensNeverRaises() {
    let clamped = ContextBudget.assess(
        promptCharacters: 4000, window: 4096, requestedMaxTokens: 32768)
    #expect(clamped == .fits(clampedMaxTokens: 3096))

    let respected = ContextBudget.assess(
        promptCharacters: 4000, window: 4096, requestedMaxTokens: 128)
    #expect(respected == .fits(clampedMaxTokens: 128))
}

@Test func promptCharactersSumsMessagesAndPrompt() {
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("system"), "content": .string("abcd")]),
            .object(["role": .string("user"), "content": .string("efgh")]),
        ]),
        "prompt": .string("ijkl"),
    ])
    #expect(ContextBudget.promptCharacters(of: payload) == 12)
}
