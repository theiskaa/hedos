import Foundation
import Testing

@testable import HedosKernel

private func makeCompositeMachine() throws -> (root: URL, scanners: [any StoreScanner]) {
    let root = try Fixtures.tempDirectory()
    let ollama = root.appendingPathComponent("ollama")
    let hub = root.appendingPathComponent("hub")
    let lmstudio = root.appendingPathComponent("lmstudio")
    let downloads = root.appendingPathComponent("Downloads")

    try DiscoveryFixtures.makeOllamaStore(
        at: ollama,
        tags: [
            .init(model: "gemma4", tag: "latest", modelBytes: 6000, extraBytes: 100),
            .init(model: "qwen3.5", tag: "9b", modelBytes: 4000),
        ])
    try FileManager.default.createDirectory(at: hub, withIntermediateDirectories: true)
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "black-forest-labs", repo: "FLUX.1-schnell",
            files: [("transformer.safetensors", 9000)],
            modelIndexJSON: DiscoveryFixtures.fluxModelIndex))
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "mlx-community", repo: "Kokoro-82M-bf16",
            files: [("kokoro.safetensors", 2000)],
            configJSON: DiscoveryFixtures.kokoroConfig))
    try DiscoveryFixtures.makeGGUF(
        at: lmstudio.appendingPathComponent("org/dup/dup-model.gguf"), bytes: 5000, fill: 0x55)
    try DiscoveryFixtures.makeGGUF(
        at: downloads.appendingPathComponent("dup-model-copy.gguf"), bytes: 5000, fill: 0x55)

    let scanners: [any StoreScanner] = [
        OllamaStoreScanner(root: ollama),
        HFCacheScanner(root: hub),
        LMStudioScanner(roots: [lmstudio]),
        LooseFileScanner(directories: [downloads]),
    ]
    return (root, scanners)
}

@Test func discoveryFindsEverythingAndReportsDiskTruth() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))

    let summary = try await DiscoveryService(scanners: scanners, duplicateThreshold: 1024)
        .discover(into: registry)

    #expect(summary.totalCount == 6)
    #expect(summary.perKind[.ollama]?.count == 2)
    #expect(summary.perKind[.ollama]?.bytes == Int64(6000 + 100 + 4000))
    #expect(summary.perKind[.huggingfaceCache]?.count == 2)
    #expect(summary.perKind[.lmStudio]?.count == 1)
    #expect(summary.perKind[.file]?.count == 1)
    #expect(summary.issues.isEmpty)
    #expect(summary.headline.hasPrefix("Found 6 models on this Mac — 2 in Ollama"))
    #expect(summary.headline.contains("2 in the Hugging Face cache"))
    #expect(summary.headline.contains("1 in LM Studio"))
    #expect(summary.headline.contains("1 loose file."))

    #expect(summary.duplicates.count == 1)
    #expect(summary.duplicates[0].wastedBytes == 5000)

    #expect(try await registry.list().count == 6)
}

@Test func rescanIsIdempotent() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)

    _ = try await service.discover(into: registry)
    let first = try await registry.list()
    _ = try await service.discover(into: registry)
    let second = try await registry.list()

    #expect(second.count == first.count)
    #expect(Set(second.map(\.id)) == Set(first.map(\.id)))
}

@Test func vanishedModelsAreMarkedMissingNotDeleted() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)
    _ = try await service.discover(into: registry)

    try FileManager.default.removeItem(
        at: root.appendingPathComponent("lmstudio/org/dup/dup-model.gguf"))
    _ = try await service.discover(into: registry)

    let records = try await registry.list()
    #expect(records.count == 6)
    let missing = try #require(records.first { $0.source.kind == .lmStudio })
    #expect(missing.state == .missing)
}

@Test func rescanWithoutHintPreservesKnownContextLength() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let ollama = root.appendingPathComponent("ollama")
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))

    try DiscoveryFixtures.makeOllamaStore(
        at: ollama,
        tags: [
            .init(
                model: "qwen3.5", tag: "9b", modelBytes: 2048,
                paramsJSON: #"{"num_ctx": 8192}"#)
        ])
    let service = DiscoveryService(
        scanners: [OllamaStoreScanner(root: ollama)], duplicateThreshold: 1024)
    _ = try await service.discover(into: registry)
    let known = try #require(try await registry.list().first)
    #expect(known.contextLength == 8192)

    try FileManager.default.removeItem(at: ollama)
    try DiscoveryFixtures.makeOllamaStore(
        at: ollama,
        tags: [
            .init(
                model: "qwen3.5", tag: "9b", modelBytes: 2048,
                paramsJSON: #"{"temperature": 1}"#)
        ])
    _ = try await service.discover(into: registry)
    let preserved = try #require(try await registry.list().first)
    #expect(preserved.contextLength == 8192)
}

@Test func discoveryPassWritesStoreOnce() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)

    _ = try await service.discover(into: registry)
    #expect(await registry.saveCount == 1)

    _ = try await service.discover(into: registry)
    #expect(await registry.saveCount == 1)
}

