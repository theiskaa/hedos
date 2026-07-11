import Foundation
import Testing

@testable import HedosKernel

private func serveManifest(
    id: String = "test-serve-\(UUID().uuidString.prefix(8))",
    execution: ExecutionMode = .stream,
    network: Bool = false,
    directory: URL? = nil
) -> RuntimeManifest {
    RuntimeManifest(
        id: id, modalities: [.text], capabilities: [.chat],
        execution: execution, alternatives: ["ollama"],
        detect: ManifestDetect(fileExtension: "xyz"),
        env: nil,
        serve: ManifestServe(entrypoint: "main.py", wireProtocol: "ndjson+frames"),
        invoke: nil,
        permissions: ManifestPermissions(network: network, paths: []),
        directory: directory)
}

private func xyzRecord(in dir: URL, runtimeID: String) throws -> ModelRecord {
    let weights = dir.appendingPathComponent("model.xyz")
    try Data("weights".utf8).write(to: weights)
    var record = ModelRecord(
        name: "serve-model", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .file, path: weights.path))
    record.runtime = RuntimeRef(
        id: RuntimeID(rawValue: runtimeID), resolved: .auto, tier: .managed)
    record.state = .ready
    return record
}

@Test func serveSpecBuildsSandboxedEntrypointWithCooperativeCancelForStreams() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let resources = dir.appendingPathComponent("runtime", isDirectory: true)
    try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
    try Data("print()".utf8).write(to: resources.appendingPathComponent("main.py"))
    let manifest = serveManifest(directory: resources)
    let adapter = ManifestSidecarAdapter(
        manifest: manifest, approvedHostExecution: true, approvedNetwork: false,
        workdirRoot: dir.appendingPathComponent("workdirs"))
    let record = try xyzRecord(in: dir, runtimeID: manifest.id)

    let spec = try adapter.spec(record: record, envDir: nil)
    #expect(spec.runtimeID == "\(manifest.id)#\(record.id)")
    #expect(spec.executable.path == "/usr/bin/sandbox-exec")
    #expect(spec.arguments.contains("/usr/bin/python3"))
    #expect(spec.arguments.contains(resources.appendingPathComponent("main.py").path))
    #expect(spec.cooperativeCancel)
    #expect(spec.environment["PYTHONPATH"] == "")

    let jobManifest = serveManifest(execution: .job, directory: resources)
    let jobAdapter = ManifestSidecarAdapter(
        manifest: jobManifest, approvedHostExecution: true, approvedNetwork: false,
        workdirRoot: dir.appendingPathComponent("workdirs"))
    let jobSpec = try jobAdapter.spec(record: record, envDir: nil)
    #expect(!jobSpec.cooperativeCancel)
}

@Test func serveAdapterRefusesBothPathsWithoutNetworkConsent() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let manifest = serveManifest(network: true)
    let adapter = ManifestSidecarAdapter(
        manifest: manifest, approvedHostExecution: false, approvedNetwork: false,
        workdirRoot: dir.appendingPathComponent("workdirs"))
    let record = try xyzRecord(in: dir, runtimeID: manifest.id)

    do {
        for try await _ in adapter.invoke(record, .chat, payload: .object([:])) {}
        Issue.record("expected the stream consent refusal")
    } catch let KernelError.runtimeUnavailable(hint) {
        #expect(hint.contains("needs your approval"))
    }
    do {
        for try await _ in adapter.run(record, .chat, payload: .object([:])) {}
        Issue.record("expected the job consent refusal")
    } catch let KernelError.runtimeUnavailable(hint) {
        #expect(hint.contains("needs your approval"))
    }
    #expect(adapter.bid(record, Identification.identify(record)) == nil)
}

@Test func serveAdapterSurfacesMissingServeSectionAsARuntimeFailure() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    var manifest = serveManifest()
    manifest.serve = nil
    let adapter = ManifestSidecarAdapter(
        manifest: manifest, approvedHostExecution: true, approvedNetwork: false,
        workdirRoot: dir.appendingPathComponent("workdirs"))
    let record = try xyzRecord(in: dir, runtimeID: manifest.id)

    do {
        for try await _ in adapter.invoke(record, .chat, payload: .object([:])) {}
        Issue.record("expected the missing [serve] failure")
    } catch let KernelError.runtimeFailed(message) {
        #expect(message.contains("[serve]"))
    }
}

@Test func serveAdapterBidCarriesManifestTierPreferenceAndAlternatives() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let manifest = serveManifest()
    let adapter = ManifestSidecarAdapter(
        manifest: manifest, approvedHostExecution: true, approvedNetwork: false,
        workdirRoot: dir.appendingPathComponent("workdirs"))
    let record = try xyzRecord(in: dir, runtimeID: manifest.id)

    let bid = try #require(adapter.bid(record, Identification.identify(record)))
    #expect(bid.tier == .managed)
    #expect(bid.preference == BidPreference.manifest)
    #expect(bid.alternatives == [.ollama])
}
