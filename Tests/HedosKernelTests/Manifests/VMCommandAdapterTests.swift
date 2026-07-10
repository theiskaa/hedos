import Foundation
import Testing

@testable import HedosKernel

private actor FakeVMHost: VMHost {
    var assets: VMAssetState = .ready
    var envReady = false
    var provisionCalls = 0
    var runCalls: [VMRunRequest] = []
    var cancelled: [String] = []
    var runResult = VMRunResult(exitCode: 0, stdout: "spoken\n", stderr: "")
    var outputFiles: [(name: String, data: Data)] = []

    func configure(
        assets: VMAssetState = .ready, envReady: Bool = false,
        runResult: VMRunResult = VMRunResult(exitCode: 0, stdout: "spoken\n", stderr: ""),
        outputFiles: [(name: String, data: Data)] = []
    ) {
        self.assets = assets
        self.envReady = envReady
        self.runResult = runResult
        self.outputFiles = outputFiles
    }

    func assetState() async -> VMAssetState { assets }

    func provisionAssets(onStatus: (@Sendable (String) async -> Void)?) async throws {}

    func environmentReady(_ request: VMRunRequest) async -> Bool { envReady }

    func provisionEnvironment(
        _ request: VMRunRequest, onStatus: (@Sendable (String) async -> Void)?
    ) async throws {
        provisionCalls += 1
        envReady = true
    }

    func run(_ request: VMRunRequest) async throws -> VMRunResult {
        runCalls.append(request)
        for file in outputFiles {
            try file.data.write(
                to: URL(fileURLWithPath: request.outputs).appendingPathComponent(file.name))
        }
        return runResult
    }

    func cancel(runtimeID: String) async {
        cancelled.append(runtimeID)
    }
}

private func vmManifest(execution: ExecutionMode = .sync, capability: Capability = .speak)
    throws -> RuntimeManifest
{
    let toml = """
        id = "vm-speak-test"
        capabilities = ["\(capability.rawValue)"]
        execution = "\(execution.rawValue)"
        detect = { extension = "gguf" }

        [vm]
        image = "docker.io/library/python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"
        setup = ["pip install soundfile"]

        [invoke]
        command = "python3 {resources}/main.py --model {model} --out {outputs} --text {prompt}"
        """
    return try RuntimeManifest.load(
        table: try TOMLLite.parse(toml), directory: Fixtures.tempDirectory())
}

private func speakRecord() -> ModelRecord {
    var record = Fixtures.gguf()
    record.runtime.id = "vm-speak-test"
    return record
}

@Test func adapterBidsOnDetectMatch() throws {
    let manifest = try vmManifest()
    let adapter = VMCommandAdapter(manifest: manifest, host: FakeVMHost())
    let record = Fixtures.gguf()
    let bid = adapter.bid(record, Identification.identify(record))
    #expect(bid?.tier == .managed)
    #expect(bid?.preference == 100)

    var mismatched = Fixtures.flux()
    mismatched.primaryWeightPath = "/tmp/model_index.json"
    #expect(adapter.bid(mismatched, Identification.identify(mismatched)) == nil)
}

@Test func adapterProvisionsOnceThenRunsWithGuestPaths() async throws {
    let host = FakeVMHost()
    let manifest = try vmManifest()
    let adapter = VMCommandAdapter(manifest: manifest, host: host)
    let record = speakRecord()
    let payload = JSONValue.object(["prompt": .string("say hello")])

    for _ in 0..<2 {
        var chunks: [CapabilityChunk] = []
        for try await chunk in adapter.invoke(record, .speak, payload: payload) {
            chunks.append(chunk)
        }
        #expect(chunks.contains(.text("spoken\n")))
    }

    #expect(await host.provisionCalls == 1)
    let calls = await host.runCalls
    #expect(calls.count == 2)
    let request = calls[0]
    #expect(request.arguments.contains("--text"))
    #expect(request.arguments.contains("say hello"))
    #expect(request.arguments.contains(VMGuestPath.model))
    #expect(request.arguments.contains(VMGuestPath.outputs))
    #expect(request.arguments.first == "python3")
    #expect(request.arguments.contains("\(VMGuestPath.resources)/main.py"))
    #expect(request.image.contains("@sha256:"))
    #expect(request.modelPath != nil)
    #expect(FileManager.default.fileExists(atPath: request.workdir))
}

@Test func adapterSurfacesNonZeroExitHonestly() async throws {
    let host = FakeVMHost()
    await host.configure(
        envReady: true,
        runResult: VMRunResult(exitCode: 3, stdout: "", stderr: "Traceback\nValueError: boom"))
    let adapter = VMCommandAdapter(manifest: try vmManifest(), host: host)
    do {
        for try await _ in adapter.invoke(
            speakRecord(), .speak, payload: .object(["prompt": .string("x")])) {}
        Issue.record("nonzero exit should throw")
    } catch let error as KernelError {
        #expect((error.errorDescription ?? "").contains("ValueError: boom"))
    }
}

@Test func adapterCollectsJobOutputsFromTheOutputsMount() async throws {
    let host = FakeVMHost()
    let png = Data([0x89, 0x50, 0x4E, 0x47])
    await host.configure(envReady: true, outputFiles: [("result.png", png)])
    let toml = """
        id = "vm-image-test"
        capabilities = ["image"]
        execution = "job"
        detect = { extension = "gguf" }

        [vm]
        image = "docker.io/library/python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"

        [invoke]
        command = "python3 {resources}/gen.py --out {outputs}"
        """
    let manifest = try RuntimeManifest.load(
        table: try TOMLLite.parse(toml), directory: Fixtures.tempDirectory())
    let adapter = VMCommandAdapter(manifest: manifest, host: host)
    var record = Fixtures.gguf()
    record.runtime.id = "vm-image-test"

    var events: [JobRuntimeEvent] = []
    for try await event in adapter.run(
        record, .image, payload: .object(["prompt": .string("draw")]))
    {
        events.append(event)
    }
    let results = events.compactMap { event -> Data? in
        if case .result(let data, let ext) = event, ext == "png" { return data }
        return nil
    }
    #expect(results == [png])
}

@Test func kernelWiresVMManifestsToTheVMAdapter() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let runtimeDir = dir.appendingPathComponent("runtimes.d/vm-speak-test", isDirectory: true)
    try FileManager.default.createDirectory(at: runtimeDir, withIntermediateDirectories: true)
    let toml = """
        id = "vm-speak-test"
        capabilities = ["speak"]
        execution = "sync"
        detect = { extension = "pth" }

        [vm]
        image = "docker.io/library/python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"

        [invoke]
        command = "python3 {resources}/main.py --model {model}"
        """
    try toml.write(
        to: runtimeDir.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)
    try RuntimeProvenance(origin: RuntimeProvenance.communityOrigin).write(in: runtimeDir)

    let host = FakeVMHost()
    let kernel = Kernel(
        directory: dir, adapters: [], secrets: InMemorySecretStore(), vmHost: host)
    _ = try await kernel.discover()
    let installed = await kernel.runtimeCatalog.installedCommunity()
    #expect(installed.count == 1)
    #expect(installed[0].id == "vm-speak-test")
    #expect(installed[0].provenance?.isCommunity == true)
}