@Test func failedScanKindSkipsMissingSweep() async throws {
    let (root, scanners) = try makeCompositeMachine()
    let ollama = root.appendingPathComponent("ollama")
    defer {
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: ollama.path)
        try? FileManager.default.removeItem(at: root)
    }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)
    _ = try await service.discover(into: registry)

    try FileManager.default.setAttributes(
        [.posixPermissions: 0o000], ofItemAtPath: ollama.path)
    let summary = try await service.discover(into: registry)

    #expect(summary.failedKinds == [.ollama])
    #expect(
        summary.issues.contains(
            "skipped the missing check for ollama — its store could not be read"))
    let ollamaRecords = try await registry.list().filter { $0.source.kind == .ollama }
    #expect(ollamaRecords.count == 2)
    #expect(ollamaRecords.allSatisfy { $0.state != .missing })
}

@Test func foundNothingStillDemotes() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)
    _ = try await service.discover(into: registry)

    try FileManager.default.removeItem(at: root.appendingPathComponent("lmstudio"))
    let summary = try await service.discover(into: registry)

    #expect(summary.failedKinds.isEmpty)
    let lmRecords = try await registry.list().filter { $0.source.kind == .lmStudio }
    #expect(lmRecords.count == 1)
    #expect(lmRecords.allSatisfy { $0.state == .missing })
}

@Test func userRuntimeChoiceSurvivesRescan() async throws {
    let (root, scanners) = try makeCompositeMachine()
    defer { try? FileManager.default.removeItem(at: root) }
    let registry = Registry(directory: root.appendingPathComponent("appsupport"))
    let service = DiscoveryService(scanners: scanners, duplicateThreshold: 1024)
    _ = try await service.discover(into: registry)

    var record = try #require(try await registry.list().first { $0.name == "qwen3.5:9b" })
    record.runtime = RuntimeRef(id: "ollama", resolved: .user, tier: .native)
    try await registry.register(record)

    _ = try await service.discover(into: registry)
    let after = try #require(try await registry.get(id: record.id))
    #expect(after.runtime.id == "ollama")
    #expect(after.runtime.resolved == .user)
}

@Test func duplicateDetectorRequiresContentMatchNotJustSize() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let a = dir.appendingPathComponent("a.gguf")
    let b = dir.appendingPathComponent("b.gguf")
    try DiscoveryFixtures.makeGGUF(at: a, bytes: 4096, fill: 0x01)
    try DiscoveryFixtures.makeGGUF(at: b, bytes: 4096, fill: 0x02)

    func model(_ url: URL) -> DiscoveredModel {
        DiscoveredModel(
            name: url.lastPathComponent,
            source: ModelSource(kind: .file, path: url.path),
            footprintBytes: 4096, primaryWeightPath: url.path)
    }
    let differing = DuplicateDetector.detect(in: [model(a), model(b)], threshold: 1024)
    #expect(differing.isEmpty)

    try DiscoveryFixtures.makeGGUF(at: b, bytes: 4096, fill: 0x01)
    let identical = DuplicateDetector.detect(in: [model(a), model(b)], threshold: 1024)
    #expect(identical.count == 1)
    #expect(identical.first?.wastedBytes == 4096)
}

private func makeStripedFile(
    at url: URL, totalBytes: Int, headByte: UInt8, middleByte: UInt8, tailByte: UInt8
) throws {
    try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    let sampleSize = 1 << 20
    var data = Data(repeating: middleByte, count: totalBytes)
    data.replaceSubrange(0..<sampleSize, with: Data(repeating: headByte, count: sampleSize))
    data.replaceSubrange(
        (totalBytes - sampleSize)..<totalBytes, with: Data(repeating: tailByte, count: sampleSize))
    try data.write(to: url)
}

