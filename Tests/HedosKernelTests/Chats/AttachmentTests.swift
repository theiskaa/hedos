import Foundation
import Testing

@testable import HedosKernel

private func pngBytes(_ marker: UInt8) -> Data {
    Data([0x89, 0x50, 0x4E, 0x47, marker])
}

@Test func attachmentStoreRoundTripsAndIsContentAddressed() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    let one = ChatAttachment(kind: .image, data: pngBytes(1), mimeType: "image/png")
    let refs = try store.store([one, one])
    #expect(refs.count == 2)
    #expect(refs[0] == refs[1])
    #expect(refs[0].hasSuffix(".png"))
    let loaded = store.load(refs)
    #expect(loaded.first?.data == pngBytes(1))
    #expect(loaded.first?.mimeType == "image/png")
}

@Test func attachmentStoreRefusesPathTraversalRefs() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    #expect(store.load(["../../../etc/passwd"]).isEmpty)
    #expect(store.load(["sub/evil.png"]).isEmpty)
    #expect(store.load(["/etc/hosts"]).isEmpty)
    #expect(!AttachmentStore.isSafeRef("../x"))
    #expect(!AttachmentStore.isSafeRef("a/b"))
    #expect(AttachmentStore.isSafeRef("abc123.png"))
}

@Test func chatMessageEncodesAttachmentsOnlyWhenPresent() throws {
    let plain = ChatMessage(role: .user, content: "hi")
    let plainJSON = String(decoding: try JSONEncoder().encode(plain), as: UTF8.self)
    #expect(!plainJSON.contains("attachments"))

    let withImage = ChatMessage(
        role: .user, content: "what is this?",
        attachments: [ChatAttachment(kind: .image, data: pngBytes(2), mimeType: "image/png")])
    let data = try JSONEncoder().encode(withImage)
    #expect(String(decoding: data, as: UTF8.self).contains("attachments"))
    let round = try JSONDecoder().decode(ChatMessage.self, from: data)
    #expect(round.attachments.first?.data == pngBytes(2))
}

@Test func payloadValueEmitsImagesAsBase64() {
    let message = ChatMessage(
        role: .user, content: "see this",
        attachments: [ChatAttachment(kind: .image, data: pngBytes(3), mimeType: "image/png")])
    guard case .object(let object) = message.payloadValue,
        case .array(let images)? = object["images"]
    else {
        Issue.record("expected an images array")
        return
    }
    #expect(images.first?.stringValue == pngBytes(3).base64EncodedString())
}

@Test func projectionUnionsAttachmentsOnSameRoleMerge() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let refsA = try store.store(
        [ChatAttachment(kind: .image, data: pngBytes(4), mimeType: "image/png")])
    let refsB = try store.store(
        [ChatAttachment(kind: .image, data: pngBytes(5), mimeType: "image/png")])
    let now = Date(timeIntervalSince1970: 0)
    let turns = [
        ChatTurn(
            id: "1", sessionID: "s", seq: 0, role: .user, content: "first",
            attachmentRefs: refsA, contentHash: "h1", createdAt: now, updatedAt: now),
        ChatTurn(
            id: "2", sessionID: "s", seq: 1, role: .user, content: "second",
            attachmentRefs: refsB, contentHash: "h2", createdAt: now, updatedAt: now),
    ]
    let loader: @Sendable ([String]) -> [ChatAttachment] = { store.load($0) }
    let messages = ChatFlow.messages(from: turns, attachmentLoader: loader)
    #expect(messages.count == 1)
    #expect(messages.first?.attachments.count == 2)
}

@Test func projectionKeepsAnImageOnlyUserTurn() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let refs = try store.store(
        [ChatAttachment(kind: .image, data: pngBytes(6), mimeType: "image/png")])
    let now = Date(timeIntervalSince1970: 0)
    let turns = [
        ChatTurn(
            id: "1", sessionID: "s", seq: 0, role: .user, content: "",
            attachmentRefs: refs, contentHash: "h", createdAt: now, updatedAt: now)
    ]
    let loader: @Sendable ([String]) -> [ChatAttachment] = { store.load($0) }
    let messages = ChatFlow.messages(from: turns, attachmentLoader: loader)
    #expect(messages.count == 1)
    #expect(messages.first?.attachments.count == 1)
}

