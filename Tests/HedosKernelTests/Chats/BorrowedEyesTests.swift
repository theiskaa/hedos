import Foundation
import Testing

@testable import HedosKernel

private let pngBytes = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3])

private func imageAttachment(_ byte: UInt8 = 1) -> ChatAttachment {
    ChatAttachment(
        kind: .image, data: pngBytes + Data([byte]), mimeType: "image/png",
        name: "Pasted image")
}

private func documentAttachment() -> ChatAttachment {
    ChatAttachment(
        kind: .document, data: Data("hello".utf8), mimeType: "text/plain", name: "notes.txt")
}

@Test func systemBlockListsOnlyOfferedBenchTools() throws {
    #expect(BenchTools.systemBlock(tools: []) == nil)
    #expect(BenchTools.systemBlock(tools: HarnessTools.specs()) == nil)

    let imager = ToolSpec(
        name: BenchTools.generateImageName, description: "", parameters: .object([:]))
    let speaker = ToolSpec(name: BenchTools.speakName, description: "", parameters: .object([:]))
    let single = try #require(BenchTools.systemBlock(tools: [imager]))
    #expect(single.contains(BenchTools.generateImageName))
    #expect(!single.contains(BenchTools.speakName))
    let both = try #require(BenchTools.systemBlock(tools: [imager, speaker]))
    #expect(both.contains("never say you cannot generate images"))
    #expect(both.contains("never say you cannot produce audio"))
}

@Test func borrowedEyesStripsImagesAndLeavesMarkers() {
    let image = imageAttachment()
    let ref = AttachmentStore.ref(for: image.data, mimeType: image.mimeType, name: image.name)
    let message = ChatMessage(
        role: .user, content: "what is this?",
        attachments: [documentAttachment(), image],
        attachmentRefs: ["doc-ref", ref])
    let plain = ChatMessage(role: .assistant, content: "an answer")

    let transformed = BenchTools.borrowedEyes(messages: [message, plain])
    #expect(transformed[1] == plain)
    let stripped = transformed[0]
    #expect(stripped.attachments.map(\.kind) == [.document])
    #expect(stripped.attachmentRefs == ["doc-ref"])
    #expect(stripped.content.contains("what is this?"))
    #expect(stripped.content.contains(ref))
    #expect(stripped.content.contains(BenchTools.describeImageName))
    if case .object(let payload) = stripped.payloadValue {
        #expect(payload["images"] == nil)
    } else {
        Issue.record("payload should be an object")
    }
}

@Test func borrowedEyesToleratesRefLessMessages() {
    let message = ChatMessage(
        role: .user, content: "look", attachments: [imageAttachment()])
    let stripped = BenchTools.borrowedEyes(messages: [message])[0]
    #expect(stripped.attachments.isEmpty)
    #expect(stripped.content.contains("no reference is available"))
}

@Test func mergedAppendsBlockToExistingSystemTurnWithoutDuplicating() {
    var record = Fixtures.gguf()
    record.state = .ready
    let turns: [JSONValue] = [
        .object(["role": .string("system"), "content": .string("be terse")]),
        .object(["role": .string("user"), "content": .string("hi")]),
    ]
    let merged = ModelConfiguration.merged(
        record: record, capability: .chat,
        payload: .object(["messages": .array(turns)]),
        appendedBlock: "use your bench")
    guard case .object(let fields) = merged, case .array(let out)? = fields["messages"],
        case .object(let system) = out[0], case .string(let content)? = system["content"]
    else {
        Issue.record("merged payload should keep messages")
        return
    }
    #expect(out.count == 2)
    #expect(content == "be terse\n\nuse your bench")

    let inserted = ModelConfiguration.merged(
        record: record, capability: .chat,
        payload: .object(["messages": .array([turns[1]])]),
        sessionPrompt: "be kind", appendedBlock: "use your bench")
    guard case .object(let insertedFields) = inserted,
        case .array(let insertedOut)? = insertedFields["messages"],
        case .object(let insertedSystem) = insertedOut[0],
        case .string(let insertedContent)? = insertedSystem["content"]
    else {
        Issue.record("merged payload should insert a system turn")
        return
    }
    #expect(insertedOut.count == 2)
    #expect(insertedContent == "be kind\n\nuse your bench")
}

