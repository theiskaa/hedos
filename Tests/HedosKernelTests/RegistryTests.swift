import Foundation
import Testing

@testable import HedosKernel

@Test func registerPersistReloadList() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = Fixtures.flux()

    let first = Registry(directory: dir)
    try await first.register(record)

    let second = Registry(directory: dir)
    let listed = try await second.list()
    #expect(listed == [record])
    #expect(try await second.get(id: record.id) == record)
}

@Test func reRegisteringSameSourceUpsertsNotDuplicates() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)

    var record = Fixtures.gguf()
    try await registry.register(record)
    record.state = .ready
    record.footprintMB = 5400
    try await registry.register(record)

    let listed = try await Registry(directory: dir).list()
    #expect(listed.count == 1)
    #expect(listed[0].state == .ready)
    #expect(listed[0].footprintMB == 5400)
}

@Test func unregisterPersists() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let record = Fixtures.flux()
    try await registry.register(record)
    try await registry.register(Fixtures.gguf())

    let removed = try await registry.unregister(id: record.id)
    #expect(removed == record)

    let listed = try await Registry(directory: dir).list()
    #expect(listed.map(\.name) == ["qwen3.5-9b-q4"])
}

@Test func missingStoreMeansEmptyRegistry() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let listed = try await Registry(directory: dir).list()
    #expect(listed.isEmpty)
}

@Test func corruptStoreThrowsInsteadOfDiscarding() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data("{not json{{".utf8).write(to: dir.appendingPathComponent("models.json"))

    let registry = Registry(directory: dir)
    await #expect(throws: RegistryError.self) {
        try await registry.list()
    }
}

@Test func corruptStoreThrowsAndQuarantines() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let garbage = Data("not json".utf8)
    try garbage.write(to: dir.appendingPathComponent("models.json"))

    let registry = Registry(directory: dir)
    await #expect(throws: RegistryError.self) {
        try await registry.list()
    }

    let contents = try FileManager.default.contentsOfDirectory(atPath: dir.path)
    let quarantined = contents.filter { $0.hasPrefix("models.json.corrupt-") }
    #expect(quarantined.count == 1)
    #expect(!contents.contains("models.json"))
    let quarantinedData = try Data(
        contentsOf: dir.appendingPathComponent(quarantined[0]))
    #expect(quarantinedData == garbage)
}

@Test func registryRecoversAfterQuarantine() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data("not json".utf8).write(to: dir.appendingPathComponent("models.json"))

    let registry = Registry(directory: dir)
    await #expect(throws: RegistryError.self) {
        try await registry.list()
    }

    let record = Fixtures.flux()
    try await registry.register(record)
    let listed = try await registry.list()
    #expect(listed == [record])

    let contents = try FileManager.default.contentsOfDirectory(atPath: dir.path)
    #expect(contents.contains { $0.hasPrefix("models.json.corrupt-") })
}

@Test func storeCarriesSchemaVersion() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try await Registry(directory: dir).register(Fixtures.gguf())

    let raw = try JSONSerialization.jsonObject(
        with: Data(contentsOf: dir.appendingPathComponent("models.json"))) as! [String: Any]
    #expect(raw["schemaVersion"] as? Int == 1)
    #expect((raw["models"] as? [[String: Any]])?.count == 1)
}

@Test func setStateIfPresentMutatesOnlyStateAndPreservesTierAndAlternatives() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)

    var record = Fixtures.gguf()
    record.state = .ready
    record.runtime.tier = .native
    record.runtime.alternatives = ["generic:llama.cpp", "generic:openai-server"]
    try await registry.register(record)

    let changed = try await registry.setStateIfPresent(id: record.id, to: .missing)
    #expect(changed)

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .missing)
    #expect(after.runtime.tier == .native)
    #expect(after.runtime.alternatives == ["generic:llama.cpp", "generic:openai-server"])
    #expect(after.footprintMB == record.footprintMB)
    #expect(after.name == record.name)
}

@Test func setStateIfPresentReturnsFalseWhenRecordMissing() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)

    let changed = try await registry.setStateIfPresent(id: "nonexistent", to: .ready)
    #expect(!changed)
}

@Test func listSortsByNameCaseInsensitively() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    try await registry.register(Fixtures.gguf(path: "/tmp/hedos-fixtures/zeta.gguf"))
    try await registry.register(Fixtures.flux())

    var zeta = Fixtures.gguf(path: "/tmp/hedos-fixtures/zeta.gguf")
    zeta.name = "Zeta"
    try await registry.register(zeta)

    let names = try await registry.list().map(\.name)
    #expect(names == ["FLUX.1-schnell", "Zeta"])
}
