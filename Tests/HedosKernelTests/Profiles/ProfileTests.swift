import Foundation
import Synchronization
import Testing

@testable import HedosKernel

private final class PayloadLog: Sendable {
    private let payloads = Mutex<[JSONValue]>([])

    func record(_ payload: JSONValue) {
        payloads.withLock { $0.append(payload) }
    }

    var all: [JSONValue] {
        payloads.withLock { $0 }
    }

    var lastFields: [String: JSONValue]? {
        guard case .object(let fields) = all.last else { return nil }
        return fields
    }
}

private struct CapturingAdapter: RuntimeAdapter {
    let log: PayloadLog

    var id: String { "fake:capture" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        log.record(payload)
        return AsyncThrowingStream { continuation in
            continuation.yield(.text("ok"))
            continuation.finish()
        }
    }
}

private struct BiddingChatAdapter: RuntimeAdapter {
    var id: String { "fake:llm" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .gguf else { return nil }
        return RuntimeBid(tier: .native, preference: 1)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}

private func capturedTextRecord(path: String = "~/Downloads/qwen3.5-9b-q4.gguf") -> ModelRecord {
    var record = Fixtures.gguf(path: path)
    record.runtime = RuntimeRef(id: "fake:capture", resolved: .auto, tier: .native)
    record.params = [
        ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)]),
        ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)]),
    ]
    record.state = .ready
    return record
}

private func speechRecord() -> ModelRecord {
    ModelRecord(
        name: "Kokoro-82M-bf16",
        modality: .speech,
        capabilities: [.speak],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/.cache/huggingface/hub/models--mlx-community--Kokoro-82M-bf16",
            repo: "mlx-community/Kokoro-82M-bf16"),
        runtime: RuntimeRef(id: "fake:capture", resolved: .auto, tier: .managed),
        execution: .stream,
        state: .ready)
}

@Test func profilesPopulateTextSchema() {
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: "fake:llm", resolved: .auto, tier: .native)
    let populated = ProfileRegistry.builtin.populated(record)
    let keys = populated.params.map(\.key)
    #expect(keys.contains("temperature"))
    #expect(keys.contains("top_p"))
    #expect(keys.contains("max_tokens"))
    #expect(keys.contains("context_length"))
    #expect(!keys.contains("thinking"))
    #expect(populated.params.allSatisfy { $0.defaultValue == nil })
}

@Test func profilesAddThinkingToggleWhereRuntimeSupportsIt() {
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: "ollama", resolved: .auto, tier: .native)
    let populated = ProfileRegistry.builtin.populated(record)
    let thinking = populated.params.first { $0.key == "thinking" }
    #expect(thinking?.type == .bool)
}

@Test func profilesPopulateSpeechSchema() {
    var record = speechRecord()
    record.params = []
    let populated = ProfileRegistry.builtin.populated(record)
    let keys = populated.params.map(\.key)
    #expect(keys == ["voice", "speed"])
    let speed = populated.params.first { $0.key == "speed" }
    #expect(speed?.defaultValue == .double(1.0))
    #expect(speed?.doubleRange == 0.5...2.0)
}

@Test func profilesLeaveExistingImageSchemaUntouched() {
    let record = Fixtures.flux()
    let populated = ProfileRegistry.builtin.populated(record)
    #expect(populated == record)
    #expect(populated.params.map(\.key) == ["steps", "guidance", "size", "seed"])
}

@Test func resolutionPopulatesSchemaWhenIdentificationDidNot() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    try await registry.register(Fixtures.gguf())

    try await ResolutionEngine(adapters: [BiddingChatAdapter()]).resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: Fixtures.gguf().id))
    #expect(resolved.runtime.id == "fake:llm")
    #expect(resolved.params.map(\.key).contains("temperature"))
    #expect(resolved.params.map(\.key).contains("context_length"))
}

