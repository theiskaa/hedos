import Foundation
import Testing

@testable import HedosKernel

private func writeUserManifest(_ text: String, kernelDir: URL, name: String) throws {
    let runtimesDir = kernelDir.appendingPathComponent("runtimes.d", isDirectory: true)
    try FileManager.default.createDirectory(at: runtimesDir, withIntermediateDirectories: true)
    try Data(text.utf8).write(to: runtimesDir.appendingPathComponent("\(name).toml"))
}

private func darkRecord(in dir: URL) throws -> ModelRecord {
    let bundle = dir.appendingPathComponent("dark-model", isDirectory: true)
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    let weight = bundle.appendingPathComponent("weights.xyz")
    try Data("xyz".utf8).write(to: weight)
    var record = ModelRecord(
        name: "dark-model", modality: .unknown, capabilities: [],
        source: ModelSource(kind: SourceKind(rawValue: "fixture"), path: bundle.path))
    record.primaryWeightPath = weight.path
    return record
}

private let darkManifest = """
    id = "dark-runner"
    modalities = ["text"]
    capabilities = ["chat", "complete"]
    execution = "stream"
    detect = { extension = "xyz" }
    [invoke]
    command = "echo {prompt}"
    """

@Test func discoverLoadsRuntimesDAndResolvesDarkRecord() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())
    try writeUserManifest(darkManifest, kernelDir: dir, name: "dark-runner")
    let record = try darkRecord(in: dir)
    try await kernel.registry.register(record)

    _ = try await kernel.discover()

    let resolved = try #require(try await kernel.registry.get(id: record.id))
    #expect(resolved.runtime.id == "dark-runner")
    #expect(resolved.runtime.tier == .managed)
    #expect(resolved.state == .ready)
    #expect(resolved.modality == .text)
    #expect(resolved.capabilities == [.chat, .complete])
    #expect(resolved.execution == .stream)
}

@Test func discoverSurfacesManifestIssuesInSummary() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())
    try writeUserManifest("this is not toml at all", kernelDir: dir, name: "broken")

    let summary = try await kernel.discover()
    #expect(summary.issues.contains { $0.contains("broken.toml") })
}

@Test func injectedAdaptersSurviveReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(
        directory: dir, adapters: [OllamaAdapter()], secrets: InMemorySecretStore())
    try writeUserManifest(darkManifest, kernelDir: dir, name: "dark-runner")
    let record = try darkRecord(in: dir)
    try await kernel.registry.register(record)

    _ = try await kernel.discover()

    let resolved = try #require(try await kernel.registry.get(id: record.id))
    #expect(resolved.runtime.id == "dark-runner")
    let explanations = try await kernel.explainShelf()
    #expect(explanations.contains { $0.winner == "dark-runner" })
}

@Test func identifiedFactsAreNotClobberedByManifestShape() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let hubRoot = dir.appendingPathComponent("hub")
    try DiscoveryFixtures.makeHFRepo(
        at: hubRoot,
        DiscoveryFixtures.HFRepo(
            org: "acme", repo: "video-thing",
            files: [("weights.safetensors", 64)],
            modelIndexJSON: DiscoveryFixtures.cogVideoModelIndex))
    let record = ModelRecord(
        name: "video-thing", modality: .unknown, capabilities: [],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: hubRoot.appendingPathComponent("models--acme--video-thing").path,
            repo: "acme/video-thing", ref: "abc123def456"))

    let manifest = RuntimeManifest(
        id: "video-runner", modalities: [.text], capabilities: [.chat],
        execution: .stream, alternatives: [],
        detect: ManifestDetect(file: "model_index.json"),
        env: nil, serve: nil, invoke: ManifestInvoke(command: "echo hi"),
        permissions: ManifestPermissions(network: false, paths: []), directory: nil)
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(record)
    let engine = ResolutionEngine(adapters: [
        ManifestCommandAdapter(manifest: manifest, approvedNetwork: false)
    ])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "video-runner")
    #expect(resolved.modality == .video)
    #expect(resolved.capabilities == [.chat])
}

