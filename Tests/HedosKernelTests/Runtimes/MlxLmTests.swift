import Foundation
import Testing

@testable import HedosKernel

private let mlxCausalLMConfig =
    #"{"architectures": ["LlamaForCausalLM"], "model_type": "llama", "quantization": {"bits": 4}}"#

private func mlxTextRecord() -> ModelRecord {
    ModelRecord(
        name: "Llama-3.2-1B-Instruct-4bit",
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/models/huggingface/hub/models--mlx-community--Llama-3.2-1B-Instruct-4bit",
            repo: "mlx-community/Llama-3.2-1B-Instruct-4bit"),
        runtime: RuntimeRef(
            id: "python:mlx-lm",
            resolved: .auto,
            tier: .managed),
        execution: .stream,
        footprintMB: 700,
        state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func mlxLmRuntimeBundleShipsCompleteAndValid() throws {
    let bundle = try #require(RuntimeBundle.directory(named: "python-mlx-lm"))
    let fm = FileManager.default
    for file in ["main.py", "manifest.toml", "requirements.in", "requirements.lock", "sandbox.sb"]
    {
        #expect(
            fm.fileExists(atPath: bundle.appendingPathComponent(file).path),
            "missing \(file)")
    }
    let manifest = try String(
        contentsOf: bundle.appendingPathComponent("manifest.toml"), encoding: .utf8)
    #expect(manifest.contains("python:mlx-lm"))
    #expect(manifest.contains(#"execution    = "stream""#))
    #expect(manifest.contains("network = false"))
    let lock = try String(
        contentsOf: bundle.appendingPathComponent("requirements.lock"), encoding: .utf8)
    #expect(lock.contains("mlx-lm=="))
    #expect(lock.contains("--hash=sha256:"))
    let profile = try Data(contentsOf: bundle.appendingPathComponent("sandbox.sb"))
    let audioBundle = try #require(RuntimeBundle.directory(named: "python-mlx-audio"))
    let audioProfile = try Data(contentsOf: audioBundle.appendingPathComponent("sandbox.sb"))
    #expect(profile == audioProfile)
    let contents = try fm.subpathsOfDirectory(atPath: bundle.path)
    #expect(!contents.contains { $0.contains("__pycache__") })
}

@Test func mlxLmAdapterBidMatrix() {
    let adapter = MlxLmAdapter()
    let record = mlxTextRecord()

    let mlxText = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.chat, .complete],
        execution: .stream)
    let bid = adapter.bid(record, mlxText)
    #expect(bid?.tier == .managed)
    #expect(bid?.preference == 40)

    let plainText = IdentifiedModel(
        format: .safetensors, modality: .text, capabilities: [.chat, .complete],
        execution: .stream)
    let mlxSpeech = IdentifiedModel(
        format: .mlxSafetensors, modality: .speech, capabilities: [.speak], execution: .stream)
    let ggufText = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat, .complete], execution: .stream)
    #expect(adapter.bid(record, plainText) == nil)
    #expect(adapter.bid(record, mlxSpeech) == nil)
    #expect(adapter.bid(record, ggufText) == nil)
}

@Test func resolvesMlxCausalLMCacheRecordToMlxLmManaged() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let hubRoot = dir.appendingPathComponent("hub")
    try DiscoveryFixtures.makeHFRepo(
        at: hubRoot,
        DiscoveryFixtures.HFRepo(
            org: "mlx-community", repo: "Llama-3.2-1B-Instruct-4bit",
            files: [("model.safetensors", 512), ("tokenizer.json", 64)],
            configJSON: mlxCausalLMConfig))

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    _ = try await DiscoveryService(scanners: [HFCacheScanner(roots: [hubRoot])])
        .discover(into: registry)
    let engine = ResolutionEngine(adapters: [
        LlamaCppAdapter(), OllamaAdapter(), MlxAudioAdapter(), MlxLmAdapter(),
    ])
    try await engine.resolveAll(in: registry)

    let records = try await registry.list()
    let resolved = try #require(
        records.first { $0.name.contains("Llama-3.2-1B-Instruct-4bit") })
    #expect(resolved.runtime.id == "python:mlx-lm")
    #expect(resolved.runtime.tier == .managed)
    #expect(resolved.state == .ready)
    #expect(resolved.execution == .stream)
    #expect(resolved.capabilities.contains(.chat))
    #expect(resolved.params.contains { $0.key == "temperature" })

    let explanation = await engine.explain(resolved)
    #expect(explanation.bids.count == 1)
    #expect(explanation.winner == "python:mlx-lm")
}

@Test func controlFrameCarriesOpAndPerModelParams() {
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hi")])
        ]),
        "temperature": .double(0.4),
        "top_p": .double(0.9),
        "max_tokens": .int(256),
    ])
    let chat = PythonSidecarRuntime.control(op: Capability.chat.rawValue, payload: payload).objectValue
    #expect(chat?["op"]?.stringValue == "chat")
    #expect(chat?["temperature"]?.doubleValue == 0.4)
    #expect(chat?["top_p"]?.doubleValue == 0.9)
    #expect(chat?["max_tokens"]?.intValue == 256)
    if case .array(let messages)? = chat?["messages"] {
        #expect(messages.count == 1)
    } else {
        Issue.record("messages missing from control frame")
    }

    let complete = PythonSidecarRuntime.control(op: Capability.complete.rawValue, payload: .object(["prompt": .string("hi")]))
        .objectValue
    #expect(complete?["op"]?.stringValue == "complete")
    #expect(complete?["prompt"]?.stringValue == "hi")
}

@Test func canServeChatAndCompleteOnly() {
    let adapter = MlxLmAdapter()
    let record = mlxTextRecord()
    #expect(adapter.canServe(record, .chat))
    #expect(adapter.canServe(record, .complete))
    #expect(!adapter.canServe(record, .speak))
    #expect(!adapter.canServe(record, .transcribe))
    var other = record
    other.runtime.id = "ollama"
    #expect(!adapter.canServe(other, .chat))
}

@Test func mlxLmChatStreamsThroughFakeSidecarWithStats() async throws {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
    let spec = SidecarSpec(
        runtimeID: "fake-mlxlm-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", script.path, "normal"],
        readyTimeout: .seconds(15),
        cooperativeCancel: true)
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hello")])
        ])
    ])
    var deltas: [String] = []
    var statuses: [String] = []
    var stats: GenerationStats?
    let stream = await supervisor.request(spec, PythonSidecarRuntime.control(op: Capability.chat.rawValue, payload: payload))
    for try await chunk in stream {
        switch chunk {
        case .text(let delta): deltas.append(delta)
        case .status(let message): statuses.append(message)
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(statuses.contains("generating"))
    #expect(deltas.joined() == "hello!")
    #expect(stats?.completionTokens == 3)
    #expect(stats?.promptTokens == 1)
    await supervisor.shutdownAll()
}