@Test func overridesAndSystemPromptReachTheChatPayload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = PayloadLog()
    let kernel = Kernel(directory: dir, adapters: [CapturingAdapter(log: log)])
    let record = capturedTextRecord()
    try await kernel.registry.register(record)

    try await kernel.setParamValue(record.id, key: "temperature", to: .double(0.7))
    try await kernel.setSystemPrompt(record.id, to: "Be brief.")

    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "hi")])
    for try await _ in stream {}

    let fields = try #require(log.lastFields)
    #expect(fields["temperature"] == .double(0.7))
    guard case .array(let turns)? = fields["messages"] else {
        Issue.record("expected messages array, got \(String(describing: fields["messages"]))")
        return
    }
    #expect(turns.count == 2)
    #expect(
        turns[0]
            == .object(["role": .string("system"), "content": .string("Be brief.")]))
    #expect(turns[1] == .object(["role": .string("user"), "content": .string("hi")]))
}

@Test func unconfiguredModelPayloadCarriesNothingSynthesized() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = PayloadLog()
    let kernel = Kernel(directory: dir, adapters: [CapturingAdapter(log: log)])
    let record = capturedTextRecord(path: "~/Downloads/other-model.gguf")
    try await kernel.registry.register(record)

    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "hi")])
    for try await _ in stream {}

    let fields = try #require(log.lastFields)
    #expect(fields.keys.sorted() == ["messages"])
    guard case .array(let turns)? = fields["messages"] else {
        Issue.record("expected messages array")
        return
    }
    #expect(turns.count == 1)
}

@Test func explicitPayloadValuesWinOverStoredOverrides() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = PayloadLog()
    let kernel = Kernel(directory: dir, adapters: [CapturingAdapter(log: log)])
    let record = capturedTextRecord()
    try await kernel.registry.register(record)
    try await kernel.setParamValue(record.id, key: "temperature", to: .double(0.7))
    try await kernel.setParamValue(record.id, key: "top_p", to: .double(0.9))

    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])]),
        "temperature": .double(0.2),
    ])
    let stream = try await kernel.invoke(record.id, .chat, payload: payload)
    for try await _ in stream {}

    let fields = try #require(log.lastFields)
    #expect(fields["temperature"] == .double(0.2))
    #expect(fields["top_p"] == .double(0.9))
}

@Test func speakOverridesMergeIntoInvokePayload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = PayloadLog()
    let kernel = Kernel(directory: dir, adapters: [CapturingAdapter(log: log)])
    var record = speechRecord()
    record.params = ProfileRegistry.builtin.schema(for: record)
    try await kernel.registry.register(record)
    try await kernel.setParamValue(record.id, key: "voice", to: .string("bf_alpha"))
    try await kernel.setParamValue(record.id, key: "speed", to: .double(1.3))

    let stream = try await kernel.invoke(
        record.id, .speak, payload: .object(["text": .string("hello")]))
    for try await _ in stream {}

    let fields = try #require(log.lastFields)
    #expect(fields["text"] == .string("hello"))
    #expect(fields["voice"] == .string("bf_alpha"))
    #expect(fields["speed"] == .double(1.3))
}

@Test func vanishedAndInvalidOverridesAreDroppedOnRegistryLoad() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    var record = capturedTextRecord()
    record.paramValues = [
        "temperature": .double(0.5),
        "vanished": .int(3),
        "top_p": .string("high"),
    ]
    try await Registry(directory: dir).register(record)

    let reloaded = try #require(try await Registry(directory: dir).get(id: record.id))
    #expect(reloaded.paramValues == ["temperature": .double(0.5)])
}

@Test func paramValueSettingValidatesClampsAndClears() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    let record = capturedTextRecord()
    try await kernel.registry.register(record)

    await #expect(throws: KernelError.self) {
        try await kernel.setParamValue(record.id, key: "steps", to: .int(8))
    }

    try await kernel.setParamValue(record.id, key: "temperature", to: .double(5))
    var stored = try #require(try await kernel.registry.get(id: record.id))
    #expect(stored.paramValues["temperature"] == .double(2))

    try await kernel.setParamValue(record.id, key: "temperature", to: nil)
    stored = try #require(try await kernel.registry.get(id: record.id))
    #expect(stored.paramValues.isEmpty)

    try await kernel.setParamValue(record.id, key: "temperature", to: .double(0.4))
    try await kernel.setParamValue(record.id, key: "top_p", to: .double(0.8))
    try await kernel.resetParamValues(record.id)
    stored = try #require(try await kernel.registry.get(id: record.id))
    #expect(stored.paramValues.isEmpty)
}

