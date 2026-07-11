import Foundation
import Testing

@testable import HedosKernel

private func ggufRecord(at dir: URL, name: String = "tiny") throws -> ModelRecord {
    let path = dir.appendingPathComponent("\(name).gguf")
    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 64))
    try payload.write(to: path)
    return ModelRecord(
        name: name,
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(kind: .file, path: path.path),
        execution: .stream,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func identifiesGGUFByMagicWithoutExtension() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let path = dir.appendingPathComponent("mystery.bin")
    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 32))
    try payload.write(to: path)

    let record = ModelRecord(
        name: "mystery", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .file, path: path.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .gguf)
    #expect(identified.modality == .text)
}

@Test func identifiesOllamaByStoreMembership() {
    let record = ModelRecord(
        name: "qwen3.5:9b", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .ollama, path: "/fake/manifest", repo: "qwen3.5:9b"))
    #expect(Identification.identify(record).format == .ollamaStore)
}

@Test func identifiesSafetensorsHeaderVariants() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    func writeSafetensors(name: String, metadataFormat: String?) throws -> URL {
        var header: [String: Any] = ["fake_tensor": ["dtype": "F16", "shape": [1], "data_offsets": [0, 2]]]
        if let metadataFormat { header["__metadata__"] = ["format": metadataFormat] }
        let json = try JSONSerialization.data(withJSONObject: header)
        var blob = Data()
        var length = UInt64(json.count).littleEndian
        blob.append(Data(bytes: &length, count: 8))
        blob.append(json)
        blob.append(DiscoveryFixtures.data(bytes: 2))
        let url = dir.appendingPathComponent(name)
        try blob.write(to: url)
        return url
    }

    let mlx = try writeSafetensors(name: "mlx.safetensors", metadataFormat: "mlx")
    #expect(Identification.safetensorsHeaderMetadataFormat(at: mlx) == "mlx")
    let plain = try writeSafetensors(name: "plain.safetensors", metadataFormat: nil)
    #expect(Identification.safetensorsHeaderMetadataFormat(at: plain) == nil)

    let bundle = dir.appendingPathComponent("bundle")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(DiscoveryFixtures.causalLMConfig.utf8)
        .write(to: bundle.appendingPathComponent("config.json"))
    _ = try FileManager.default.copyItem(
        at: mlx, to: bundle.appendingPathComponent("weights.safetensors"))
    let record = ModelRecord(
        name: "bundle", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .mlxSafetensors)
    #expect(identified.modality == .text)
}

@Test func identifiesQuantizationBlockAsMLX() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bundle = dir.appendingPathComponent("quant")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(#"{"architectures":["Qwen3ForCausalLM"],"quantization":{"bits":4}}"#.utf8)
        .write(to: bundle.appendingPathComponent("config.json"))
    try DiscoveryFixtures.data(bytes: 32)
        .write(to: bundle.appendingPathComponent("weights.safetensors"))

    let record = ModelRecord(
        name: "quant", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    #expect(Identification.identify(record).format == .mlxSafetensors)
}

@Test func unrecognizableFolderIdentifiesAsUnknownWithoutError() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let mystery = dir.appendingPathComponent("mystery-model")
    try FileManager.default.createDirectory(at: mystery, withIntermediateDirectories: true)
    try DiscoveryFixtures.data(bytes: 128).write(to: mystery.appendingPathComponent("blob.bin"))

    let record = ModelRecord(
        name: "mystery-model", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: mystery.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .unknown)
    #expect(identified.modality == nil)
}

@Test func engineResolvesGGUFToLlamaCppNative() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = try ggufRecord(at: dir)
    try await registry.register(record)

    let engine = ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "llama-cpp")
    #expect(resolved.runtime.tier == .native)
    #expect(resolved.runtime.resolved == .auto)
    #expect(resolved.runtime.confirmedAt == nil)
    #expect(resolved.state == .ready)
    #expect(resolved.runtime.alternatives.isEmpty)
}

