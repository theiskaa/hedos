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

    let qwen = try #require(byName["qwen3.5:9b"])
    #expect(qwen.footprintBytes == 4096 + 64)
    #expect(qwen.modalityHint == .text)
    #expect(qwen.capabilitiesHint == [.chat, .complete])
    #expect(qwen.primaryWeightPath?.contains("blobs/sha256-") == true)

    #expect(byName["someorg/custom:latest"] != nil)
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

@Test func ollamaScannerReadsNumCtxStopAndTemplatePresence() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try DiscoveryFixtures.makeOllamaStore(
        at: root,
        tags: [
            .init(
                model: "qwen3.5", tag: "9b", modelBytes: 4096,
                paramsJSON: #"{"num_ctx": 8192, "stop": ["<|im_end|>", "<|endoftext|>"]}"#,
                hasTemplateLayer: true)
        ])

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.issues.isEmpty)
    let model = try #require(result.discovered.first)
    #expect(model.contextLengthHint == 8192)
    #expect(model.stopTokensHint == ["<|im_end|>", "<|endoftext|>"])
    #expect(model.hasChatTemplateHint == true)
}

@Test func paramsBlobWithoutNumCtxYieldsNilHint() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try DiscoveryFixtures.makeOllamaStore(
        at: root,
        tags: [
            .init(
                model: "gemma4", tag: "latest", modelBytes: 2048,
                paramsJSON: #"{"temperature": 1, "top_k": 64, "top_p": 0.95}"#)
        ])

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.issues.isEmpty)
    let model = try #require(result.discovered.first)
    #expect(model.contextLengthHint == nil)
    #expect(model.stopTokensHint == nil)
    #expect(model.hasChatTemplateHint == nil)
}

@Test func unreadableParamsBlobStillRegistersWithIssue() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try DiscoveryFixtures.makeOllamaStore(
        at: root,
        tags: [
            .init(
                model: "qwen3.5", tag: "9b", modelBytes: 2048,
                paramsJSON: #"{"num_ctx": 8192}"#,
                paramsBlobMissing: true)
        ])

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.discovered.count == 1)
    #expect(result.discovered.first?.contextLengthHint == nil)
    #expect(result.issues == ["ollama: unreadable params blob for qwen3.5:9b"])
}

@Test func unreadableOllamaRootReportsFailedKind() async throws {
    let root = try Fixtures.tempDirectory()
    defer {
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: root.path)
        try? FileManager.default.removeItem(at: root)
    }
    try DiscoveryFixtures.makeOllamaStore(
        at: root, tags: [.init(model: "qwen3.5", tag: "9b", modelBytes: 4096)])
    try FileManager.default.setAttributes(
        [.posixPermissions: 0o000], ofItemAtPath: root.path)

    let result = await OllamaStoreScanner(root: root).scan()
    #expect(result.failedKinds == [.ollama])
    #expect(result.discovered.isEmpty)
}

@Test func absentUserWatchedFolderReportsFailedKind() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let gone = root.appendingPathComponent("unmounted-drive/models")

    let result = await LooseFileScanner(directories: [], userDirectories: [gone]).scan()
    #expect(result.failedKinds == [.file, .folder])
    #expect(result.discovered.isEmpty)
}

@Test func absentDefaultRootReportsFoundNothing() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let gone = root.appendingPathComponent("never-installed")

    let ollama = await OllamaStoreScanner(root: gone).scan()
    #expect(ollama.failedKinds.isEmpty)
    #expect(ollama.discovered.isEmpty)

    let loose = await LooseFileScanner(directories: [gone]).scan()
    #expect(loose.failedKinds.isEmpty)

    let lmstudio = await LMStudioScanner(roots: [gone]).scan()
    #expect(lmstudio.failedKinds.isEmpty)

    let hf = await HFCacheScanner(roots: [gone]).scan()
    #expect(hf.failedKinds.isEmpty)
}

@Test func absentUserHFCacheRootReportsFailedKind() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let gone = root.appendingPathComponent("external/hf")

    let result = await HFCacheScanner(roots: [], userRoots: [gone]).scan()
    #expect(result.failedKinds == [.huggingfaceCache])
}

@Test func hfScannerCarriesContextLengthHint() async throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "mlx-community", repo: "Tiny-Chat-4bit",
            files: [("model.safetensors", 512), ("tokenizer.json", 64)],
            configJSON:
                #"{"architectures": ["LlamaForCausalLM"], "max_position_embeddings": 8192}"#))

    let result = await HFCacheScanner(root: hub).scan()
    let model = try #require(result.discovered.first)
    #expect(model.contextLengthHint == 8192)
    #expect(model.modalityHint == .text)
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
    #expect(flux.footprintBytes >= 8192)
    #expect(flux.source.ref == "abc123def456")
    #expect(flux.primaryWeightPath?.hasSuffix("blob0") == true)

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
    let fakeHome = URL(fileURLWithPath: "/tmp/hedos-fake-home-for-tests")
    let roots = HFCacheScanner.defaultRoots(
        environment: ["HF_HOME": "/tmp/stale-override"], home: fakeHome)
    #expect(roots.count == 2)
    #expect(roots[0].path == "/tmp/stale-override/hub")
    #expect(roots[1].path == "\(fakeHome.path)/.cache/huggingface/hub")

    let plain = HFCacheScanner.defaultRoots(environment: [:], home: fakeHome)
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

@Test func userHFRootsJoinDefaultRootsWithHubDetectionAndDedup() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let withHub = dir.appendingPathComponent("home-style")
    try FileManager.default.createDirectory(
        at: withHub.appendingPathComponent("hub"), withIntermediateDirectories: true)
    let bare = dir.appendingPathComponent("bare-hub")
    try FileManager.default.createDirectory(at: bare, withIntermediateDirectories: true)

    let roots = HFCacheScanner.defaultRoots(
        environment: [:], user: [withHub.path, bare.path, withHub.path])
    let paths = roots.map(\.path)
    #expect(paths.contains(withHub.appendingPathComponent("hub").path))
    #expect(paths.contains(bare.path))
    #expect(paths.filter { $0 == withHub.appendingPathComponent("hub").path }.count == 1)

    let envRoots = HFCacheScanner.defaultRoots(
        environment: ["HF_HOME": withHub.path], user: [withHub.path])
    #expect(envRoots.filter { $0.path == withHub.appendingPathComponent("hub").path }.count == 1)
}
