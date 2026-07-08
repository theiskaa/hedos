import Foundation
import Testing

@testable import HedosKernel

@Test func kernelIsConstructibleAndVersioned() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    _ = Kernel(directory: dir)
    #expect(!Kernel.version.isEmpty)
}

@Test func discoverScansUserConfiguredHFRootAndResolvesMlxLm() async throws {
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
    try await kernel.addHFCacheRoot(customRoot.path)
    _ = try await kernel.discover()

    let records = try await kernel.shelf()
    let record = try #require(records.first { $0.name.contains("Tiny-Chat-4bit") })
    #expect(record.source.kind == .huggingfaceCache)
    #expect(record.runtime.id == "python:mlx-lm")
    #expect(record.runtime.tier == .managed)
    #expect(record.state == .ready)
}

@Test func registerEndpointCreatesReadyUserPinnedRecord() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())

    let record = try await kernel.registerEndpoint(
        baseURL: "127.0.0.1:11434/v1", model: "gemma4:latest")
    #expect(record.source.kind == .endpoint)
    #expect(record.source.path == "http://127.0.0.1:11434")
    #expect(record.source.repo == "gemma4:latest")
    #expect(record.runtime.id == "generic:openai-server")
    #expect(record.runtime.resolved == .user)
    #expect(record.runtime.tier == .native)
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
    let kernel = Kernel(directory: dir, secrets: secrets)
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
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())
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
