import Foundation
import Testing

@testable import HedosKernel

private func chatPipeline(name: String = "greeter", modelID: String = "m1") -> Pipeline {
    Pipeline(
        name: name,
        stages: [PipelineStage(modelID: modelID, capability: .chat)])
}

@Test func pipelineStoreRoundTrip() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PipelineStore(directory: dir)
    let saved = try await store.save(chatPipeline())

    let reloaded = PipelineStore(directory: dir)
    let listed = await reloaded.list()
    #expect(listed.count == 1)
    #expect(listed[0].id == saved.id)
    #expect(listed[0].name == "greeter")
    #expect(await reloaded.get(id: saved.id)?.stages.first?.capability == .chat)
}

@Test func pipelineStoreStampsUpdatedAt() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PipelineStore(directory: dir)
    var pipeline = chatPipeline()
    pipeline.updatedAt = Date(timeIntervalSince1970: 0)
    let saved = try await store.save(pipeline)
    #expect(saved.updatedAt.timeIntervalSince1970 > 0)
}

@Test func pipelineStoreDeleteRemovesFile() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PipelineStore(directory: dir)
    let saved = try await store.save(chatPipeline())
    await store.delete(id: saved.id)
    #expect(await store.get(id: saved.id) == nil)
    #expect(
        !FileManager.default.fileExists(
            atPath: dir.appendingPathComponent("\(saved.id).json").path))
}

@Test func kernelSavePipelineValidatesAgainstShelf() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [], secrets: InMemorySecretStore())

    var chat = Fixtures.gguf(path: "/tmp/hedos-fixtures/chat.gguf")
    chat.name = "gemma"
    chat.capabilities = [.chat]
    chat.state = .ready
    try await kernel.registry.register(chat)

    let valid = Pipeline(
        name: "ok", stages: [PipelineStage(modelID: chat.id, capability: .chat)])
    _ = try await kernel.pipelineStore.save(valid)
    #expect(await kernel.pipelineStore.list().count == 1)

    let invalid = Pipeline(
        name: "bad", stages: [PipelineStage(modelID: chat.id, capability: .speak)])
    await #expect(throws: PipelineValidationError.self) {
        try await kernel.pipelineStore.save(invalid)
    }
    #expect(await kernel.pipelineStore.list().count == 1)
}

@Test func shelfProvidedStoreRejectsAPipelineNamingAMissingModel() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PipelineStore(directory: dir, shelf: { [] })

    await #expect(throws: PipelineValidationError.self) {
        try await store.save(chatPipeline(modelID: "nowhere"))
    }
    #expect(await store.list().isEmpty)

    var record = Fixtures.gguf()
    record.state = .ready
    record.capabilities = [.chat]
    let ready = record
    let stocked = PipelineStore(directory: dir, shelf: { [ready] })
    let saved = try await stocked.save(chatPipeline(modelID: ready.id))
    #expect(await stocked.signature(of: saved) != nil)
}