@Test func duplicateDetectorSamplesHeadAndTailNotJustFirstMegabyte() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let totalBytes = 3 << 20

    func model(_ url: URL) -> DiscoveredModel {
        DiscoveredModel(
            name: url.lastPathComponent,
            source: ModelSource(kind: .file, path: url.path),
            footprintBytes: Int64(totalBytes), primaryWeightPath: url.path)
    }

    let sameHeadA = dir.appendingPathComponent("same-head-a.gguf")
    let sameHeadB = dir.appendingPathComponent("same-head-b.gguf")
    try makeStripedFile(
        at: sameHeadA, totalBytes: totalBytes, headByte: 0xAA, middleByte: 0xBB, tailByte: 0xCC)
    try makeStripedFile(
        at: sameHeadB, totalBytes: totalBytes, headByte: 0xAA, middleByte: 0xBB, tailByte: 0xDD)
    let notDuplicates = DuplicateDetector.detect(
        in: [model(sameHeadA), model(sameHeadB)], threshold: 1024)
    #expect(notDuplicates.isEmpty)

    let identicalA = dir.appendingPathComponent("identical-a.gguf")
    let identicalB = dir.appendingPathComponent("identical-b.gguf")
    try makeStripedFile(
        at: identicalA, totalBytes: totalBytes, headByte: 0xAA, middleByte: 0xBB, tailByte: 0xCC)
    try makeStripedFile(
        at: identicalB, totalBytes: totalBytes, headByte: 0xAA, middleByte: 0xBB, tailByte: 0xCC)
    let duplicates = DuplicateDetector.detect(
        in: [model(identicalA), model(identicalB)], threshold: 1024)
    #expect(duplicates.count == 1)
    #expect(duplicates.first?.wastedBytes == Int64(totalBytes))
}

@Test func headlineFormatting() {
    let empty = DiscoverySummary(
        perKind: [:], totalCount: 0, totalBytes: 0, duplicates: [], issues: [])
    #expect(empty.headline == "No models found on this Mac yet.")

    let one = DiscoverySummary(
        perKind: [.ollama: .init(count: 1, bytes: 5 << 30)],
        totalCount: 1, totalBytes: 5 << 30, duplicates: [], issues: [])
    #expect(one.headline == "Found 1 model on this Mac — 1 in Ollama. Total: 5 GB.")

    #expect(DiscoverySummary.formatBytes(87 << 30) == "87 GB")
    #expect(DiscoverySummary.formatBytes(Int64(6.5 * Double(1 << 30))) == "6.5 GB")
    #expect(DiscoverySummary.formatBytes(350 << 20) == "350 MB")
    #expect(DiscoverySummary.formatBytes(512) == "512 B")
}

@Test func downloadingRepoStaysUnresolvedThenHealsWhenBlobsFinish() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let hub = root.appendingPathComponent("hub")
    try FileManager.default.createDirectory(at: hub, withIntermediateDirectories: true)
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "mlx-community", repo: "Downloading-Model",
            files: [("model.safetensors", 512), ("tokenizer.json", 64)],
            configJSON: DiscoveryFixtures.causalLMConfig,
            incompleteBlobs: ["pending"]))

    let registry = Registry(directory: root.appendingPathComponent("store"))
    let service = DiscoveryService(scanners: [HFCacheScanner(root: hub)], duplicateThreshold: 1024)
    let engine = ResolutionEngine(adapters: [AlwaysBidAdapter()])

    _ = try await service.discover(into: registry)
    try await engine.resolveAll(in: registry)
    let downloading = try #require(try await registry.list().first)
    #expect(downloading.downloading)
    #expect(downloading.state == .unresolved)

    try FileManager.default.removeItem(
        at: hub.appendingPathComponent(
            "models--mlx-community--Downloading-Model/blobs/pending.incomplete"))

    _ = try await service.discover(into: registry)
    try await engine.resolveAll(in: registry)
    let healed = try #require(try await registry.list().first)
    #expect(!healed.downloading)
    #expect(healed.state == .ready)
}

private func moveScenario() throws -> (root: URL, registry: Registry, service: DiscoveryService, fileA: URL, dirB: URL) {
    let root = try Fixtures.tempDirectory()
    let dirA = root.appendingPathComponent("A")
    let dirB = root.appendingPathComponent("B")
    try FileManager.default.createDirectory(at: dirA, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: dirB, withIntermediateDirectories: true)
    let fileA = dirA.appendingPathComponent("model.gguf")
    try DiscoveryFixtures.makeGGUF(at: fileA, bytes: 4096, fill: 0x5A)

    let registry = Registry(directory: root.appendingPathComponent("store"))
    let service = DiscoveryService(
        scanners: [LooseFileScanner(directories: [dirA, dirB])], duplicateThreshold: 1 << 30)
    return (root, registry, service, fileA, dirB)
}