@Test func approveNetworkRuntimeFlipsRecordToReady() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())
    let networkManifest = darkManifest
        .replacingOccurrences(of: "id = \"dark-runner\"", with: "id = \"net-runner\"")
        .replacingOccurrences(
            of: "[invoke]", with: "[permissions]\nnetwork = true\n[invoke]")
    try writeUserManifest(networkManifest, kernelDir: dir, name: "net-runner")
    let record = try darkRecord(in: dir)
    try await kernel.registry.register(record)

    _ = try await kernel.discover()
    let unresolved = try #require(try await kernel.registry.get(id: record.id))
    #expect(unresolved.runtime.tier == .recipeNeeded)

    let consent = try await kernel.pendingNetworkConsent(for: record.id)
    #expect(consent?.id == "net-runner")

    try await kernel.approveNetworkRuntime("net-runner")
    let resolved = try #require(try await kernel.registry.get(id: record.id))
    #expect(resolved.runtime.id == "net-runner")
    #expect(resolved.state == .ready)
    #expect(try await kernel.pendingNetworkConsent(for: record.id) == nil)
}

@Test func manifestTemplateRendersDetectFromRecordFacts() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    let diffusersBundle = dir.appendingPathComponent("sd-thing", isDirectory: true)
    try FileManager.default.createDirectory(
        at: diffusersBundle, withIntermediateDirectories: true)
    try Data(#"{"_class_name": "SomePipeline"}"#.utf8)
        .write(to: diffusersBundle.appendingPathComponent("model_index.json"))
    let diffusersRecord = ModelRecord(
        name: "sd-thing", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: diffusersBundle.path))
    let diffusersTemplate = ManifestTemplate.render(
        record: diffusersRecord, identified: Identification.identify(diffusersRecord))
    #expect(diffusersTemplate.contains(#"contains = "SomePipeline""#))

    var weightRecord = ModelRecord(
        name: "loose", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .file, path: dir.appendingPathComponent("m.pth").path))
    weightRecord.primaryWeightPath = dir.appendingPathComponent("m.pth").path
    let weightTemplate = ManifestTemplate.render(
        record: weightRecord, identified: Identification.identify(weightRecord))
    #expect(weightTemplate.contains(#"extension = "pth""#))

    let reparsed = try TOMLLite.parse(
        diffusersTemplate.replacingOccurrences(of: "# or replace", with: "# alt"))
    #expect(reparsed["id"] != nil)
}

@Test func kernelInvokeDispatchesToManifestCommandAdapterEndToEnd() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, secrets: InMemorySecretStore())
    try FileManager.default.createDirectory(
        at: dir.appendingPathComponent("dark-model"), withIntermediateDirectories: true)
    let script = dir.appendingPathComponent("dark-model/fake_e2e.py")
    try Data("import sys\nprint(\"reply:\", sys.argv[1])".utf8).write(to: script)
    let pythonProcess = Process()
    pythonProcess.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    pythonProcess.arguments = [
        "python3", "-c", "import os, sys; print(os.path.realpath(sys.executable))",
    ]
    let pythonPipe = Pipe()
    pythonProcess.standardOutput = pythonPipe
    try pythonProcess.run()
    let pythonPath = String(
        decoding: try pythonPipe.fileHandleForReading.readToEnd() ?? Data(), as: UTF8.self
    ).trimmingCharacters(in: .whitespacesAndNewlines)
    pythonProcess.waitUntilExit()
    let e2eManifest = """
        id = "e2e-runner"
        modalities = ["text"]
        capabilities = ["chat", "complete"]
        execution = "stream"
        detect = { extension = "xyz" }
        [invoke]
        command = "\(pythonPath) \(script.path) {prompt}"
        """
    try writeUserManifest(e2eManifest, kernelDir: dir, name: "e2e-runner")
    defer {
        try? FileManager.default.removeItem(
            at: Registry.defaultDirectory().appendingPathComponent("workdirs/e2e-runner"))
    }
    let record = try darkRecord(in: dir)
    try await kernel.registry.register(record)
    _ = try await kernel.discover()

    var text = ""
    let stream = try await kernel.chat(
        record.id, messages: [.init(role: .user, content: "pong")])
    for try await chunk in stream {
        if case .text(let delta) = chunk { text += delta }
    }
    #expect(text.contains("reply: pong"))
}
