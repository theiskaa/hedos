import Foundation
import Testing

@testable import HedosKernel

/// Builds a composite fixture machine: all four stores populated, one
/// deliberately planted duplicate (same bytes in LM Studio and Downloads).
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
    // Planted duplicate: identical bytes in LM Studio and Downloads.
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

/// The M2 gate, headless: full discovery over a composite fixture machine.
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

    // The planted duplicate is flagged with correct waste.
    #expect(summary.duplicates.count == 1)
    #expect(summary.duplicates[0].wastedBytes == 5000)

    // Records persisted.
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
    #expect(records.count == 6)  // kept, not deleted
    let missing = try #require(records.first { $0.source.kind == .lmStudio })
    #expect(missing.state == .missing)
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
    try DiscoveryFixtures.makeGGUF(at: b, bytes: 4096, fill: 0x02)  // same size, different bytes

    func model(_ url: URL) -> DiscoveredModel {
        DiscoveredModel(
            name: url.lastPathComponent,
            source: ModelSource(kind: .file, path: url.path),
            footprintBytes: 4096, primaryWeightPath: url.path)
    }
    let differing = DuplicateDetector.detect(in: [model(a), model(b)], threshold: 1024)
    #expect(differing.isEmpty)

    // Below threshold: ignored even when identical.
    try DiscoveryFixtures.makeGGUF(at: b, bytes: 4096, fill: 0x01)
    let identical = DuplicateDetector.detect(in: [model(a), model(b)], threshold: 8192)
    #expect(identical.isEmpty)
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
