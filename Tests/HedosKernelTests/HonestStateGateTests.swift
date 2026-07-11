import Foundation
import Testing

@testable import HedosKernel

private func gateKernel(appSupport: URL, home: URL) -> Kernel {
    Kernel(
        directory: appSupport,
        governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore(),
        habitat: ModelHabitat(home: home, environment: [:]),
        duplicateThreshold: 1024)
}

private func probe(_ kernel: Kernel) async throws -> String {
    ShelfReport.render(try await kernel.explainShelf())
}

@Test func gatePartialHFDownloadShowsStillDownloadingThenHeals() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let home = root.appendingPathComponent("home")
    let hub = home.appendingPathComponent(".cache/huggingface/hub")
    try FileManager.default.createDirectory(at: hub, withIntermediateDirectories: true)
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        .init(
            org: "mlx-community", repo: "Downloading-Model",
            files: [("model.safetensors", 4096), ("tokenizer.json", 64)],
            configJSON: DiscoveryFixtures.causalLMConfig,
            incompleteBlobs: ["pending"]))

    let kernel = gateKernel(appSupport: root.appendingPathComponent("app"), home: home)
    _ = try await kernel.discover()
    let before = try await probe(kernel)
    print("── GATE partial-download (during) ──\n\(before)")
    #expect(before.contains("still downloading"))

    try FileManager.default.removeItem(
        at: hub.appendingPathComponent(
            "models--mlx-community--Downloading-Model/blobs/pending.incomplete"))
    _ = try await kernel.discover()
    let after = try await probe(kernel)
    print("── GATE partial-download (finished) ──\n\(after)")
    #expect(!after.contains("still downloading"))
}

@Test func gateShardedGGUFCollapsesAndDeletingPartFlagsDownloading() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let home = root.appendingPathComponent("home")
    let models = home.appendingPathComponent("Models")
    let written = try DiscoveryFixtures.makeShardedGGUF(
        at: models, baseName: "big-model", parts: 3)

    let kernel = gateKernel(appSupport: root.appendingPathComponent("app"), home: home)
    let summary = try await kernel.discover()
    let whole = try await probe(kernel)
    print("── GATE shards (complete) ──\n\(whole)")
    #expect(summary.perKind[.file]?.count == 1)

    try FileManager.default.removeItem(at: written[1])
    _ = try await kernel.discover()
    let partial = try await probe(kernel)
    print("── GATE shards (part deleted) ──\n\(partial)")
    #expect(partial.contains("still downloading"))
}

@Test func gateMovedConfiguredModelKeepsConfigWithNoOrphan() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let home = root.appendingPathComponent("home")
    let downloads = home.appendingPathComponent("Downloads")
    let models = home.appendingPathComponent("Models")
    try FileManager.default.createDirectory(at: downloads, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: models, withIntermediateDirectories: true)
    let start = downloads.appendingPathComponent("keeper.gguf")
    try DiscoveryFixtures.makeGGUF(at: start, bytes: 4096, fill: 0x4D)

    let kernel = gateKernel(appSupport: root.appendingPathComponent("app"), home: home)
    _ = try await kernel.discover()
    var configured = try #require(
        try await kernel.registry.list().first { $0.name == "keeper" })
    configured.alias = "Keeper"
    configured.systemPrompt = "stay terse"
    try await kernel.registry.register(configured)

    try FileManager.default.moveItem(at: start, to: models.appendingPathComponent("keeper.gguf"))
    _ = try await kernel.discover()

    let all = try await kernel.registry.list().filter { $0.name == "keeper" }
    print("── GATE move (records after) ──")
    for record in all {
        print("  \(record.displayName) · \(record.state.rawValue) · \(record.primaryWeightPath ?? "-")")
    }
    #expect(all.count == 1)
    let moved = try #require(all.first)
    #expect(moved.alias == "Keeper")
    #expect(moved.systemPrompt == "stay terse")
    #expect(moved.primaryWeightPath?.hasSuffix("/Models/keeper.gguf") == true)
}

@Test func gateResolveIsByteIdenticalAcrossTwoPasses() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let home = root.appendingPathComponent("home")
    let models = home.appendingPathComponent("Models")
    try DiscoveryFixtures.makeShardedGGUF(at: models, baseName: "sharded", parts: 2)
    try DiscoveryFixtures.makeGGUF(
        at: models.appendingPathComponent("solo.gguf"), bytes: 2048, fill: 0x19)

    let app = root.appendingPathComponent("app")
    let kernel = gateKernel(appSupport: app, home: home)
    _ = try await kernel.discover()
    let first = try Data(contentsOf: app.appendingPathComponent("models.json"))
    _ = try await kernel.discover()
    let second = try Data(contentsOf: app.appendingPathComponent("models.json"))
    print("── GATE byte-identical models.json ── first=\(first.count)B second=\(second.count)B equal=\(first == second)")
    #expect(first == second)
}

@Test func gateGGUFOnlyHFRepoParticipatesInDedup() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let home = root.appendingPathComponent("home")
    let hub = home.appendingPathComponent(".cache/huggingface/hub")
    try FileManager.default.createDirectory(at: hub, withIntermediateDirectories: true)
    let gguf = DiscoveryFixtures.data(bytes: 4096, fill: 0x2E)
    for repo in ["Copy-One", "Copy-Two"] {
        let dir = hub.appendingPathComponent("models--org--\(repo)/snapshots/rev")
        let blobs = hub.appendingPathComponent("models--org--\(repo)/blobs")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: blobs, withIntermediateDirectories: true)
        let refs = hub.appendingPathComponent("models--org--\(repo)/refs")
        try FileManager.default.createDirectory(at: refs, withIntermediateDirectories: true)
        try "rev".write(to: refs.appendingPathComponent("main"), atomically: true, encoding: .utf8)
        try gguf.write(to: blobs.appendingPathComponent("blob0"))
        try FileManager.default.createSymbolicLink(
            at: dir.appendingPathComponent("model.gguf"),
            withDestinationURL: blobs.appendingPathComponent("blob0"))
        try Data(DiscoveryFixtures.causalLMConfig.utf8)
            .write(to: blobs.appendingPathComponent("cfg"))
        try FileManager.default.createSymbolicLink(
            at: dir.appendingPathComponent("config.json"),
            withDestinationURL: blobs.appendingPathComponent("cfg"))
    }

    let kernel = gateKernel(appSupport: root.appendingPathComponent("app"), home: home)
    let summary = try await kernel.discover()
    print("── GATE gguf-only dedup ── duplicate groups: \(summary.duplicates.count)")
    #expect(!summary.duplicates.isEmpty)
}
