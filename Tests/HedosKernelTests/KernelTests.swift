import Foundation
import Testing

@testable import HedosKernel

@Test func kernelIsConstructibleAndVersioned() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    _ = Kernel(directory: dir)
}

@Test func discoverScansUserConfiguredHFRootAndResolvesMlxSwift() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let customRoot = dir.appendingPathComponent("custom-hf")
    try DiscoveryFixtures.makeHFRepo(
        at: customRoot.appendingPathComponent("hub"),
        DiscoveryFixtures.HFRepo(
            org: "mlx-community", repo: "Tiny-Chat-4bit",
            files: [("model.safetensors", 512), ("tokenizer.json", 64)],
            configJSON:
                #"{"architectures": ["LlamaForCausalLM"], "model_type": "llama", "quantization": {"bits": 4}}"#
        ))

    let kernel = Kernel(directory: dir.appendingPathComponent("support"))
    try await kernel.settings.addHFCacheRoot(customRoot.path)
    _ = try await kernel.discover()

    let records = try await kernel.shelf()
    let record = try #require(records.first { $0.name.contains("Tiny-Chat-4bit") })
    #expect(record.source.kind == .huggingfaceCache)
    #expect(record.runtime.id == "mlx-swift")
    #expect(record.runtime.tier == .native)
    #expect(record.runtime.alternatives.contains("python:mlx-lm"))
    #expect(record.state == .ready)
}

private final class BudgetPayloadBox: @unchecked Sendable {
    private let lock = NSLock()
    private var payloads: [JSONValue] = []

    func record(_ payload: JSONValue) {
        lock.withLock { payloads.append(payload) }
    }

    var lastMaxTokens: Int? {
        lock.withLock {
            guard case .object(let fields)? = payloads.last else { return nil }
            return fields["max_tokens"]?.intValue
        }
    }
}

private struct BudgetCapturingAdapter: RuntimeAdapter {
    let box: BudgetPayloadBox

    var id: RuntimeID { "ollama" }

    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        requested ?? record.contextLength
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        capability == .chat
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        box.record(payload)
        return AsyncThrowingStream { continuation in
            continuation.yield(.text("ok"))
            continuation.finish()
        }
    }
}

private func smallWindowRecord() -> ModelRecord {
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: "ollama", resolved: .user, tier: .native)
    record.contextLength = 512
    record.state = .ready
    return record
}

@Test func oversizedChatIsRefusedWithContextExceeded() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let box = BudgetPayloadBox()
    let kernel = Kernel(
        directory: dir, adapters: [BudgetCapturingAdapter(box: box)],
        secrets: InMemorySecretStore())
    let record = smallWindowRecord()
    try await kernel.registry.register(record)

    let oversized = String(repeating: "a", count: 4000)
    do {
        _ = try await kernel.chat(
            record.id, messages: [ChatMessage(role: .user, content: oversized)])
        Issue.record("an oversized chat must refuse before dispatch")
    } catch let KernelError.contextExceeded(model) {
        #expect(model == record.name)
    }
    #expect(box.lastMaxTokens == nil)
}

@Test func fittingChatClampsMaxTokens() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let box = BudgetPayloadBox()
    let kernel = Kernel(
        directory: dir, adapters: [BudgetCapturingAdapter(box: box)],
        secrets: InMemorySecretStore())
    let record = smallWindowRecord()
    try await kernel.registry.register(record)

    let fitting = String(repeating: "a", count: 400)
    let stream = try await kernel.chat(
        record.id, messages: [ChatMessage(role: .user, content: fitting)])
    for try await _ in stream {}

    let clamped = try #require(box.lastMaxTokens)
    #expect(clamped <= 512 - 100)
    #expect(clamped >= 256)
}

@Test func scopedRescanWithNoChangesWritesNothing() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try DiscoveryFixtures.makeOllamaStore(
        at: home.appendingPathComponent(".ollama/models"),
        tags: [.init(model: "gemma4", tag: "latest", modelBytes: 64)])
    let habitat = ModelHabitat(home: home, environment: ["HF_HOME": "", "HF_HUB_CACHE": ""])
    let kernel = Kernel(
        directory: dir, governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore(), habitat: habitat)
    _ = try await kernel.discover()
    let baseline = await kernel.registry.saveCount

    await kernel.scopedRescan([.ollama])

    #expect(await kernel.registry.saveCount == baseline)
}

@Test func registerEndpointCreatesReadyUserPinnedRecord() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(
        directory: dir, governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore())

    let record = try await kernel.registerEndpoint(
        baseURL: "127.0.0.1:11434/v1", model: "gemma4:latest")
    #expect(record.source.kind == .endpoint)
    #expect(record.source.path == "http://127.0.0.1:11434")
    #expect(record.source.repo == "gemma4:latest")
    #expect(record.runtime.id == "generic:openai-server")
    #expect(record.runtime.resolved == .user)
    #expect(record.runtime.tier == .remote)
    #expect(record.state == .ready)
    #expect(record.params.contains { $0.key == "temperature" })
    #expect(!record.params.contains { $0.key == "context_length" })

    let identified = Identification.identify(record)
    #expect(identified.format == .endpoint)
    #expect(identified.capabilities == [.chat, .complete])

    let again = try await kernel.registerEndpoint(
        baseURL: "http://127.0.0.1:11434", model: "gemma4:latest")
    #expect(again.id == record.id)
}

@Test func removeEndpointDeletesKeyOnlyWhenLastRecordForServer() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = InMemorySecretStore()
    let kernel = Kernel(
        directory: dir, governor: MemoryGovernor(totalMemoryMB: 262_144), secrets: secrets)
    try secrets.set("sk-live", account: "http://127.0.0.1:9999")

    let first = try await kernel.registerEndpoint(
        baseURL: "http://127.0.0.1:9999", model: "alpha")
    let second = try await kernel.registerEndpoint(
        baseURL: "http://127.0.0.1:9999", model: "beta")

    try await kernel.removeEndpoint(first.id)
    #expect(try secrets.get(account: "http://127.0.0.1:9999") == "sk-live")

    try await kernel.removeEndpoint(second.id)
    #expect(try secrets.get(account: "http://127.0.0.1:9999") == nil)
}

@Test func endpointRecordsSurviveDiscoverUntouched() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(
        directory: dir, governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore())
    let record = try await kernel.registerEndpoint(
        baseURL: "http://127.0.0.1:9999", model: "alpha")

    _ = try await kernel.discover()

    let after = try #require(try await kernel.registry.get(id: record.id))
    #expect(after.state == .ready)
    #expect(after.runtime.id == "generic:openai-server")

    let explanations = try await kernel.explainShelf()
    let explanation = try #require(explanations.first { $0.record.id == record.id })
    #expect(explanation.winner == "generic:openai-server")
    let line = ShelfReport.render([explanation])
    #expect(line.contains("user-pinned"))
}

@Test func startOllamaWithoutARegisteredAdapterThrows() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(
        directory: dir, adapters: [], governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore())

    await #expect(throws: KernelError.self) {
        try await kernel.startOllama()
    }
}