@Test func payloadCarriesImagesDetectsImageMessages() {
    let withImage = ChatMessage(
        role: .user, content: "x",
        attachments: [ChatAttachment(kind: .image, data: pngBytes(7), mimeType: "image/png")])
    let payload = JSONValue.object(["messages": .array([withImage.payloadValue])])
    #expect(Kernel.payloadCarriesImages(payload))

    let textOnly = JSONValue.object([
        "messages": .array([ChatMessage(role: .user, content: "x").payloadValue])
    ])
    #expect(!Kernel.payloadCarriesImages(textOnly))

    let historicalImageThenText = JSONValue.object([
        "messages": .array([
            withImage.payloadValue,
            ChatMessage(role: .assistant, content: "a cat").payloadValue,
            ChatMessage(role: .user, content: "and now in words?").payloadValue,
        ])
    ])
    #expect(!Kernel.payloadCarriesImages(historicalImageThenText))
}

@Test func attachmentRefMatchesTheStoredFilename() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let stored = try store.store(
        [ChatAttachment(kind: .image, data: pngBytes(11), mimeType: "image/png")])
    let computed = Kernel.attachmentRef(for: pngBytes(11), mimeType: "image/png")
    #expect(stored.first == computed)
    #expect(AttachmentStore.ref(for: pngBytes(11), mimeType: "image/png") == computed)
}

@Test func kernelChatAttachmentsReadsFromTheSessionAttachmentDirectory() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir)
    let store = AttachmentStore(directory: dir.appendingPathComponent("attachments"))
    let refs = try store.store(
        [ChatAttachment(kind: .image, data: pngBytes(9), mimeType: "image/png")])
    let loaded = await kernel.chatAttachments(refs)
    #expect(loaded.first?.data == pngBytes(9))
    #expect(loaded.first?.mimeType == "image/png")
    #expect(await kernel.chatAttachments(["../secret.png"]).isEmpty)
}

@Test func imageBearingTurnPersistsAndReloadsThroughTheStore() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: "m")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "look", attachmentRefs: ["abc.png"]), to: session.id)
    let reloaded = try #require(try await store.session(id: session.id))
    #expect(reloaded.turns.first?.attachmentRefs == ["abc.png"])
}

private func textBytes(_ text: String) -> Data {
    Data(text.utf8)
}

@Test func documentRefsCarryASanitizedNameSlug() {
    let data = textBytes("hello")
    let named = AttachmentStore.ref(for: data, mimeType: "text/markdown", name: "My Notes v2.md")
    #expect(named.hasSuffix(".my-notes-v2.md"))
    #expect(AttachmentStore.isSafeRef(named))
    let unnamed = AttachmentStore.ref(for: data, mimeType: "text/markdown")
    #expect(unnamed.hasSuffix(".md"))
    #expect(!unnamed.contains("--"))
    #expect(AttachmentStore.slug("Weird — name!!.txt") == "weird-name")
    #expect(AttachmentStore.slug("dots.in.middle.txt") == "dots-in-middle")
    #expect(AttachmentStore.slug("ünïcödé.md") == "n-c-d")
    #expect(AttachmentStore.slug(String(repeating: "a", count: 80) + ".txt").count <= 40)
    #expect(AttachmentStore.slug("...") == "")
    #expect(!AttachmentStore.slug("comma,name.txt").contains(","))
}

@Test func documentStoreLoadRoundTripsKindAndName() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let doc = ChatAttachment(
        kind: .document, data: textBytes("# hi"), mimeType: "text/markdown", name: "readme.md")
    let refs = try store.store([doc])
    let loaded = try #require(store.load(refs).first)
    #expect(loaded.kind == .document)
    #expect(loaded.mimeType == "text/markdown")
    #expect(loaded.name == "readme.md")
    #expect(loaded.data == textBytes("# hi"))
}

@Test func sourceFilesKeepTheirExtensionThroughTheStore() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let doc = ChatAttachment(
        kind: .document, data: textBytes("let x = 1"), mimeType: "text/plain",
        name: "parser.swift")
    let refs = try store.store([doc])
    #expect(refs.first?.hasSuffix(".parser.swift") == true)
    let loaded = try #require(store.load(refs).first)
    #expect(loaded.kind == .document)
    #expect(loaded.name == "parser.swift")
}

@Test func legacyRefsLoadWithTodaysBehavior() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let attachments = dir.appendingPathComponent("a")
    try FileManager.default.createDirectory(at: attachments, withIntermediateDirectories: true)
    try pngBytes(1).write(to: attachments.appendingPathComponent("abc.png"))
    try textBytes("x").write(to: attachments.appendingPathComponent("abc.bin"))
    try textBytes("hi").write(to: attachments.appendingPathComponent("abc.txt"))
    let store = AttachmentStore(directory: attachments)
    let image = try #require(store.load(["abc.png"]).first)
    #expect(image.kind == .image)
    #expect(image.name == nil)
    let binary = try #require(store.load(["abc.bin"]).first)
    #expect(binary.kind == .image)
    #expect(binary.mimeType == "application/octet-stream")
    let text = try #require(store.load(["abc.txt"]).first)
    #expect(text.kind == .document)
    #expect(text.name == nil)
}

