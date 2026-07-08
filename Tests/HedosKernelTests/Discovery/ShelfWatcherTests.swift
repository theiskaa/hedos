import Foundation
import Testing

@testable import HedosKernel

private func fixtureHabitat(home: URL, hub: URL? = nil) -> ModelHabitat {
    var environment: [String: String] = ["HF_HOME": "", "HF_HUB_CACHE": ""]
    if let hub {
        environment["HF_HUB_CACHE"] = hub.path
    }
    return ModelHabitat(home: home, environment: environment)
}

private func pollUntil(
    attempts: Int = 200, interval: Duration = .milliseconds(50),
    _ condition: () async throws -> Bool
) async rethrows -> Bool {
    for _ in 0..<attempts {
        if try await condition() { return true }
        try? await Task.sleep(for: interval)
    }
    return false
}

@Test func habitatMapRoutesEventPathsToOwningKinds() {
    let map = HabitatMap(roots: [
        (.ollama, URL(fileURLWithPath: "/fake/home/.ollama/models")),
        (.file, URL(fileURLWithPath: "/fake/home/Models")),
    ])

    #expect(
        map.kinds(forEventPath: "/fake/home/.ollama/models/manifests", rootExists: { _ in true })
            == [.ollama])
    #expect(
        map.kinds(forEventPath: "/fake/home/Models/", rootExists: { _ in true }) == [.file])
    #expect(map.kinds(forEventPath: "/fake/home/Documents", rootExists: { _ in true }).isEmpty)
    #expect(
        map.kinds(forEventPath: "/fake/home", rootExists: { _ in true })
            == [.ollama, .file])
    #expect(map.kinds(forEventPath: "/fake/home", rootExists: { _ in false }).isEmpty)
}

@Test func habitatBuildsOnlyScannersForRequestedKinds() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let habitat = fixtureHabitat(home: dir)
    let models = ModelsSettings()

    let ollamaOnly = habitat.scanners(kinds: [.ollama], models: models)
    #expect(ollamaOnly.count == 1)
    #expect(ollamaOnly.first is OllamaStoreScanner)

    let looseOnly = habitat.scanners(kinds: [.file], models: models)
    #expect(looseOnly.count == 1)
    #expect(looseOnly.first is LooseFileScanner)

    let all = habitat.scanners(kinds: nil, models: models)
    #expect(all.count == 5)
}

@Test func watcherEmitsOllamaKindAfterStoreMutation() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let store = home.appendingPathComponent(".ollama/models")
    try DiscoveryFixtures.makeOllamaStore(
        at: store, tags: [.init(model: "first", tag: "latest", modelBytes: 64)])

    let watcher = ShelfWatcher(
        roots: [(.ollama, store)], debounce: .milliseconds(150))
    watcher.start()
    defer { watcher.stop() }

    let collector = Task { () -> Bool in
        for await kinds in watcher.events where kinds.contains(.ollama) {
            return true
        }
        return false
    }
    try await Task.sleep(for: .milliseconds(300))
    try DiscoveryFixtures.makeOllamaStore(
        at: store, tags: [.init(model: "second", tag: "latest", modelBytes: 64)])

    let raced = Task {
        try? await Task.sleep(for: .seconds(10))
        collector.cancel()
    }
    let sawEvent = await collector.value
    raced.cancel()
    #expect(sawEvent)
}

@Test func watcherCoalescesBurstIntoSingleEmission() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let root = home.appendingPathComponent("Models")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

    let watcher = ShelfWatcher(roots: [(.file, root)], debounce: .milliseconds(800))
    watcher.start()
    defer { watcher.stop() }

    let counter = EmissionCounter()
    let collector = Task {
        for await _ in watcher.events {
            await counter.increment()
        }
    }
    try await Task.sleep(for: .milliseconds(300))
    for index in 0..<10 {
        try Data("x".utf8).write(to: root.appendingPathComponent("burst-\(index).bin"))
        try await Task.sleep(for: .milliseconds(20))
    }

    let sawFirst = await pollUntil { await counter.value >= 1 }
    #expect(sawFirst)
    try await Task.sleep(for: .seconds(2))
    let total = await counter.value
    #expect(total == 1)
    collector.cancel()
}

private actor EmissionCounter {
    private(set) var value = 0

    func increment() {
        value += 1
    }
}

@Test func watcherPicksUpHabitatRootCreatedLater() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let root = home.appendingPathComponent("Models")

    let watcher = ShelfWatcher(roots: [(.file, root)], debounce: .milliseconds(150))
    watcher.start()
    defer { watcher.stop() }

    let collector = Task { () -> Bool in
        for await kinds in watcher.events where kinds.contains(.file) {
            return true
        }
        return false
    }
    try await Task.sleep(for: .milliseconds(300))
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    try Data("gguf".utf8).write(to: root.appendingPathComponent("late.gguf"))

    let raced = Task {
        try? await Task.sleep(for: .seconds(10))
        collector.cancel()
    }
    let sawEvent = await collector.value
    raced.cancel()
    #expect(sawEvent)
}

