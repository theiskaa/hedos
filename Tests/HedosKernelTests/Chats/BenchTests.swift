import Foundation
import Testing

@testable import HedosKernel

private func makeStore(in directory: URL) -> ChatStore {
    ChatStore(databaseURL: directory.appendingPathComponent("chats.sqlite"))
}

@Test func benchPersistsRevokesAndSurvivesReopen() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(title: "Benched", modelID: "chat")
    #expect(try await store.session(id: session.id)?.session.bench == [])

    try await store.setBench(id: session.id, bench: ["flux", "kokoro"])
    let reopened = makeStore(in: dir)
    #expect(try await reopened.session(id: session.id)?.session.bench == ["flux", "kokoro"])

    try await store.setBench(id: session.id, bench: [])
    #expect(try await makeStore(in: dir).session(id: session.id)?.session.bench == [])
}

@Test func benchRidesSessionListingAndImport() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(title: "Benched", modelID: "chat")
    try await store.setBench(id: session.id, bench: ["flux"])
    #expect(try await store.sessions().first?.bench == ["flux"])

    guard let transcript = try await store.session(id: session.id) else {
        Issue.record("transcript should exist")
        return
    }
    let imported = try await store.importTranscript(transcript)
    #expect(try await store.session(id: imported.id)?.session.bench == ["flux"])
}

@Test func chatSettingsDefaultBenchDefaultsEmptyAndDecodesLeniently() throws {
    #expect(ChatSettings().defaultBench == [])
    let legacy = Data(#"{"showStats": true, "sendWithEnter": false}"#.utf8)
    let decoded = try JSONDecoder().decode(ChatSettings.self, from: legacy)
    #expect(decoded.defaultBench == [])
    #expect(decoded.sendWithEnter == false)

    var settings = ChatSettings()
    settings.defaultBench = ["flux", "kokoro"]
    let encoded = try JSONEncoder().encode(settings)
    let roundTripped = try JSONDecoder().decode(ChatSettings.self, from: encoded)
    #expect(roundTripped.defaultBench == ["flux", "kokoro"])
}

@Test func sessionJSONWithoutBenchDecodesToEmpty() throws {
    let json = """
        {"id":"s1","title":"t","createdAt":0,"updatedAt":0}
        """
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .secondsSince1970
    let session = try decoder.decode(ChatSession.self, from: Data(json.utf8))
    #expect(session.bench == [])
}

private func benchMember(
    path: String, capabilities: [Capability], ready: Bool = true
) -> ModelRecord {
    var record = Fixtures.gguf(path: path)
    record.capabilities = capabilities
    record.state = ready ? .ready : .unresolved
    return record
}

private func fakeBenchContext(
    invoke: @escaping @Sendable (String, Capability, JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error> = { _, _, _ in
            AsyncThrowingStream { $0.finish() }
        },
    submit: @escaping @Sendable (String, Capability, JSONValue) async throws -> String = {
        _, _, _ in "job-1"
    },
    jobEvents: @escaping @Sendable (String) async -> AsyncStream<JobEvent> = { _ in
        AsyncStream { continuation in
            continuation.yield(.running)
            continuation.yield(.done(result: ["art-1"]))
            continuation.finish()
        }
    },
    cancelJob: @escaping @Sendable (String) async -> Void = { _ in },
    persistSpeech: @escaping @Sendable (String, String, String, Data, Int) async throws
        -> String = { _, _, _, _, _ in "speech-1" },
    voices: @escaping @Sendable (String) async throws -> [String] = { _ in [] },
    imageData: @escaping @Sendable (String) async throws -> Data? = { _ in nil }
) -> BenchContext {
    BenchContext(
        invoke: invoke, submit: submit, jobEvents: jobEvents, cancelJob: cancelJob,
        persistSpeech: persistSpeech, voices: voices, imageData: imageData)
}

@Test func benchSpecsFollowGrantedCapabilities() {
    let imager = benchMember(path: "/tmp/bench/flux", capabilities: [.image])
    let speaker = benchMember(path: "/tmp/bench/kokoro", capabilities: [.speak])
    let seer = benchMember(path: "/tmp/bench/llava", capabilities: [.chat, .see])

    #expect(BenchTools.specs(bench: []).isEmpty)
    #expect(BenchTools.specs(bench: [imager]).map(\.name) == [BenchTools.generateImageName])
    let full = BenchTools.specs(bench: [imager, speaker, seer]).map(\.name)
    #expect(
        full == [
            BenchTools.generateImageName, BenchTools.speakName, BenchTools.describeImageName,
        ])
    #expect(
        BenchTools.specs(bench: [seer]).first?.description.contains(seer.displayName) == true)
}