@Test func movedWeightKeepsAliasPromptAndParamValues() async throws {
    let (root, registry, service, fileA, dirB) = try moveScenario()
    defer { try? FileManager.default.removeItem(at: root) }
    _ = try await service.discover(into: registry)

    var configured = try #require(try await registry.list().first)
    configured.params = [
        ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)])
    ]
    configured.alias = "My Model"
    configured.systemPrompt = "be concise"
    configured.paramValues = ["temperature": .double(0.4)]
    try await registry.register(configured)

    try FileManager.default.moveItem(at: fileA, to: dirB.appendingPathComponent("model.gguf"))
    _ = try await service.discover(into: registry)

    let records = try await registry.list()
    #expect(records.count == 1)
    let moved = try #require(records.first)
    #expect(moved.primaryWeightPath?.hasSuffix("/B/model.gguf") == true)
    #expect(moved.alias == "My Model")
    #expect(moved.systemPrompt == "be concise")
    #expect(moved.paramValues["temperature"] == .double(0.4))
    #expect(moved.state != .missing)
}

@Test func moveMigrationRemovesTheOrphanedRecord() async throws {
    let (root, registry, service, fileA, dirB) = try moveScenario()
    defer { try? FileManager.default.removeItem(at: root) }
    _ = try await service.discover(into: registry)
    let originalID = try #require(try await registry.list().first).id

    try FileManager.default.moveItem(at: fileA, to: dirB.appendingPathComponent("model.gguf"))
    _ = try await service.discover(into: registry)

    #expect(try await registry.get(id: originalID) == nil)
    #expect(try await registry.list().count == 1)
}

@Test func moveMigrationSkipsWhenTwoOrphansShareContent() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let dirA = root.appendingPathComponent("A")
    let dirB = root.appendingPathComponent("B")
    try FileManager.default.createDirectory(at: dirA, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: dirB, withIntermediateDirectories: true)
    let one = dirA.appendingPathComponent("one.gguf")
    let two = dirA.appendingPathComponent("two.gguf")
    try DiscoveryFixtures.makeGGUF(at: one, bytes: 4096, fill: 0x7C)
    try DiscoveryFixtures.makeGGUF(at: two, bytes: 4096, fill: 0x7C)

    let registry = Registry(directory: root.appendingPathComponent("store"))
    let service = DiscoveryService(
        scanners: [LooseFileScanner(directories: [dirA, dirB])], duplicateThreshold: 1 << 30)
    _ = try await service.discover(into: registry)

    var configured = try #require(try await registry.list().first {
        $0.primaryWeightPath?.hasSuffix("one.gguf") == true
    })
    configured.alias = "Configured"
    try await registry.register(configured)

    try FileManager.default.removeItem(at: one)
    try FileManager.default.removeItem(at: two)
    try DiscoveryFixtures.makeGGUF(
        at: dirB.appendingPathComponent("moved.gguf"), bytes: 4096, fill: 0x7C)
    _ = try await service.discover(into: registry)

    let moved = try #require(try await registry.list().first {
        $0.primaryWeightPath?.hasSuffix("moved.gguf") == true
    })
    #expect(moved.alias == nil)
}

private struct FlippableFoundationBackend: AppleFoundationBackend {
    let state: BuiltinAvailability
    func availability() -> BuiltinAvailability { state }

    func stream(
        messages: [ChatMessage], temperature: Double?, topP: Double?, topK: Int?,
        seed: UInt64?, maxTokens: Int?, tools: [ToolSpec],
        resultProvider: BuiltinToolResultProvider?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}

@Test func builtinFlipDemotesOnScanAndHealsOnReturn() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    func service(_ state: BuiltinAvailability) -> DiscoveryService {
        DiscoveryService(
            scanners: [AppleFoundationScanner(backend: FlippableFoundationBackend(state: state))],
            duplicateThreshold: 1 << 30)
    }

    _ = try await service(.available).discover(into: registry)
    #expect(try await registry.list().first?.state != .missing)

    _ = try await service(.notEnabled).discover(into: registry)
    let demoted = try await registry.list()
    #expect(demoted.count == 1)
    #expect(demoted.first?.state == .missing)

    _ = try await service(.available).discover(into: registry)
    #expect(try await registry.list().first?.state != .missing)
}

private struct AlwaysBidAdapter: RuntimeAdapter {
    var id: RuntimeID { "always" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool { true }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        RuntimeBid(tier: .native, preference: 10)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