@Test func scopedRescanRegistersNewModelWithoutFullSweep() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = home.appendingPathComponent(".ollama/models")
    try DiscoveryFixtures.makeOllamaStore(
        at: store, tags: [.init(model: "seed", tag: "latest", modelBytes: 64)])
    let looseDir = home.appendingPathComponent("Models")
    try DiscoveryFixtures.makeGGUF(at: looseDir.appendingPathComponent("seed.gguf"), bytes: 128)

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home))
    let baseline = try await kernel.discover()
    let baselineFileStat = baseline.perKind[.file]
    await kernel.startWatching(debounce: .milliseconds(150))

    let updates = await kernel.shelfUpdates()
    let collector = Task { () -> DiscoverySummary? in
        for await summary in updates {
            if summary.perKind[.ollama]?.count == 2 { return summary }
        }
        return nil
    }
    try await Task.sleep(for: .milliseconds(400))
    try DiscoveryFixtures.makeOllamaStore(
        at: store,
        tags: [
            .init(model: "seed", tag: "latest", modelBytes: 64),
            .init(model: "fresh", tag: "latest", modelBytes: 64),
        ])

    let raced = Task {
        try? await Task.sleep(for: .seconds(10))
        collector.cancel()
    }
    let updated = await collector.value
    raced.cancel()
    await kernel.stopWatching()

    let summary = try #require(updated)
    #expect(summary.perKind[.ollama]?.count == 2)
    #expect(summary.perKind[.file] == baselineFileStat)
    let records = try await kernel.shelf()
    let fresh = try #require(records.first { $0.name.contains("fresh") })
    #expect(fresh.state == .ready)
    #expect(fresh.runtime.id == "ollama")
}

@Test func deletionMarksMissingLiveAndReappearanceHeals() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let looseDir = home.appendingPathComponent("Models")
    let gguf = looseDir.appendingPathComponent("here.gguf")
    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 64))
    try FileManager.default.createDirectory(at: looseDir, withIntermediateDirectories: true)
    try payload.write(to: gguf)

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home))
    _ = try await kernel.discover()
    let records = try await kernel.shelf()
    let record = try #require(records.first { $0.name == "here" })
    await kernel.startWatching(debounce: .milliseconds(150))
    try await Task.sleep(for: .milliseconds(400))

    try FileManager.default.removeItem(at: gguf)
    let missing = await pollUntil {
        (try? await kernel.registry.get(id: record.id))??.state == .missing
    }
    #expect(missing)

    try payload.write(to: gguf)
    let healed = await pollUntil {
        (try? await kernel.registry.get(id: record.id))??.state == .ready
    }
    #expect(healed)
    await kernel.stopWatching()
}

@Test func rearmOnAddWatchedFolderWatchesNewRoot() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let extra = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: extra) }

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home))
    _ = try await kernel.discover()
    await kernel.startWatching(debounce: .milliseconds(150))
    try await kernel.addWatchedFolder(extra.path)
    try await Task.sleep(for: .milliseconds(400))

    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 64))
    try payload.write(to: extra.appendingPathComponent("dropped.gguf"))

    let appeared = await pollUntil {
        let records = (try? await kernel.shelf()) ?? []
        return records.contains { $0.name == "dropped" && $0.state == .ready }
    }
    #expect(appeared)
    await kernel.stopWatching()
}

@Test func shelfUpdatesFansOutToAllSubscribers() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let looseDir = home.appendingPathComponent("Models")
    try FileManager.default.createDirectory(at: looseDir, withIntermediateDirectories: true)

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home))
    _ = try await kernel.discover()
    await kernel.startWatching(debounce: .milliseconds(150))

    let first = await kernel.shelfUpdates()
    let second = await kernel.shelfUpdates()
    let firstTask = Task { () -> Bool in
        for await _ in first { return true }
        return false
    }
    let secondTask = Task { () -> Bool in
        for await _ in second { return true }
        return false
    }
    try await Task.sleep(for: .milliseconds(400))
    try DiscoveryFixtures.makeGGUF(at: looseDir.appendingPathComponent("fan.gguf"), bytes: 64)

    let raced = Task {
        try? await Task.sleep(for: .seconds(10))
        firstTask.cancel()
        secondTask.cancel()
    }
    let firstSaw = await firstTask.value
    let secondSaw = await secondTask.value
    raced.cancel()
    #expect(firstSaw)
    #expect(secondSaw)
    await kernel.stopWatching()
}