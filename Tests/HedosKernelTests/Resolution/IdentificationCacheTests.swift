import Foundation
import Testing

@testable import HedosKernel

private struct GGUFHeaderBuilder {
    var data = Data("GGUF".utf8)

    init(keyValueCount: Int) {
        append(UInt32(3))
        append(UInt64(0))
        append(UInt64(keyValueCount))
    }

    mutating func append<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
    }

    mutating func appendString(_ string: String) {
        append(UInt64(string.utf8.count))
        data.append(Data(string.utf8))
    }

    mutating func addString(key: String, value: String) {
        appendString(key)
        append(UInt32(8))
        appendString(value)
    }
}

private func writeGGUF(architecture: String, to url: URL) throws {
    var builder = GGUFHeaderBuilder(keyValueCount: 1)
    builder.addString(key: "general.architecture", value: architecture)
    builder.data.append(DiscoveryFixtures.data(bytes: 64))
    try builder.data.write(to: url)
}

private func fileRecord(at url: URL) -> ModelRecord {
    ModelRecord(
        name: url.deletingPathExtension().lastPathComponent,
        modality: .unknown,
        capabilities: [],
        source: ModelSource(kind: .file, path: url.path),
        execution: .stream,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func unchangedFileIsServedFromCache() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("model.gguf")
    try writeGGUF(architecture: "whisper", to: url)
    let cache = IdentificationCache()
    let record = fileRecord(at: url)

    let first = cache.identify(record)
    #expect(cache.hitCount == 0)
    let second = cache.identify(record)
    #expect(cache.hitCount == 1)
    #expect(first == second)
    #expect(first.capabilities == [.transcribe])
}

@Test func modifiedFileReIdentifies() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("model.gguf")
    try writeGGUF(architecture: "whisper", to: url)
    let cache = IdentificationCache()
    let record = fileRecord(at: url)

    #expect(cache.identify(record).modality == .audio)
    try writeGGUF(architecture: "llama", to: url)
    let reIdentified = cache.identify(record)
    #expect(cache.hitCount == 0)
    #expect(reIdentified.modality == .text)
    #expect(reIdentified.capabilities == [.chat, .complete])
}

@Test func uncachedKindsBypassTheCache() {
    let cache = IdentificationCache()
    let record = ModelRecord(
        name: "gemma4:latest", modality: .text, capabilities: [.chat],
        source: ModelSource(kind: .ollama, path: "/fake", repo: "gemma4:latest"))

    let first = cache.identify(record)
    let second = cache.identify(record)
    #expect(cache.hitCount == 0)
    #expect(first == second)
}

@Test func explainServesIdentificationFromTheCache() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("model.gguf")
    try writeGGUF(architecture: "llama", to: url)
    let cache = IdentificationCache()
    let record = fileRecord(at: url)
    let engine = ResolutionEngine(adapters: [LlamaCppAdapter()], identificationCache: cache)

    _ = await engine.explain(record)
    #expect(cache.hitCount == 0)
    _ = await engine.explain(record)
    #expect(cache.hitCount == 1)
}