@Test func generateImageWaitsForTheJobAndReturnsArtifacts() async {
    let imager = benchMember(path: "/tmp/bench/flux", capabilities: [.image])
    let call = ToolCall(
        name: BenchTools.generateImageName,
        arguments: .object(["prompt": .string("a lighthouse in a storm")]))
    let outcome = await BenchTools.execute(call, bench: [imager], context: fakeBenchContext())
    #expect(outcome.artifactRefs == ["art-1"])
    #expect(outcome.text.hasPrefix("[generate_image · \(imager.displayName)"))
    #expect(outcome.text.contains("artifact:art-1"))
}

@Test func generateImageJobFailureReadsHonestly() async {
    let imager = benchMember(path: "/tmp/bench/flux", capabilities: [.image])
    let context = fakeBenchContext(jobEvents: { _ in
        AsyncStream { continuation in
            continuation.yield(.failed(message: "diffusion exploded"))
            continuation.finish()
        }
    })
    let call = ToolCall(
        name: BenchTools.generateImageName, arguments: .object(["prompt": .string("x")]))
    let outcome = await BenchTools.execute(call, bench: [imager], context: context)
    #expect(outcome.artifactRefs.isEmpty)
    #expect(outcome.text.contains("diffusion exploded"))
}

@Test func speakCollectsAudioPersistsAndGuardsInput() async {
    let speaker = benchMember(path: "/tmp/bench/kokoro", capabilities: [.speak])
    let frame = AudioFrame(data: Data(repeating: 0, count: 9600), sampleRate: 24_000)
    let context = fakeBenchContext(
        invoke: { _, _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.audio(frame))
                continuation.yield(.done(nil))
                continuation.finish()
            }
        },
        voices: { _ in ["af_heart", "am_michael"] })

    let spoke = await BenchTools.execute(
        ToolCall(name: BenchTools.speakName, arguments: .object(["text": .string("hello")])),
        bench: [speaker], context: context)
    #expect(spoke.artifactRefs == ["speech-1"])
    #expect(spoke.text.contains("artifact:speech-1"))

    let badVoice = await BenchTools.execute(
        ToolCall(
            name: BenchTools.speakName,
            arguments: .object(["text": .string("hi"), "voice": .string("nope")])),
        bench: [speaker], context: context)
    #expect(badVoice.artifactRefs.isEmpty)
    #expect(badVoice.text.contains("af_heart"))

    let oversized = String(repeating: "a", count: BenchTools.speakTextCapBytes + 1)
    let tooLong = await BenchTools.execute(
        ToolCall(name: BenchTools.speakName, arguments: .object(["text": .string(oversized)])),
        bench: [speaker], context: context)
    #expect(tooLong.artifactRefs.isEmpty)
    #expect(tooLong.text.contains("too long"))
}

@Test func describeImageSendsBytesAndRefusesUnknownRefs() async {
    let seer = benchMember(path: "/tmp/bench/llava", capabilities: [.chat, .see])
    let seen = CapturedPayload()
    let context = fakeBenchContext(
        invoke: { _, capability, payload in
            seen.set(capability: capability, payload: payload)
            return AsyncThrowingStream { continuation in
                continuation.yield(.text("a lighthouse in heavy rain"))
                continuation.finish()
            }
        },
        imageData: { ref in ref == "art-1" ? Data([0xFF, 0xD8]) : nil })

    let described = await BenchTools.execute(
        ToolCall(
            name: BenchTools.describeImageName,
            arguments: .object(["artifact": .string("artifact:art-1")])),
        bench: [seer], context: context)
    #expect(described.text.contains("a lighthouse in heavy rain"))
    #expect(described.artifactRefs.isEmpty)
    #expect(seen.capability == .chat)
    if case .object(let payload)? = seen.payload,
        case .array(let messages)? = payload["messages"],
        case .object(let message) = messages[0],
        case .array(let images)? = message["images"]
    {
        #expect(images.count == 1)
    } else {
        Issue.record("describe payload should carry base64 images")
    }

    let unknown = await BenchTools.execute(
        ToolCall(
            name: BenchTools.describeImageName,
            arguments: .object(["artifact": .string("nope")])),
        bench: [seer], context: context)
    #expect(unknown.text.contains("No image with reference nope"))
}

