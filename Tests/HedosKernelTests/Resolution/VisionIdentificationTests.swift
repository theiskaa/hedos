import Foundation
import Testing

@testable import HedosKernel

private func fileRecord(at url: URL) -> ModelRecord {
    ModelRecord(
        name: url.deletingPathExtension().lastPathComponent,
        modality: .unknown,
        capabilities: [],
        source: ModelSource(kind: .file, path: url.path),
        execution: .stream,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func visionArchitectureGGUFCarriesSeeAlongsideChat() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try DiscoveryFixtures.makeGGUF(
        architecture: "qwen2vl", at: dir, name: "qwen2-vl-7b.gguf")

    let identified = Identification.identify(fileRecord(at: url))
    #expect(identified.format == .gguf)
    #expect(identified.modality == .text)
    #expect(identified.capabilities == [.chat, .complete, .see])
    #expect(identified.execution == .stream)
}

@Test func ggufWithMmprojCompanionCarriesSee() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let main = try DiscoveryFixtures.makeGGUF(
        architecture: "llama", at: dir, name: "llava-v1.6-7b.gguf")
    _ = try DiscoveryFixtures.makeGGUF(
        architecture: "clip", at: dir, name: "mmproj-llava-v1.6.gguf")

    let identified = Identification.identify(fileRecord(at: main))
    #expect(identified.capabilities == [.chat, .complete, .see])
    #expect(identified.modality == .text)
}

@Test func mmprojFileIsNotItsOwnChatModel() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let mmproj = try DiscoveryFixtures.makeGGUF(
        architecture: "clip", at: dir, name: "mmproj-llava-v1.6.gguf")
    _ = try DiscoveryFixtures.makeGGUF(
        architecture: "llama", at: dir, name: "llava-v1.6-7b.gguf")

    let identified = Identification.identify(fileRecord(at: mmproj))
    #expect(identified.modality == .vision)
    #expect(identified.capabilities.isEmpty)
    #expect(identified.execution == .sync)

    let loose = await LooseFileScanner(directories: [dir]).scan()
    #expect(loose.discovered.map(\.name) == ["llava-v1.6-7b"])

    let lmstudio = await LMStudioScanner(roots: [dir]).scan()
    #expect(lmstudio.discovered.map(\.name) == ["llava-v1.6-7b"])
}

@Test func seeTaggedGGUFResolvesReadyForChatOnly() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let url = try DiscoveryFixtures.makeGGUF(
        architecture: "qwen2vl", at: dir, name: "qwen2-vl-7b.gguf")
    let record = fileRecord(at: url)
    try await registry.register(record)

    try await ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter()])
        .resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.state == .ready)
    #expect(resolved.runtime.id == "llama-cpp")
    #expect(resolved.capabilities == [.chat, .complete, .see])

    let adapter = LlamaCppAdapter()
    #expect(adapter.canServe(resolved, .chat))
    #expect(!adapter.canServe(resolved, .see))
}
