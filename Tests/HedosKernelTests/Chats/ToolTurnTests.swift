import Foundation
import Testing

@testable import HedosKernel

private func toolStore() throws -> (store: ChatStore, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    return (ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite")), dir)
}

private func sampleCall(id: String = "call-1") -> ToolCall {
    ToolCall(
        id: id, name: "get_time",
        arguments: .object(["zone": .string("UTC")]))
}

@Test func versionOneDatabaseMigratesAndOldTurnsReadBackUntouched() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("chats.sqlite")

    let legacy = try ChatDatabase(url: url)
    try legacy.execute(ChatStore.migrations[0])
    try legacy.setUserVersion(1)
    try legacy.run(
        """
        INSERT INTO sessions
            (id, title, created_at, updated_at, model_id, capability_tags,
             turn_count, pinned, archived, deleted_at)
        VALUES ('old-session', 'Old', 1.0, 1.0, 'model-a', '', 1, 0, 0, NULL)
        """, [])
    try legacy.run(
        """
        INSERT INTO turns
            (id, session_id, seq, role, content, thinking, model_id, stats_json,
             artifact_refs, superseded_by, content_hash, created_at, updated_at)
        VALUES ('old-turn', 'old-session', 0, 'user', 'before the migration',
                NULL, NULL, NULL, '', NULL, 'hash', 1.0, 1.0)
        """, [])

    let reopened = ChatStore(databaseURL: url)
    let transcript = try #require(try await reopened.session(id: "old-session"))
    #expect(transcript.turns.map(\.id) == ["old-turn"])
    #expect(transcript.turns[0].content == "before the migration")
    #expect(transcript.turns[0].toolCallsJSON == nil)
    #expect(transcript.turns[0].toolCalls.isEmpty)
}

@Test func toolTurnsRoundTripThroughAppendReadAndUpdate() async throws {
    let (store, dir) = try toolStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")

    let call = sampleCall()
    let assistant = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "", modelID: "model-a", stats: nil,
                  toolCalls: [call]),
        to: session.id)
    let result = try await store.appendTurn(
        TurnDraft(role: .tool, content: "12:00 UTC", stats: nil,
                  toolCallID: call.id, toolName: call.name),
        to: session.id)

    let transcript = try #require(try await store.session(id: session.id))
    #expect(transcript.turns[0].toolCalls == [call])
    #expect(transcript.turns[1].role == .tool)
    #expect(transcript.turns[1].toolCallID == call.id)
    #expect(transcript.turns[1].toolName == "get_time")

    var updated = transcript.turns[1]
    updated.content = "12:01 UTC"
    let written = try await store.updateTurn(updated)
    #expect(written.toolCallID == call.id)
    let reread = try #require(try await store.session(id: session.id))
    #expect(reread.turns[1].content == "12:01 UTC")
    #expect(reread.turns[1].toolName == "get_time")
    _ = assistant
    _ = result
}

@Test func contentHashCoversTheToolFields() async throws {
    let (store, dir) = try toolStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let turn = try await store.appendTurn(
        TurnDraft(role: .tool, content: "same", stats: nil,
                  toolCallID: "call-1", toolName: "get_time"),
        to: session.id)

    var renamedTool = turn
    renamedTool.toolName = "get_weather"
    let written = try await store.updateTurn(renamedTool)
    #expect(written.contentHash != turn.contentHash)

    try await store.resetWriteCounter()
    _ = try await store.updateTurn(written)
    #expect(try await store.rowsWritten() == 0)
}

@Test func exportWithToolTurnsRoundTripsAndArchivesWithoutThemStayByteEqual() async throws {
    let (store, dir) = try toolStore()
    defer { try? FileManager.default.removeItem(at: dir) }

    let plain = try await store.createSession(modelID: "model-a")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "plain"), to: plain.id)
    let plainExport = try ChatExport.json(try #require(try await store.session(id: plain.id)))
    try await store.deleteSession(id: plain.id)
    _ = try await store.importTranscript(ChatExport.decode(plainExport))
    let plainReexport = try ChatExport.json(try #require(try await store.session(id: plain.id)))
    #expect(plainReexport == plainExport)
    #expect(!String(decoding: plainExport, as: UTF8.self).contains("toolCallsJSON"))

    let tooled = try await store.createSession(modelID: "model-a")
    let call = sampleCall(id: "call-x")
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "", modelID: "model-a", stats: nil,
                  toolCalls: [call]),
        to: tooled.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .tool, content: "ok", stats: nil,
                  toolCallID: call.id, toolName: call.name),
        to: tooled.id)
    let archive = try ChatExport.json(try #require(try await store.session(id: tooled.id)))
    try await store.deleteSession(id: tooled.id)
    _ = try await store.importTranscript(ChatExport.decode(archive))
    let restored = try #require(try await store.session(id: tooled.id))
    #expect(restored.turns[0].toolCalls == [call])
    #expect(restored.turns[1].toolCallID == "call-x")
    #expect(try ChatExport.json(restored) == archive)
}

@Test func toolResultContentIsSearchable() async throws {
    let (store, dir) = try toolStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    _ = try await store.appendTurn(
        TurnDraft(role: .tool, content: "the xylophone constant is 42", stats: nil,
                  toolCallID: "call-1", toolName: "read_file"),
        to: session.id)

    let hits = try await store.searchChats(query: "xylophone")
    #expect(hits.count == 1)
    #expect(hits[0].sessionID == session.id)
}