@Test func engineResolvesOllamaRecordToOllamaNative() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = ModelRecord(
        name: "gemma4:latest", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .ollama, path: "/fake", repo: "gemma4:latest"))
    try await registry.register(record)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "ollama")
    #expect(resolved.runtime.tier == .native)
    #expect(resolved.state == .ready)
}

@Test func engineMarksUnservableModelsRecipeNeeded() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let kokoro = Fixtures.flux()
    try await registry.register(kokoro)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: kokoro.id))
    #expect(resolved.runtime.tier == .recipeNeeded)
    #expect(resolved.runtime.id == nil)
    #expect(resolved.state == .unresolved)
}

@Test func ollamaEmbedRecordSurvivesResolveWithoutChatRegression() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = dir.appendingPathComponent("ollama")
    try DiscoveryFixtures.makeOllamaStore(
        at: store,
        tags: [
            .init(model: "nomic-embed-text", tag: "latest", modelBytes: 512, ggufArchitecture: "nomic-bert")
        ])
    let registry = Registry(directory: dir.appendingPathComponent("appsupport"))
    _ = try await DiscoveryService(
        scanners: [OllamaStoreScanner(root: store)], duplicateThreshold: 1024
    ).discover(into: registry)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let resolved = try #require(try await registry.list().first)
    #expect(resolved.capabilities == [.embed])
    #expect(resolved.modality == .embedding)
    #expect(resolved.runtime.id == "ollama")
    #expect(resolved.state == .ready)
}

@Test func mlxSafetensorsWhisperKeepsTranscribeAndLandsRecipeNeeded() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bundle = dir.appendingPathComponent("whisper-large-v3-mlx")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(DiscoveryFixtures.mlxWhisperConfig.utf8)
        .write(to: bundle.appendingPathComponent("config.json"))
    try DiscoveryFixtures.data(bytes: 256).write(
        to: bundle.appendingPathComponent("weights.safetensors"))

    let record = ModelRecord(
        name: "whisper-large-v3-mlx", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .mlxSafetensors)
    #expect(identified.modality == .audio)
    #expect(identified.capabilities == [.transcribe])

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(record)
    try await ResolutionEngine(
        adapters: [
            LlamaCppAdapter(), WhisperCppAdapter(), MlxSwiftAdapter(), OllamaAdapter(),
            MlxAudioAdapter(),
        ]
    ).resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.capabilities == [.transcribe])
    #expect(resolved.modality == .audio)
    #expect(resolved.runtime.tier == .recipeNeeded)
    #expect(resolved.runtime.id == nil)
    #expect(resolved.state == .unresolved)
}

@Test func sentenceTransformersLayoutIdentifiesAsEmbed() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bundle = dir.appendingPathComponent("all-MiniLM-L6-v2")
    try FileManager.default.createDirectory(
        at: bundle.appendingPathComponent("1_Pooling"), withIntermediateDirectories: true)
    try Data(#"{"architectures": ["SomeEncoderModel"]}"#.utf8)
        .write(to: bundle.appendingPathComponent("config.json"))
    try DiscoveryFixtures.data(bytes: 128).write(
        to: bundle.appendingPathComponent("model.safetensors"))

    let record = ModelRecord(
        name: "all-MiniLM-L6-v2", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    let identified = Identification.identify(record)
    #expect(identified.modality == .embedding)
    #expect(identified.capabilities == [.embed])
    #expect(identified.execution == .stream)
}

@Test func resolveCopiesGGUFContextLengthOntoRecord() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let url = dir.appendingPathComponent("windowed.gguf")
    var builder = GGUFFixtureBuilder(keyValueCount: 3)
    builder.addString(key: "general.architecture", value: "llama")
    builder.addUInt32(key: "llama.context_length", value: 32768)
    builder.addString(key: "tokenizer.chat_template", value: "{{ messages }}")
    try builder.write(to: url)
    let record = ModelRecord(
        name: "windowed", modality: .text, capabilities: [.chat, .complete],
        source: ModelSource(kind: .file, path: url.path), execution: .stream)
    try await registry.register(record)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.contextLength == 32768)
    #expect(resolved.hasChatTemplate == true)
    #expect(resolved.state == .ready)
}

