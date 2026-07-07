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

    mutating func addUInt32(key: String, value: UInt32) {
        appendString(key)
        append(UInt32(4))
        append(value)
    }

    mutating func addStringArray(key: String, values: [String]) {
        appendString(key)
        append(UInt32(9))
        append(UInt32(8))
        append(UInt64(values.count))
        for value in values {
            appendString(value)
        }
    }
}

private func writeGGUF(architecture: String, at dir: URL, name: String) throws -> URL {
    var builder = GGUFHeaderBuilder(keyValueCount: 3)
    builder.addUInt32(key: "general.alignment", value: 32)
    builder.addStringArray(key: "general.tags", values: ["speech", "asr"])
    builder.addString(key: "general.architecture", value: architecture)
    builder.data.append(DiscoveryFixtures.data(bytes: 64))
    let url = dir.appendingPathComponent(name)
    try builder.data.write(to: url)
    return url
}

private func record(at url: URL, name: String) -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .unknown,
        capabilities: [],
        source: ModelSource(kind: .file, path: url.path),
        execution: .stream,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func identifiesWhisperGGUFHeaderAsTranscribe() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try writeGGUF(architecture: "whisper", at: dir, name: "whisper-tiny.gguf")

    let identified = Identification.identify(record(at: url, name: "whisper-tiny"))
    #expect(identified.format == .gguf)
    #expect(identified.modality == .audio)
    #expect(identified.capabilities == [.transcribe])
    #expect(identified.execution == .stream)
}

@Test func identifiesTextArchitectureGGUFAsChat() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try writeGGUF(architecture: "llama", at: dir, name: "llama-tiny.gguf")

    let identified = Identification.identify(record(at: url, name: "llama-tiny"))
    #expect(identified.format == .gguf)
    #expect(identified.modality == .text)
    #expect(identified.capabilities == [.chat, .complete])
}

@Test func ggufWithGarbageHeaderStaysText() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("garbage.gguf")
    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 64))
    try payload.write(to: url)

    #expect(Identification.ggufGeneralArchitecture(at: url) == nil)
    let identified = Identification.identify(record(at: url, name: "garbage"))
    #expect(identified.modality == .text)
    #expect(identified.capabilities == [.chat, .complete])
}

@Test func engineResolvesWhisperGGUFToWhisperCppManaged() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try writeGGUF(architecture: "whisper", at: dir, name: "whisper-base.gguf")
    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let whisper = record(at: url, name: "whisper-base")
    try await registry.register(whisper)

    let engine = ResolutionEngine(
        adapters: [LlamaCppAdapter(), WhisperCppAdapter(), OllamaAdapter()])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: whisper.id))
    #expect(resolved.runtime.id == "whisper-cpp")
    #expect(resolved.runtime.tier == .managed)
    #expect(resolved.runtime.resolved == .auto)
    #expect(resolved.state == .ready)
    #expect(resolved.modality == .audio)
    #expect(resolved.capabilities == [.transcribe])
    #expect(resolved.execution == .stream)
}

@Test func whisperAdapterBidMatrix() {
    let whisperAdapter = WhisperCppAdapter()
    let llamaAdapter = LlamaCppAdapter()
    let transcribeGGUF = IdentifiedModel(
        format: .gguf, modality: .audio, capabilities: [.transcribe], execution: .stream)
    let chatGGUF = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat, .complete], execution: .stream)
    let transcribeSafetensors = IdentifiedModel(
        format: .safetensors, modality: .audio, capabilities: [.transcribe], execution: .stream)
    let record = Fixtures.gguf()

    #expect(whisperAdapter.bid(record, transcribeGGUF)?.tier == .managed)
    #expect(whisperAdapter.bid(record, chatGGUF) == nil)
    #expect(whisperAdapter.bid(record, transcribeSafetensors) == nil)
    #expect(llamaAdapter.bid(record, transcribeGGUF) == nil)
    #expect(llamaAdapter.bid(record, chatGGUF) != nil)
}

@Test func whisperAdapterServesOnlyResolvedTranscribe() {
    let adapter = WhisperCppAdapter()
    var whisper = Fixtures.gguf(path: "~/Downloads/whisper-tiny.gguf")
    #expect(!adapter.canServe(whisper, .transcribe))
    whisper.runtime = RuntimeRef(id: "whisper-cpp", resolved: .auto, tier: .native)
    #expect(adapter.canServe(whisper, .transcribe))
    #expect(!adapter.canServe(whisper, .chat))
}