@Test func payloadValueInlinesDocumentsIntoContent() {
    let doc = ChatAttachment(
        kind: .document, data: textBytes("alpha beta"), mimeType: "text/plain",
        name: "notes.txt")
    let message = ChatMessage(role: .user, content: "summarize this", attachments: [doc])
    guard case .object(let object) = message.payloadValue,
        case .string(let content)? = object["content"]
    else {
        Issue.record("expected string content")
        return
    }
    #expect(content.contains("<attached-file name=\"notes.txt\">"))
    #expect(content.contains("alpha beta"))
    #expect(content.contains("</attached-file>"))
    #expect(content.hasSuffix("summarize this"))
    #expect(object["images"] == nil)
}

@Test func payloadValueMixesImagesAndDocuments() {
    let doc = ChatAttachment(
        kind: .document, data: textBytes("body"), mimeType: "text/plain", name: nil)
    let image = ChatAttachment(kind: .image, data: pngBytes(8), mimeType: "image/png")
    let message = ChatMessage(role: .user, content: "both", attachments: [doc, image])
    guard case .object(let object) = message.payloadValue,
        case .string(let content)? = object["content"],
        case .array(let images)? = object["images"]
    else {
        Issue.record("expected content and images")
        return
    }
    #expect(content.contains("<attached-file>"))
    #expect(images.count == 1)
}

@Test func documentOnlyMessagesDoNotTripTheVisionGate() {
    let doc = ChatAttachment(
        kind: .document, data: textBytes("text"), mimeType: "text/plain", name: "a.txt")
    let message = ChatMessage(role: .user, content: "read", attachments: [doc])
    let payload = JSONValue.object(["messages": .array([message.payloadValue])])
    #expect(!Kernel.payloadCarriesImages(payload))
}

@Test func mergedTurnsPlaceDocumentBlocksBeforeMergedContent() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = AttachmentStore(directory: dir.appendingPathComponent("a"))
    let refsA = try store.store([
        ChatAttachment(
            kind: .document, data: textBytes("doc one"), mimeType: "text/plain", name: "one.txt")
    ])
    let refsB = try store.store([
        ChatAttachment(
            kind: .document, data: textBytes("doc two"), mimeType: "text/plain", name: "two.txt")
    ])
    let now = Date(timeIntervalSince1970: 0)
    let turns = [
        ChatTurn(
            id: "1", sessionID: "s", seq: 0, role: .user, content: "first",
            attachmentRefs: refsA, contentHash: "h1", createdAt: now, updatedAt: now),
        ChatTurn(
            id: "2", sessionID: "s", seq: 1, role: .user, content: "second",
            attachmentRefs: refsB, contentHash: "h2", createdAt: now, updatedAt: now),
    ]
    let loader: @Sendable ([String]) -> [ChatAttachment] = { store.load($0) }
    let messages = ChatFlow.messages(from: turns, attachmentLoader: loader)
    #expect(messages.count == 1)
    #expect(messages.first?.attachments.count == 2)
    guard case .object(let object) = messages[0].payloadValue,
        case .string(let content)? = object["content"]
    else {
        Issue.record("expected string content")
        return
    }
    let docOne = try #require(content.range(of: "doc one"))
    let docTwo = try #require(content.range(of: "doc two"))
    let first = try #require(content.range(of: "first"))
    #expect(docOne.lowerBound < first.lowerBound)
    #expect(docTwo.lowerBound < first.lowerBound)
}

@Test func chatMessageCodableRoundTripsAttachmentNames() throws {
    let doc = ChatAttachment(
        kind: .document, data: textBytes("x"), mimeType: "text/plain", name: "a.txt")
    let message = ChatMessage(role: .user, content: "c", attachments: [doc])
    let round = try JSONDecoder().decode(
        ChatMessage.self, from: try JSONEncoder().encode(message))
    #expect(round.attachments.first?.name == "a.txt")
    let legacy = """
        {"role":"user","content":"c","attachments":[{"kind":"image","data":"","mimeType":"image/png"}]}
        """
    let decoded = try JSONDecoder().decode(ChatMessage.self, from: Data(legacy.utf8))
    #expect(decoded.attachments.first?.name == nil)
}