@Test func resolveAllScopedToKindsLeavesOtherKindsUntouched() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let gguf = try ggufRecord(at: dir)
    let ollama = ModelRecord(
        name: "gemma4:latest", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .ollama, path: "/fake", repo: "gemma4:latest"))
    try await registry.register(gguf)
    try await registry.register(ollama)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry, kinds: [.file])

    let resolvedGGUF = try #require(try await registry.get(id: gguf.id))
    #expect(resolvedGGUF.state == .ready)
    #expect(resolvedGGUF.runtime.id == "llama-cpp")
    let untouchedOllama = try #require(try await registry.get(id: ollama.id))
    #expect(untouchedOllama.runtime.id == nil)
    #expect(untouchedOllama.state != .ready)
}

@Test func userResolutionSurvivesReResolution() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    var record = try ggufRecord(at: dir)
    record.runtime = RuntimeRef(
        id: "ollama", resolved: .user, tier: .native, confirmedAt: Date())
    try await registry.register(record)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.runtime.id == "ollama")
    #expect(after.runtime.resolved == .user)
}

@Test func pinnedRuntimeWhoseAdapterVanishedDemotesHonestly() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let bundle = dir.appendingPathComponent("dark-model")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data("xyz".utf8).write(to: bundle.appendingPathComponent("weights.xyz"))
    var record = ModelRecord(
        name: "dark-model", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: SourceKind(rawValue: "fixture"), path: bundle.path))
    record.runtime = RuntimeRef(
        id: "dark-runner", resolved: .user, tier: .managed, confirmedAt: Date())
    record.state = .ready
    try await registry.register(record)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let demoted = try #require(try await registry.get(id: record.id))
    #expect(demoted.runtime.id == nil)
    #expect(demoted.runtime.resolved == .unresolved)
    #expect(demoted.runtime.tier == .recipeNeeded)
    #expect(demoted.state == .unresolved)
}

@Test func confirmStampsAndSurvivesReResolution() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir.appendingPathComponent("store"))
    let record = try ggufRecord(at: dir)
    try await kernel.registry.register(record)
    try await kernel.resolve()

    try await kernel.confirmRuntime(record.id)
    try await kernel.resolve()

    let after = try #require(try await kernel.registry.get(id: record.id))
    #expect(after.runtime.confirmedAt != nil)
    #expect(after.runtime.id == "llama-cpp")
}

@Test func chatMLFallbackPromptShape() {
    let prompt = LlamaEngine.chatMLPrompt(messages: [
        .init(role: .system, content: "be brief"),
        .init(role: .user, content: "hi"),
    ])
    #expect(prompt.hasSuffix("<|im_start|>assistant\n"))
    #expect(prompt.contains("<|im_start|>system\nbe brief<|im_end|>"))
    #expect(prompt.contains("<|im_start|>user\nhi<|im_end|>"))
}

@Test func equalPreferenceTieBreaksOnAdapterID() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = try ggufRecord(at: dir)
    try await registry.register(record)

    let engine = ResolutionEngine(adapters: [
        EqualBidAdapter(adapterID: "zeta-runner"),
        EqualBidAdapter(adapterID: "alpha-runner"),
    ])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "alpha-runner")
    #expect(resolved.runtime.alternatives == ["zeta-runner"])

    try await engine.resolveAll(in: registry)
    let again = try #require(try await registry.get(id: record.id))
    #expect(again == resolved)
}

@Test func concurrentDemotionIsNotRevertedByResolve() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = try ggufRecord(at: dir)
    try await registry.register(record)

    let engine = ResolutionEngine(adapters: [LlamaCppAdapter()])
    try await engine.resolve(record, in: registry)
    let staleSnapshot = try #require(try await registry.get(id: record.id))
    #expect(staleSnapshot.state == .ready)

    try await registry.setStateIfPresent(id: record.id, to: .missing)
    try await engine.resolve(staleSnapshot, in: registry)

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .missing)
}

private struct EqualBidAdapter: RuntimeAdapter {
    let adapterID: String

    var id: RuntimeID { RuntimeID(rawValue: adapterID) }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool { false }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        RuntimeBid(tier: .native, preference: 50)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
