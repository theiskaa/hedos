import Foundation
import Testing

@testable import HedosKernel

@Test func recordGeneratedTurnAppendsPromptAndArtifactTurns() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    let session = try await kernel.chats.createSession(modelID: "mflux:schnell")
    try await kernel.recordGeneratedTurn(
        sessionID: session.id,
        prompt: "a koala riding a bicycle",
        artifactID: "img_abc123",
        tag: SessionTag.generatedImage)

    let transcript = try #require(try await kernel.chats.session(id: session.id))
    #expect(transcript.turns.count == 2)
    #expect(transcript.turns[0].role == .user)
    #expect(transcript.turns[0].content == "a koala riding a bicycle")
    #expect(transcript.turns[0].artifactRefs.isEmpty)
    #expect(transcript.turns[1].role == .assistant)
    #expect(transcript.turns[1].content.isEmpty)
    #expect(transcript.turns[1].artifactRefs == ["img_abc123"])
    #expect(transcript.turns.map(\.seq) == [0, 1])
    #expect(transcript.session.capabilityTags == [SessionTag.generatedImage])
}

@Test func recordGeneratedTurnMergesTagsAcrossModalities() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    let session = try await kernel.chats.createSession()
    try await kernel.recordGeneratedTurn(
        sessionID: session.id, prompt: "a koala", artifactID: "img_1",
        tag: SessionTag.generatedImage)
    try await kernel.recordGeneratedTurn(
        sessionID: session.id, prompt: "say hello", artifactID: "wav_1",
        tag: SessionTag.spoke)

    let transcript = try #require(try await kernel.chats.session(id: session.id))
    #expect(transcript.turns.count == 4)
    #expect(transcript.turns[3].artifactRefs == ["wav_1"])
    #expect(transcript.session.turnCount == 4)
    #expect(
        Set(transcript.session.capabilityTags)
            == Set([SessionTag.generatedImage, SessionTag.spoke]))
}

@Test func artifactOnlyConversationTitlesFromItsPrompt() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    let session = try await kernel.chats.createSession()
    try await kernel.recordGeneratedTurn(
        sessionID: session.id, prompt: "a koala riding a bicycle", artifactID: "img_1",
        tag: SessionTag.generatedImage)

    let title = try await kernel.autoTitleIfNeeded(sessionID: session.id)
    #expect(title == "a koala riding a bicycle")
    let reloaded = try #require(try await kernel.chats.session(id: session.id))
    #expect(reloaded.session.title == "a koala riding a bicycle")
}

@Test func emptyConversationIsNotTitled() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    let session = try await kernel.chats.createSession()
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .user, content: "hello"), to: session.id)

    #expect(try await kernel.autoTitleIfNeeded(sessionID: session.id) == nil)
}

@Test func recordGeneratedTurnRejectsUnknownSession() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    await #expect(throws: ChatStoreError.self) {
        try await kernel.recordGeneratedTurn(
            sessionID: "missing", prompt: "a koala", artifactID: "img_1",
            tag: SessionTag.generatedImage)
    }
}