private final class ChatPayloadBox: @unchecked Sendable {
    private let lock = NSLock()
    private(set) var payloads: [JSONValue] = []

    func record(_ payload: JSONValue) {
        lock.withLock { payloads.append(payload) }
    }

    var lastMessages: [JSONValue] {
        lock.withLock {
            guard case .object(let fields)? = payloads.last,
                case .array(let messages)? = fields["messages"]
            else { return [] }
            return messages
        }
    }
}

private struct ChatOnlyAdapter: RuntimeAdapter {
    let box: ChatPayloadBox

    var id: RuntimeID { "ollama" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        capability == .chat
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        box.record(payload)
        return AsyncThrowingStream { continuation in
            continuation.yield(.text("ok"))
            continuation.finish()
        }
    }
}

private func chatKernel(in dir: URL) async throws -> (Kernel, ChatPayloadBox, ModelRecord) {
    let box = ChatPayloadBox()
    let kernel = Kernel(
        directory: dir, adapters: [ChatOnlyAdapter(box: box)], secrets: InMemorySecretStore())
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: "ollama", resolved: .user, tier: .native)
    record.state = .ready
    try await kernel.registry.register(record)
    return (kernel, box, record)
}

private func describeSpec() -> ToolSpec {
    ToolSpec(name: BenchTools.describeImageName, description: "", parameters: .object([:]))
}

@Test func chatStripsImagesWhenDescribeImageOfferedToBlindModel() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, box, record) = try await chatKernel(in: dir)

    let image = imageAttachment()
    let ref = AttachmentStore.ref(for: image.data, mimeType: image.mimeType, name: image.name)
    let message = ChatMessage(
        role: .user, content: "what is this?", attachments: [image], attachmentRefs: [ref])
    let stream = try await kernel.chat(record.id, messages: [message], tools: [describeSpec()])
    for try await _ in stream {}

    guard case .object(let fields) = box.lastMessages.last else {
        Issue.record("chat payload should carry the user message")
        return
    }
    #expect(fields["images"] == nil)
    if case .string(let content)? = fields["content"] {
        #expect(content.contains(ref))
        #expect(content.contains(BenchTools.describeImageName))
    } else {
        Issue.record("content should carry the marker")
    }
}

@Test func chatWithoutDescribeImageStillStripsWithAnHonestMarker() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, box, record) = try await chatKernel(in: dir)

    let image = imageAttachment()
    let ref = AttachmentStore.ref(for: image.data, mimeType: image.mimeType, name: image.name)
    let message = ChatMessage(
        role: .user, content: "look", attachments: [image], attachmentRefs: [ref])
    let stream = try await kernel.chat(record.id, messages: [message], tools: [])
    for try await _ in stream {}

    guard case .object(let fields) = box.lastMessages.last,
        case .string(let content)? = fields["content"]
    else {
        Issue.record("chat payload should carry the user message")
        return
    }
    #expect(fields["images"] == nil)
    #expect(content.contains(ref))
    #expect(content.contains("no vision model is available"))
}

@Test func chatSeedsTheBenchBlockBehindTheSessionPrompt() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, box, record) = try await chatKernel(in: dir)

    let imager = ToolSpec(
        name: BenchTools.generateImageName, description: "", parameters: .object([:]))
    let stream = try await kernel.chat(
        record.id, messages: [ChatMessage(role: .user, content: "draw a koala")],
        tools: [imager], systemPromptOverride: "be terse")
    for try await _ in stream {}

    guard case .object(let system) = box.lastMessages.first,
        system["role"] == .string("system"),
        case .string(let content)? = system["content"]
    else {
        Issue.record("payload should open with a system turn")
        return
    }
    #expect(content.hasPrefix("be terse"))
    #expect(content.contains("never say you cannot generate images"))
}

