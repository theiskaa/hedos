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