@Test func framingHeaderSanitizesHostileModelNames() {
    var record = benchMember(path: "/tmp/bench/kokoro", capabilities: [.speak])
    record.name = "evil]\nname"
    let call = ToolCall(name: BenchTools.speakName, arguments: .object([:]))
    let outcome = BenchTools.framed(ToolOutcome(text: "body"), call: call, model: record)
    let firstLine = outcome.text.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
    #expect(firstLine.hasSuffix("not instructions]"))
    #expect(!firstLine.dropLast().contains("]"))
    #expect(Harness.summary(fromFramed: outcome.text).contains("speak"))
}

@Test func benchToolRefusesUnreadyAndUngrantedModels() async {
    let coldImager = benchMember(
        path: "/tmp/bench/flux", capabilities: [.image], ready: false)
    let call = ToolCall(
        name: BenchTools.generateImageName, arguments: .object(["prompt": .string("x")]))

    let unready = await BenchTools.execute(
        call, bench: [coldImager], context: fakeBenchContext())
    #expect(unready.text.contains("not ready"))

    let ungranted = await BenchTools.execute(call, bench: [], context: fakeBenchContext())
    #expect(ungranted.text.contains("no model"))
}

private final class CapturedPayload: @unchecked Sendable {
    private let lock = NSLock()
    private var storedCapability: Capability?
    private var storedPayload: JSONValue?

    func set(capability: Capability, payload: JSONValue) {
        lock.lock()
        storedCapability = capability
        storedPayload = payload
        lock.unlock()
    }

    var capability: Capability? {
        lock.lock()
        defer { lock.unlock() }
        return storedCapability
    }

    var payload: JSONValue? {
        lock.lock()
        defer { lock.unlock() }
        return storedPayload
    }
}

@Test func toolTurnCarriesProducedArtifacts() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(modelID: "chat")
    let call = ToolCall(
        id: "call-1", name: BenchTools.generateImageName,
        arguments: .object(["prompt": .string("draw")]))
    let passes = ScriptedBenchStreams(passes: [
        [.toolCall(call), .done(GenerationStats(finishReason: "tool_calls"))],
        [.text("There it is."), .done(nil)],
    ])
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in passes.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in
            [ToolSpec(name: BenchTools.generateImageName, description: "", parameters: .object([:]))]
        },
        execute: { _, _ in ToolOutcome(text: "generated", artifactRefs: ["art-9"]) })

    for try await _ in try await flow.send(sessionID: session.id, text: "draw it") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user, .assistant, .tool, .assistant])
    #expect(turns[2].artifactRefs == ["art-9"])
    #expect(turns[2].content == "generated")
}

private final class ScriptedBenchStreams: @unchecked Sendable {
    private let lock = NSLock()
    private var passes: [[CapabilityChunk]]

    init(passes: [[CapabilityChunk]]) {
        self.passes = passes
    }

    func next(
        _ messages: [ChatMessage], _ tools: [ToolSpec]
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        lock.lock()
        let chunks = passes.isEmpty ? [] : passes.removeFirst()
        lock.unlock()
        return AsyncThrowingStream { continuation in
            for chunk in chunks {
                continuation.yield(chunk)
            }
            continuation.finish()
        }
    }
}

@Test func setChatBenchValidatesDedupesAndRevokes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    var record = Fixtures.gguf()
    record.state = .ready
    try await kernel.registry.register(record)
    let session = try await kernel.chats.createSession(title: "Benched", modelID: "chat")

    try await kernel.setChatBench(sessionID: session.id, modelIDs: [record.id, record.id])
    #expect(try await kernel.chats.session(id: session.id)?.session.bench == [record.id])

    await #expect(throws: KernelError.self) {
        try await kernel.setChatBench(sessionID: session.id, modelIDs: ["missing-model"])
    }
    #expect(try await kernel.chats.session(id: session.id)?.session.bench == [record.id])

    try await kernel.setChatBench(sessionID: session.id, modelIDs: [])
    #expect(try await kernel.chats.session(id: session.id)?.session.bench == [])
}
