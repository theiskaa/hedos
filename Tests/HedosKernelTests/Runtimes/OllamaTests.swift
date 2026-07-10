import Foundation
import Testing

@testable import HedosKernel


@Test func parserTranslatesDeltaLines() {
    let chunk = OllamaStreamParser.parse(
        line: #"{"model":"qwen3.5:9b","message":{"role":"assistant","content":"Hel"},"done":false}"#)
    #expect(chunk == .text("Hel"))
}

@Test func parserTranslatesFinalLineWithStats() {
    let line = #"""
    {"model":"qwen3.5:9b","done":true,"total_duration":2500000000,"prompt_eval_count":26,"eval_count":298}
    """#
    let chunk = OllamaStreamParser.parse(line: line)
    guard case .done(let stats) = chunk else {
        Issue.record("expected .done, got \(String(describing: chunk))")
        return
    }
    #expect(stats?.promptTokens == 26)
    #expect(stats?.completionTokens == 298)
    #expect(stats?.durationMs == 2500)
}

@Test func parserTranslatesRealCapturedLines() {
    let thinkingLine = #"""
    {"model":"qwen3.5:9b","created_at":"2026-07-05T19:18:15.612869Z","message":{"role":"assistant","content":"","thinking":"Thinking"},"done":false}
    """#
    #expect(OllamaStreamParser.parse(line: thinkingLine) == .thinking("Thinking"))

    let realDone = #"""
    {"model":"qwen3.5:9b","created_at":"2026-07-05T19:19:34.991772Z","done":true,"done_reason":"stop","total_duration":39306562500,"load_duration":141121625,"prompt_eval_count":18,"prompt_eval_duration":44281000,"eval_count":1796,"eval_duration":39113772000}
    """#
    guard case .done(let stats) = OllamaStreamParser.parse(line: realDone) else {
        Issue.record("expected .done")
        return
    }
    #expect(stats?.promptTokens == 18)
    #expect(stats?.completionTokens == 1796)
    #expect(stats?.durationMs == 39306)
}

@Test func parserIgnoresBlankEmptyAndMalformedLines() {
    #expect(OllamaStreamParser.parse(line: "") == nil)
    #expect(OllamaStreamParser.parse(line: "   ") == nil)
    #expect(OllamaStreamParser.parse(line: "{broken json") == nil)
    #expect(
        OllamaStreamParser.parse(
            line: #"{"message":{"role":"assistant","content":""},"done":false}"#) == nil)
}


private func ollamaRecord(name: String = "qwen3.5:9b") -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(kind: .ollama, path: "/fake/manifests/\(name)", repo: name),
        execution: .stream)
}

@Test func adapterCanServeMatrix() {
    let adapter = OllamaAdapter()
    #expect(adapter.canServe(ollamaRecord(), .chat))
    #expect(adapter.canServe(ollamaRecord(), .complete))
    #expect(adapter.canServe(ollamaRecord(), .embed))
    #expect(!adapter.canServe(ollamaRecord(), .image))
    #expect(!adapter.canServe(Fixtures.flux(), .chat))
    #expect(!adapter.canServe(Fixtures.flux(), .embed))
}

@Test func embedOnlyOllamaRecordRefusesChatInvoke() async throws {
    var embedRecord = ollamaRecord(name: "nomic-embed-text:latest")
    embedRecord.capabilities = [.embed]
    embedRecord.modality = .embedding

    let adapter = OllamaAdapter()
    #expect(!adapter.canServe(embedRecord, .chat))
    #expect(!adapter.canServe(embedRecord, .complete))
    #expect(adapter.canServe(embedRecord, .embed))

    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [adapter], secrets: InMemorySecretStore())
    embedRecord.state = .ready
    try await kernel.registry.register(embedRecord)

    do {
        _ = try await kernel.chat(
            embedRecord.id, messages: [ChatMessage(role: .user, content: "hi")])
        Issue.record("an embed-only record must refuse a chat invoke")
    } catch let KernelError.capabilityUnsupported(model, capability) {
        #expect(model == embedRecord.name)
        #expect(capability == .chat)
    }
}

@Test func adapterBuildsCorrectChatBody() throws {
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hello")])
        ])
    ])
    let data = try OllamaAdapter.requestBody(model: "qwen3.5:9b", payload: payload)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(body["model"] as? String == "qwen3.5:9b")
    #expect(body["stream"] as? Bool == true)
    let messages = body["messages"] as! [[String: Any]]
    #expect(messages.count == 1)
    #expect(messages[0]["content"] as? String == "hello")
}

@Test func adapterRejectsPayloadWithoutMessages() {
    #expect(throws: KernelError.self) {
        _ = try OllamaAdapter.requestBody(model: "m", payload: .object([:]))
    }
}

@Test func adapterBuildsCorrectEmbedBody() throws {
    let payload: JSONValue = .object(["input": .string("vectorize me")])
    let data = try OllamaAdapter.embedRequestBody(model: "nomic-embed-text", payload: payload)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(body["model"] as? String == "nomic-embed-text")
    #expect(body["input"] as? String == "vectorize me")
}

@Test func adapterEmbedBodyPassesThroughArrayInput() throws {
    let payload: JSONValue = .object([
        "input": .array([.string("a"), .string("b")])
    ])
    let data = try OllamaAdapter.embedRequestBody(model: "m", payload: payload)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(body["input"] as? [String] == ["a", "b"])
}

@Test func adapterRejectsEmbedPayloadWithoutInput() {
    #expect(throws: KernelError.self) {
        _ = try OllamaAdapter.embedRequestBody(model: "m", payload: .object([:]))
    }
}

@Test func adapterParsesEmbedResponseWithPromptTokens() throws {
    let json = Data(
        #"{"model":"nomic-embed-text","embeddings":[[0.1,0.2,0.3]],"prompt_eval_count":5}"#
            .utf8)
    let parsed = try OllamaAdapter.parseEmbedResponse(json)
    #expect(parsed.vectors == [[0.1, 0.2, 0.3]])
    #expect(parsed.promptTokens == 5)
}

@Test func adapterParsesBatchedEmbedResponse() throws {
    let json = Data(#"{"embeddings":[[0.1,0.2],[0.3,0.4]]}"#.utf8)
    let parsed = try OllamaAdapter.parseEmbedResponse(json)
    #expect(parsed.vectors == [[0.1, 0.2], [0.3, 0.4]])
    #expect(parsed.promptTokens == nil)
}

@Test func adapterNeverFakesAVectorOnErrorResponse() {
    let json = Data(#"{"error":"model does not support embeddings"}"#.utf8)
    #expect(throws: KernelError.self) {
        _ = try OllamaAdapter.parseEmbedResponse(json)
    }
}

@Test func adapterNeverFakesAVectorOnMalformedResponse() {
    let json = Data(#"{"unexpected":true}"#.utf8)
    #expect(throws: KernelError.self) {
        _ = try OllamaAdapter.parseEmbedResponse(json)
    }
}


private final class TerminationFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    func set() {
        lock.lock()
        value = true
        lock.unlock()
    }
    var isSet: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

private struct FakeAdapter: RuntimeAdapter {
    var id: String { "fake" }
    let chunks: [CapabilityChunk]
    let delayNs: UInt64
    let terminated: TerminationFlag
    var served: Capability = .chat

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        capability == served
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                for chunk in chunks {
                    try? await Task.sleep(nanoseconds: delayNs)
                    if Task.isCancelled { break }
                    continuation.yield(chunk)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in
                task.cancel()
                terminated.set()
            }
        }
    }
}

@Test func kernelRoutesAndRelaysChunksInOrder() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let flag = TerminationFlag()
    let kernel = Kernel(
        directory: dir,
        adapters: [
            FakeAdapter(
                chunks: [.text("a"), .text("b"), .done(nil)], delayNs: 0, terminated: flag)
        ])
    let record = ollamaRecord()
    try await kernel.registry.register(record)

    var received: [CapabilityChunk] = []
    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "hi")])
    for try await chunk in stream { received.append(chunk) }
    #expect(received == [.text("a"), .text("b"), .done(nil)])
}

@Test func cancellingConsumerTerminatesAdapterStream() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let flag = TerminationFlag()
    let many = Array(repeating: CapabilityChunk.text("x"), count: 200)
    let kernel = Kernel(
        directory: dir,
        adapters: [FakeAdapter(chunks: many, delayNs: 20_000_000, terminated: flag)])
    let record = ollamaRecord()
    try await kernel.registry.register(record)

    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "go")])
    let consumer = Task {
        var count = 0
        do {
            for try await _ in stream { count += 1 }
        } catch {}
        return count
    }
    try await Task.sleep(nanoseconds: 100_000_000)
    consumer.cancel()
    let consumed = await consumer.value
    #expect(consumed < 200)

    for _ in 0..<50 where !flag.isSet {
        try await Task.sleep(nanoseconds: 20_000_000)
    }
    #expect(flag.isSet)
}

@Test func kernelThrowsForUnknownModelAndUnsupportedCapability() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [OllamaAdapter()])

    await #expect(throws: KernelError.self) {
        _ = try await kernel.chat("nonexistent", messages: [])
    }

    let record = Fixtures.flux()
    try await kernel.registry.register(record)
    await #expect(throws: KernelError.self) {
        _ = try await kernel.chat(record.id, messages: [])
    }
}

@Test func kernelInvokeReturnsVectorForEmbedCapability() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let flag = TerminationFlag()
    let kernel = Kernel(
        directory: dir,
        adapters: [
            FakeAdapter(
                chunks: [.vector([0.1, 0.2, 0.3]), .done(nil)], delayNs: 0, terminated: flag,
                served: .embed)
        ])
    let record = ollamaRecord()
    try await kernel.registry.register(record)

    let stream = try await kernel.invoke(
        record.id, .embed, payload: .object(["input": .string("hello")]))
    var received: [CapabilityChunk] = []
    for try await chunk in stream { received.append(chunk) }
    #expect(received == [.vector([0.1, 0.2, 0.3]), .done(nil)])
}