@Test func benchImageDataResolvesOwnSessionAttachmentsOnly() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    let store = AttachmentStore(
        directory: dir.appendingPathComponent("attachments", isDirectory: true))

    let session = try await kernel.chats.createSession(modelID: "chat")
    let other = try await kernel.chats.createSession(modelID: "chat")
    let refs = try store.store([imageAttachment(), documentAttachment()])
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .user, content: "look", attachmentRefs: refs), to: session.id)

    let context = await kernel.benchContext(sessionID: session.id)
    let imageData = try await context.imageData(refs[0])
    #expect(imageData == imageAttachment().data)
    #expect(try await context.imageData(refs[1]) == nil)

    let otherContext = await kernel.benchContext(sessionID: other.id)
    #expect(try await otherContext.imageData(refs[0]) == nil)
}

@Test func loadPairsDropsUnreadableRefsAtomically() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    let first = imageAttachment(1)
    let second = imageAttachment(2)
    let refs = try store.store([first, second])
    try FileManager.default.removeItem(
        at: dir.appendingPathComponent("attachments").appendingPathComponent(refs[0]))

    let pairs = store.loadPairs(refs)
    #expect(pairs.count == 1)
    #expect(pairs[0].ref == refs[1])
    #expect(pairs[0].attachment.data == second.data)
}

@Test func chatMessageAttachmentRefsRoundTripLeniently() throws {
    let message = ChatMessage(
        role: .user, content: "look", attachments: [imageAttachment()],
        attachmentRefs: ["ref-1"])
    let encoded = try JSONEncoder().encode(message)
    let decoded = try JSONDecoder().decode(ChatMessage.self, from: encoded)
    #expect(decoded.attachmentRefs == ["ref-1"])

    let legacy = Data(#"{"role":"user","content":"hi"}"#.utf8)
    let decodedLegacy = try JSONDecoder().decode(ChatMessage.self, from: legacy)
    #expect(decodedLegacy.attachmentRefs == [])
}

@Test func mergedConsecutiveTurnsConcatenateImageRefs() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    let refsA = try store.store([imageAttachment(1)])
    let refsB = try store.store([imageAttachment(2)])
    func userTurn(_ id: String, seq: Int, refs: [String]) -> ChatTurn {
        ChatTurn(
            id: id, sessionID: "s", seq: seq, role: .user, content: "look \(id)",
            attachmentRefs: refs, contentHash: id,
            createdAt: Date(timeIntervalSince1970: 0), updatedAt: Date(timeIntervalSince1970: 0))
    }

    let messages = ChatFlow.messages(
        from: [userTurn("t1", seq: 0, refs: refsA), userTurn("t2", seq: 1, refs: refsB)]
    ) { store.loadPairs($0) }
    #expect(messages.count == 1)
    #expect(messages[0].attachmentRefs == refsA + refsB)
    #expect(messages[0].attachments.count == 2)
}

@Test func describeImageToleratesTrailingPunctuationOnRefs() async {
    var seer = Fixtures.gguf(path: "/tmp/bench/llava-eyes")
    seer.capabilities = [.chat, .see]
    seer.state = .ready
    let context = BenchContext(
        invoke: { _, _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.text("a koala"))
                continuation.finish()
            }
        },
        submit: { _, _, _ in "job" },
        jobEvents: { _ in AsyncStream { $0.finish() } },
        cancelJob: { _ in },
        persistSpeech: { _, _, _, _, _ in "s" },
        voices: { _ in [] },
        imageData: { ref in ref == "abc.pasted-image.png" ? Data([1]) : nil })
    let outcome = await BenchTools.execute(
        ToolCall(
            name: BenchTools.describeImageName,
            arguments: .object(["artifact": .string("abc.pasted-image.png.")])),
        bench: [seer], context: context)
    #expect(outcome.text.contains("a koala"))
}

@Test func projectedMessagesCarryImageRefs() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    let image = imageAttachment()
    let refs = try store.store([documentAttachment(), image])
    let turn = ChatTurn(
        id: "t1", sessionID: "s", seq: 0, role: .user, content: "look",
        attachmentRefs: refs, contentHash: "h",
        createdAt: Date(timeIntervalSince1970: 0), updatedAt: Date(timeIntervalSince1970: 0))

    let messages = ChatFlow.messages(from: [turn]) { store.loadPairs($0) }
    #expect(messages.count == 1)
    #expect(messages[0].attachments.map(\.kind) == [.image])
    #expect(messages[0].attachmentRefs == [refs[1]])
}
