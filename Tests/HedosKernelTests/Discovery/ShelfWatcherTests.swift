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
    retryEvery: Int = Int.max, onRetry: (@Sendable () -> Void)? = nil,
    _ condition: () async throws -> Bool
) async rethrows -> Bool {
    for attempt in 0..<attempts {
        if try await condition() { return true }
        if attempt > 0, attempt % retryEvery == 0 {
            onRetry?()
        }
        try? await Task.sleep(for: interval)
    }
    return false
}

private func repeatingTrigger(
    every interval: Duration = .milliseconds(250), times: Int = 40,
    _ action: @escaping @Sendable () -> Void
) -> Task<Void, Never> {
    Task {
        for _ in 0..<times {
            if Task.isCancelled { return }
            action()
            try? await Task.sleep(for: interval)
        }
    }
}

private func awaitFirstEvent<T: Sendable>(
    ceiling: Duration = .seconds(10),
    collector: Task<T, Never>,
    trigger: @escaping @Sendable () -> Void
) async -> T {
    let armed = repeatingTrigger(every: .milliseconds(250), times: 40, trigger)
    let racer = Task {
        try? await Task.sleep(for: ceiling)
        collector.cancel()
    }
    let outcome = await collector.value
    racer.cancel()
    armed.cancel()
    return outcome
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
    let sawEvent = await awaitFirstEvent(collector: collector) {
        try? DiscoveryFixtures.makeOllamaStore(
            at: store, tags: [.init(model: "second", tag: "latest", modelBytes: 64)])
    }
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

    for index in 0..<10 {
        try Data("x".utf8).write(to: root.appendingPathComponent("burst-\(index).bin"))
    }

    let sawFirst = await pollUntil { await counter.value >= 1 }
    #expect(sawFirst)
    try await Task.sleep(for: .milliseconds(2400))
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
    let sawEvent = await awaitFirstEvent(collector: collector) {
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try? Data("gguf".utf8).write(to: root.appendingPathComponent("late.gguf"))
    }
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
    let updated = await awaitFirstEvent(collector: collector) {
        try? DiscoveryFixtures.makeOllamaStore(
            at: store,
            tags: [
                .init(model: "seed", tag: "latest", modelBytes: 64),
                .init(model: "fresh", tag: "latest", modelBytes: 64),
            ])
    }
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
    let payload: Data = {
        var data = Data("GGUF".utf8)
        data.append(DiscoveryFixtures.data(bytes: 64))
        return data
    }()
    try FileManager.default.createDirectory(at: looseDir, withIntermediateDirectories: true)
    try payload.write(to: gguf)

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home))
    _ = try await kernel.discover()
    let records = try await kernel.shelf()
    let record = try #require(records.first { $0.name == "here" })
    await kernel.startWatching(debounce: .milliseconds(150))

    let deleteFile: @Sendable () -> Void = { try? FileManager.default.removeItem(at: gguf) }
    deleteFile()
    let missing = await pollUntil(retryEvery: 20, onRetry: deleteFile) {
        (try? await kernel.registry.get(id: record.id))??.state == .missing
    }
    #expect(missing)

    let healFile: @Sendable () -> Void = { try? payload.write(to: gguf) }
    healFile()
    let healed = await pollUntil(retryEvery: 20, onRetry: healFile) {
        (try? await kernel.registry.get(id: record.id))??.state == .ready
    }
    #expect(healed)
    await kernel.stopWatching()
}

@Test func scopedRescanDropsStaleDuplicateCardForRemovedModel() async throws {
    let home = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: home) }
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let looseDir = home.appendingPathComponent("Models")
    try FileManager.default.createDirectory(at: looseDir, withIntermediateDirectories: true)

    let dupA = looseDir.appendingPathComponent("dup-a.gguf")
    let dupB = looseDir.appendingPathComponent("dup-b.gguf")
    try DiscoveryFixtures.makeGGUF(at: dupA, bytes: 4096, fill: 0x5A)
    try DiscoveryFixtures.makeGGUF(at: dupB, bytes: 4096, fill: 0x5A)

    let kernel = Kernel(
        directory: dir, secrets: InMemorySecretStore(), habitat: fixtureHabitat(home: home),
        duplicateThreshold: 1024)
    let initial = try await kernel.discover()
    #expect(initial.duplicates.count == 1)
    #expect(initial.duplicates.first?.paths.count == 2)

    await kernel.startWatching(debounce: .milliseconds(150))
    let updates = await kernel.shelfUpdates()
    let collector = Task { () -> Bool in
        for await summary in updates {
            if summary.duplicates.isEmpty { return true }
        }
        return false
    }
    let removeDup: @Sendable () -> Void = { try? FileManager.default.removeItem(at: dupB) }
    let sawEmptyDuplicates = await awaitFirstEvent(collector: collector, trigger: removeDup)
    #expect(sawEmptyDuplicates)
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

    let payload: Data = {
        var data = Data("GGUF".utf8)
        data.append(DiscoveryFixtures.data(bytes: 64))
        return data
    }()
    let dropFile: @Sendable () -> Void = {
        try? payload.write(to: extra.appendingPathComponent("dropped.gguf"))
    }
    dropFile()

    let appeared = await pollUntil(retryEvery: 20, onRetry: dropFile) {
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
    let writeFan: @Sendable () -> Void = {
        try? DiscoveryFixtures.makeGGUF(at: looseDir.appendingPathComponent("fan.gguf"), bytes: 64)
    }
    async let firstSaw = awaitFirstEvent(collector: firstTask, trigger: writeFan)
    async let secondSaw = awaitFirstEvent(collector: secondTask, trigger: writeFan)
    let (gotFirst, gotSecond) = await (firstSaw, secondSaw)
    #expect(gotFirst)
    #expect(gotSecond)
    await kernel.stopWatching()
}