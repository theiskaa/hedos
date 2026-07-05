import Foundation
import Testing

@testable import HedosKernel

@Test func ollamaScannerReadsRealManifestShape() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try DiscoveryFixtures.makeOllamaStore(
        at: root,
        tags: [
            .init(model: "qwen3.5", tag: "9b", modelBytes: 4096, extraBytes: 64),
            .init(namespace: "someorg", model: "custom", tag: "latest", modelBytes: 2048),
        ])

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.issues.isEmpty)
    let byName = Dictionary(uniqueKeysWithValues: result.discovered.map { ($0.name, $0) })
    #expect(byName.count == 2)

    let qwen = try #require(byName["qwen3.5:9b"])  // "library" namespace elided
    #expect(qwen.footprintBytes == 4096 + 64)
    #expect(qwen.modalityHint == .text)
    #expect(qwen.capabilitiesHint == [.chat, .complete])
    #expect(qwen.primaryWeightPath?.contains("blobs/sha256-") == true)

    #expect(byName["someorg/custom:latest"] != nil)  // non-library keeps namespace
}

@Test func ollamaScannerIsolatesMalformedManifests() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try DiscoveryFixtures.makeOllamaStore(
        at: root,
        tags: [
            .init(model: "good", tag: "latest", modelBytes: 1024),
            .init(model: "bad", tag: "latest", modelBytes: 0, malformed: true),
        ])

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.discovered.map(\.name) == ["good:latest"])
    #expect(result.issues.count == 1)
}

@Test func hfScannerClassifiesAndMeasures() async throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "black-forest-labs", repo: "FLUX.1-schnell",
            files: [("transformer.safetensors", 8192)],
            modelIndexJSON: DiscoveryFixtures.fluxModelIndex))
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "mlx-community", repo: "Kokoro-82M-bf16",
            files: [("kokoro.safetensors", 2048)],
            configJSON: DiscoveryFixtures.kokoroConfig))

    let result = await HFCacheScanner(root: hub).scan()
    #expect(result.issues.isEmpty)
    let byRepo = Dictionary(
        uniqueKeysWithValues: result.discovered.map { ($0.source.repo ?? "", $0) })

    let flux = try #require(byRepo["black-forest-labs/FLUX.1-schnell"])
    #expect(flux.modalityHint == .image)
    #expect(flux.executionHint == .job)
    #expect(flux.footprintBytes >= 8192)  // blobs include the model_index blob too
    #expect(flux.source.ref == "abc123def456")
    #expect(flux.primaryWeightPath?.hasSuffix("blob0") == true)  // symlink resolved

    let kokoro = try #require(byRepo["mlx-community/Kokoro-82M-bf16"])
    #expect(kokoro.modalityHint == .speech)
    #expect(kokoro.capabilitiesHint == [.speak])
}

@Test func hfScannerFallsBackToNewestSnapshotWithoutRefs() async throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "acme", repo: "no-refs",
            files: [("model.safetensors", 512)],
            configJSON: DiscoveryFixtures.causalLMConfig,
            writeRefsMain: false, revision: "rev-only"))

    let result = await HFCacheScanner(root: hub).scan()
    let model = try #require(result.discovered.first)
    #expect(model.source.ref == "rev-only")
    #expect(model.modalityHint == .text)
}

@Test func hfDefaultRootsNeverLetEnvOverrideHideTheStandardCache() {
    // Regression: a stale HF_HOME pointing at an empty dir must not hide
    // the real ~/.cache/huggingface/hub (39 GB missed on first real run).
    let roots = HFCacheScanner.defaultRoots(environment: ["HF_HOME": "/tmp/stale-override"])
    #expect(roots.count == 2)
    #expect(roots[0].path == "/tmp/stale-override/hub")
    #expect(roots[1].path.hasSuffix(".cache/huggingface/hub"))

    // No override: just the standard location, no duplicates.
    let plain = HFCacheScanner.defaultRoots(environment: [:])
    #expect(plain.count == 1)
}

@Test func lmStudioScannerFindsGGUFsInBothRoots() async throws {
    let rootA = try Fixtures.tempDirectory()
    let rootB = try Fixtures.tempDirectory()
    defer {
        try? FileManager.default.removeItem(at: rootA)
        try? FileManager.default.removeItem(at: rootB)
    }
    try DiscoveryFixtures.makeGGUF(
        at: rootA.appendingPathComponent("org/repo/model-q4.gguf"), bytes: 4096)
    try DiscoveryFixtures.makeGGUF(
        at: rootB.appendingPathComponent("other/thing/tiny.gguf"), bytes: 128)

    let result = await LMStudioScanner(roots: [rootA, rootB]).scan()
    #expect(result.discovered.count == 2)
    let model = try #require(result.discovered.first { $0.name == "model-q4" })
    #expect(model.source.kind == .lmStudio)
    #expect(model.source.repo == "org/repo")
    #expect(model.footprintBytes == 4096)
}

@Test func looseFileScannerFindsFilesAndBundlesWithinDepth() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    // Depth 0 file, depth-1 bundle folder, depth-3 file (must be ignored).
    try DiscoveryFixtures.makeGGUF(at: dir.appendingPathComponent("loose.gguf"), bytes: 256)
    let bundle = dir.appendingPathComponent("some-model")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(DiscoveryFixtures.causalLMConfig.utf8)
        .write(to: bundle.appendingPathComponent("config.json"))
    try DiscoveryFixtures.data(bytes: 1024)
        .write(to: bundle.appendingPathComponent("weights.safetensors"))
    try DiscoveryFixtures.makeGGUF(
        at: dir.appendingPathComponent("a/b/c/deep.gguf"), bytes: 64)

    let result = await LooseFileScanner(directories: [dir]).scan()
    let names = Set(result.discovered.map(\.name))
    #expect(names == ["loose", "some-model"])

    let folder = try #require(result.discovered.first { $0.name == "some-model" })
    #expect(folder.source.kind == .folder)
    #expect(folder.modalityHint == .text)
    #expect(folder.footprintBytes >= 1024)
}
