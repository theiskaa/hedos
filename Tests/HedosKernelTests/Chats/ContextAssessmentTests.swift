import Foundation
import Testing

@testable import HedosKernel

private func windowedChatRecord(runtimeID: RuntimeID, window: Int?) -> ModelRecord {
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: runtimeID, resolved: .user, tier: .native)
    record.contextLength = window
    record.state = .ready
    return record
}

private func seededKernel(
    in dir: URL, record: ModelRecord, turnCharacters: Int
) async throws -> (kernel: Kernel, sessionID: String) {
    let kernel = Kernel(directory: dir, adapters: [], secrets: InMemorySecretStore())
    try await kernel.registry.register(record)
    let session = try await kernel.chats.createSession(title: "long", modelID: record.id)
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .user, content: String(repeating: "a", count: turnCharacters)),
        to: session.id)
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .assistant, content: String(repeating: "b", count: turnCharacters)),
        to: session.id)
    return (kernel, session.id)
}

@Test func assessmentFlagsHistoryExceedingSmallWindow() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = windowedChatRecord(runtimeID: "ollama", window: 512)
    let (kernel, sessionID) = try await seededKernel(
        in: dir, record: record, turnCharacters: 2000)

    let assessment = try #require(
        try await kernel.chatContextAssessment(sessionID: sessionID, modelID: record.id))
    #expect(assessment.fits == false)
    #expect(assessment.window == 512)
    #expect(assessment.estimatedTokens == 1000)
}

@Test func assessmentPassesFittingHistory() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = windowedChatRecord(runtimeID: "ollama", window: 8192)
    let (kernel, sessionID) = try await seededKernel(
        in: dir, record: record, turnCharacters: 400)

    let assessment = try #require(
        try await kernel.chatContextAssessment(sessionID: sessionID, modelID: record.id))
    #expect(assessment.fits)
    #expect(assessment.estimatedTokens == 200)
}

@Test func assessmentNilWhenWindowUnknown() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = windowedChatRecord(runtimeID: "generic:openai-server", window: nil)
    let (kernel, sessionID) = try await seededKernel(
        in: dir, record: record, turnCharacters: 400)

    let assessment = try await kernel.chatContextAssessment(
        sessionID: sessionID, modelID: record.id)
    #expect(assessment == nil)
}
