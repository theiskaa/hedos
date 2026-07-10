import Foundation
import Testing

@testable import HedosKernel

@Test func capturesContextLengthAndTemplatePresence() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("llama.gguf")
    var builder = GGUFFixtureBuilder(keyValueCount: 3)
    builder.addString(key: "general.architecture", value: "llama")
    builder.addUInt32(key: "llama.context_length", value: 32768)
    builder.addString(key: "tokenizer.chat_template", value: "{% for m in messages %}{% endfor %}")
    try builder.write(to: url)

    let facts = try #require(Identification.ggufFacts(at: url))
    #expect(facts.architecture == "llama")
    #expect(facts.contextLength == 32768)
    #expect(facts.hasChatTemplate)
}

@Test func readsUInt64ContextLength() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("qwen3.gguf")
    var builder = GGUFFixtureBuilder(keyValueCount: 2)
    builder.addString(key: "general.architecture", value: "qwen3")
    builder.addUInt64(key: "qwen3.context_length", value: 131072)
    try builder.write(to: url)

    let facts = try #require(Identification.ggufFacts(at: url))
    #expect(facts.contextLength == 131072)
    #expect(facts.hasChatTemplate == false)
}

@Test func headerWithoutContextKeysYieldsNilLength() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("bare.gguf")
    var builder = GGUFFixtureBuilder(keyValueCount: 1)
    builder.addString(key: "general.architecture", value: "llama")
    try builder.write(to: url)

    let facts = try #require(Identification.ggufFacts(at: url))
    #expect(facts.architecture == "llama")
    #expect(facts.contextLength == nil)
    #expect(facts.hasChatTemplate == false)
}

@Test func garbageHeaderYieldsNilFacts() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("garbage.gguf")
    try DiscoveryFixtures.data(bytes: 128, fill: 0x5A).write(to: url)

    #expect(Identification.ggufFacts(at: url) == nil)
}

@Test func identifyCarriesContextLengthOntoIdentifiedModel() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("chatty.gguf")
    var builder = GGUFFixtureBuilder(keyValueCount: 3)
    builder.addString(key: "general.architecture", value: "llama")
    builder.addUInt32(key: "llama.context_length", value: 8192)
    builder.addString(key: "tokenizer.chat_template", value: "{{ messages }}")
    try builder.write(to: url)

    let record = ModelRecord(
        name: "chatty", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .file, path: url.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .gguf)
    #expect(identified.contextLength == 8192)
    #expect(identified.hasChatTemplate == true)
    #expect(identified.capabilities == [.chat, .complete])
}