@Test func aliasAndSystemPromptPersistAndTrim() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    let record = capturedTextRecord()
    try await kernel.registry.register(record)

    try await kernel.setAlias(record.id, to: "  Qwen  ")
    try await kernel.setSystemPrompt(record.id, to: "Answer in haiku.")
    var stored = try #require(try await kernel.registry.get(id: record.id))
    #expect(stored.alias == "Qwen")
    #expect(stored.displayName == "Qwen")
    #expect(stored.systemPrompt == "Answer in haiku.")

    let reloaded = try #require(try await Registry(directory: dir).get(id: record.id))
    #expect(reloaded.alias == "Qwen")
    #expect(reloaded.systemPrompt == "Answer in haiku.")

    try await kernel.setAlias(record.id, to: "   ")
    try await kernel.setSystemPrompt(record.id, to: nil)
    stored = try #require(try await kernel.registry.get(id: record.id))
    #expect(stored.alias == nil)
    #expect(stored.displayName == stored.name)
    #expect(stored.systemPrompt == nil)
}

@Test func recordsWithoutConfigFieldsStillDecode() throws {
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    let data = try encoder.encode(Fixtures.flux())
    var object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    object.removeValue(forKey: "paramValues")
    object.removeValue(forKey: "systemPrompt")
    object.removeValue(forKey: "alias")
    let stripped = try JSONSerialization.data(withJSONObject: object)

    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601
    let decoded = try decoder.decode(ModelRecord.self, from: stripped)
    #expect(decoded.paramValues.isEmpty)
    #expect(decoded.systemPrompt == nil)
    #expect(decoded.alias == nil)
    #expect(decoded.name == "FLUX.1-schnell")
}

@Test func ollamaBodyFoldsSamplingOverridesIntoOptions() throws {
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])]),
        "temperature": .double(0.7),
        "max_tokens": .int(256),
        "context_length": .int(8192),
        "thinking": .bool(true),
    ])
    let data = try OllamaAdapter.requestBody(model: "m", payload: payload)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let options = try #require(body["options"] as? [String: Any])
    #expect(options["temperature"] as? Double == 0.7)
    #expect(options["num_predict"] as? Int == 256)
    #expect(options["num_ctx"] as? Int == 8192)
    #expect(body["think"] as? Bool == true)
    #expect(body["temperature"] == nil)
}

@Test func globalDefaultPromptFallsBackWhenModelHasNone() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = PayloadLog()
    let kernel = Kernel(directory: dir, adapters: [CapturingAdapter(log: log)])
    let record = capturedTextRecord()
    try await kernel.registry.register(record)

    var chat = await kernel.chatSettings()
    chat.defaultSystemPrompt = "Answer in one sentence."
    try await kernel.updateChatSettings(chat)

    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "hi")])
    for try await _ in stream {}
    var fields = try #require(log.lastFields)
    guard case .array(let turns)? = fields["messages"] else {
        Issue.record("expected messages array")
        return
    }
    #expect(
        turns.first
            == .object([
                "role": .string("system"), "content": .string("Answer in one sentence."),
            ]))

    try await kernel.setSystemPrompt(record.id, to: "Be brief.")
    let second = try await kernel.chat(
        record.id, messages: [.init(role: .user, content: "hi again")])
    for try await _ in second {}
    fields = try #require(log.lastFields)
    guard case .array(let secondTurns)? = fields["messages"] else {
        Issue.record("expected messages array")
        return
    }
    #expect(
        secondTurns.first
            == .object(["role": .string("system"), "content": .string("Be brief.")]))
}
