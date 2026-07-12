import Foundation
import Testing

@testable import HedosKernel

private func makeStore(in directory: URL) -> ChatStore {
    ChatStore(databaseURL: directory.appendingPathComponent("chats.sqlite"))
}

@Test func setSystemPromptPersistsAndSurvivesReopen() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(
        title: "Prompted", modelID: "m", systemPrompt: "be terse")
    #expect(try await store.session(id: session.id)?.session.systemPrompt == "be terse")

    try await store.setSystemPrompt(id: session.id, prompt: "be verbose")
    let reopened = makeStore(in: dir)
    #expect(try await reopened.session(id: session.id)?.session.systemPrompt == "be verbose")

    try await store.setSystemPrompt(id: session.id, prompt: nil)
    #expect(try await makeStore(in: dir).session(id: session.id)?.session.systemPrompt == nil)
}

@Test func attributionNeededWhenActiveAssistantTurnsSpanModels() {
    func assistant(_ id: String, model: String, superseded: String? = nil) -> ChatTurn {
        ChatTurn(
            id: id, sessionID: "s", seq: 0, role: .assistant, content: "x", modelID: model,
            supersededBy: superseded, contentHash: id,
            createdAt: Date(timeIntervalSince1970: 0), updatedAt: Date(timeIntervalSince1970: 0))
    }
    func session(model: String?) -> ChatSession {
        ChatSession(
            id: "s", title: "t", createdAt: Date(timeIntervalSince1970: 0),
            updatedAt: Date(timeIntervalSince1970: 0), modelID: model)
    }

    let single = ChatTranscript(
        session: session(model: "a"), turns: [assistant("t1", model: "a")])
    #expect(!single.attributionNeeded)

    let spanning = ChatTranscript(
        session: session(model: "a"),
        turns: [assistant("t1", model: "a"), assistant("t2", model: "b")])
    #expect(spanning.attributionNeeded)

    let differsFromBound = ChatTranscript(
        session: session(model: "a"), turns: [assistant("t1", model: "b")])
    #expect(differsFromBound.attributionNeeded)

    let supersededSpanIgnored = ChatTranscript(
        session: session(model: "a"),
        turns: [assistant("t1", model: "a"), assistant("t2", model: "b", superseded: "t3")])
    #expect(!supersededSpanIgnored.attributionNeeded)
}

@Test func importCollidingWithLiveSessionDuplicatesUnderFreshIDsPreservingSupersedeChain()
    async throws
{
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(modelID: "m")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "q"), to: session.id)
    var first = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "first", modelID: "m"), to: session.id)
    let second = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "second", modelID: "m"), to: session.id)
    first.supersededBy = second.id
    _ = try await store.updateTurn(first)

    let transcript = try #require(try await store.session(id: session.id))
    let imported = try await store.importTranscript(transcript)
    #expect(imported.id != session.id)

    let active = try await store.sessions(filter: .active).map(\.id)
    #expect(active.contains(session.id))
    #expect(active.contains(imported.id))

    let copy = try #require(try await store.session(id: imported.id))
    let copyFirst = try #require(copy.turns.first { $0.content == "first" })
    let copySecond = try #require(copy.turns.first { $0.content == "second" })
    #expect(copyFirst.supersededBy == copySecond.id)
    #expect(copyFirst.id != first.id)
}

@Test func importOfTombstonedSessionStillRestoresVerbatim() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(modelID: "m")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hi"), to: session.id)
    let transcript = try #require(try await store.session(id: session.id))
    try await store.deleteSession(id: session.id)

    let restored = try await store.importTranscript(transcript)
    #expect(restored.id == session.id)
    #expect(try await store.session(id: session.id) != nil)
}

@Test func migrationAddsInterruptedAndSessionColumnsToExistingDatabase() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(title: "Migration", modelID: "m")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hi"), to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "hello", modelID: "m"), to: session.id)

    let reopened = makeStore(in: dir)
    let transcript = try await reopened.session(id: session.id)
    #expect(transcript?.turns.allSatisfy { !$0.interrupted } == true)
    #expect(transcript?.session.systemPrompt == nil)
    #expect(transcript?.session.titledBy == nil)
}

@Test func interruptedFlagPersistsThroughUpdateAndReopen() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(title: "Interrupted")
    var turn = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "partial answer", modelID: "m"), to: session.id)
    #expect(!turn.interrupted)
    turn.interrupted = true
    let updated = try await store.updateTurn(turn)
    #expect(updated.interrupted)

    let reopened = makeStore(in: dir)
    let transcript = try await reopened.session(id: session.id)
    #expect(
        transcript?.turns.first(where: { $0.role == .assistant })?.interrupted == true)
}

@Test func chatPersistsAcrossStoreReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let session = try await store.createSession(title: "Trip planning", modelID: "ollama:qwen3")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "Plan a weekend in Lisbon", modelID: "ollama:qwen3"),
        to: session.id)
    let reply = try await store.appendTurn(
        TurnDraft(
            role: .assistant,
            content: "Start at the Alfama viewpoints.",
            thinking: "the user wants an itinerary",
            modelID: "ollama:qwen3",
            statsJSON: "{\"tokens\":42}",
            artifactRefs: ["kokoro_abc123_j1"]),
        to: session.id,
        mergingCapabilityTags: ["thinking", "spoke"])
    try await store.renameSession(id: session.id, title: "Lisbon weekend")
    try await store.setPinned(id: session.id, true)

    let reloaded = makeStore(in: dir)
    let sessions = try await reloaded.sessions()
    #expect(sessions.count == 1)
    #expect(sessions[0].id == session.id)
    #expect(sessions[0].title == "Lisbon weekend")
    #expect(sessions[0].turnCount == 2)
    #expect(sessions[0].pinned)
    #expect(!sessions[0].archived)
    #expect(sessions[0].modelID == "ollama:qwen3")
    #expect(sessions[0].capabilityTags == ["thinking", "spoke"])

    let transcript = try await reloaded.session(id: session.id)
    let turns = try #require(transcript?.turns)
    #expect(turns.count == 2)
    #expect(turns.map(\.seq) == [0, 1])
    #expect(turns[0].role == .user)
    #expect(turns[0].content == "Plan a weekend in Lisbon")
    #expect(turns[1].id == reply.id)
    #expect(turns[1].role == .assistant)
    #expect(turns[1].content == "Start at the Alfama viewpoints.")
    #expect(turns[1].thinking == "the user wants an itinerary")
    #expect(turns[1].modelID == "ollama:qwen3")
    #expect(turns[1].statsJSON == "{\"tokens\":42}")
    #expect(turns[1].artifactRefs == ["kokoro_abc123_j1"])
    #expect(turns[1].supersededBy == nil)
    #expect(turns[1].contentHash == reply.contentHash)
}

@Test func appendToTwoHundredTurnSessionWritesAtMostTwoRows() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let session = try await store.createSession(title: "Long haul")
    for index in 0..<200 {
        _ = try await store.appendTurn(
            TurnDraft(
                role: index.isMultiple(of: 2) ? .user : .assistant,
                content: "turn number \(index)"),
            to: session.id)
    }

    try await store.resetWriteCounter()
    let appended = try await store.appendTurn(
        TurnDraft(role: .user, content: "one more"), to: session.id)
    #expect(try await store.rowsWritten() <= 2)
    #expect(appended.seq == 200)

    try await store.resetWriteCounter()
    let unchanged = try await store.updateTurn(appended)
    #expect(try await store.rowsWritten() == 0)
    #expect(unchanged.contentHash == appended.contentHash)

    var edited = appended
    edited.content = "one more, edited"
    try await store.resetWriteCounter()
    let updated = try await store.updateTurn(edited)
    #expect(try await store.rowsWritten() <= 2)
    #expect(updated.contentHash != appended.contentHash)

    let transcript = try await store.session(id: session.id)
    #expect(transcript?.turns.count == 201)
    #expect(transcript?.turns.last?.content == "one more, edited")
}

@Test func searchFindsWordFromThirdTurnOfArchivedSession() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let noise = try await store.createSession(title: "Grocery run")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "Add oat milk and coffee to the list"), to: noise.id)

    let session = try await store.createSession(title: "Night dive log")
    let contents = [
        "We checked the tanks before sunset",
        "Visibility was better than expected",
        "The water glowed bioluminescent blue around every stroke",
        "We surfaced near the pier",
        "Logged forty minutes of bottom time",
    ]
    for (index, content) in contents.enumerated() {
        _ = try await store.appendTurn(
            TurnDraft(role: index.isMultiple(of: 2) ? .user : .assistant, content: content),
            to: session.id)
    }
    try await store.setArchived(id: session.id, true)
    #expect(try await store.sessions(filter: .active).map(\.id) == [noise.id])
    #expect(try await store.sessions(filter: .archived).map(\.id) == [session.id])

    let hits = try await store.searchChats(query: "bioluminescent")
    #expect(hits.count == 1)
    let hit = try #require(hits.first)
    #expect(hit.sessionID == session.id)
    #expect(hit.sessionTitle == "Night dive log")
    #expect(hit.snippet.contains("[bioluminescent]"))

    let prefixHits = try await store.searchChats(query: "biolum")
    #expect(prefixHits.count == 1)
    #expect(prefixHits.first?.sessionID == session.id)

    let transcript = try await store.session(id: session.id)
    #expect(transcript?.turns[2].id == hit.turnID)
}

@Test func partialTurnWrittenDuringStreamingSurvivesSimulatedCrash() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let streaming = makeStore(in: dir)

    let session = try await streaming.createSession(title: "Interrupted", modelID: "ollama:qwen3")
    _ = try await streaming.appendTurn(
        TurnDraft(role: .user, content: "Tell me a story"), to: session.id)
    var partial = try await streaming.appendTurn(
        TurnDraft(role: .assistant, content: "Once upon a", modelID: "ollama:qwen3"),
        to: session.id)
    partial.content = "Once upon a time, a lighthouse"
    partial = try await streaming.updateTurn(partial)

    let reopened = makeStore(in: dir)
    let transcript = try await reopened.session(id: session.id)
    let turns = try #require(transcript?.turns)
    #expect(turns.count == 2)
    #expect(turns[1].id == partial.id)
    #expect(turns[1].content == "Once upon a time, a lighthouse")
    #expect(turns[1].modelID == "ollama:qwen3")
    #expect(try await reopened.sessions().first?.turnCount == 2)
}

@Test func concurrentAppendsFromTwoConnectionsMintUniqueSequenceNumbers() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let first = makeStore(in: dir)
    let second = makeStore(in: dir)

    let session = try await first.createSession(title: "Contended")
    try await withThrowingTaskGroup(of: Void.self) { group in
        for index in 0..<10 {
            let store = index.isMultiple(of: 2) ? first : second
            group.addTask {
                _ = try await store.appendTurn(
                    TurnDraft(role: .user, content: "turn \(index)"), to: session.id)
            }
        }
        try await group.waitForAll()
    }

    let transcript = try await first.session(id: session.id)
    let turns = try #require(transcript?.turns)
    #expect(turns.count == 10)
    #expect(turns.map(\.seq).sorted() == Array(0..<10))
    #expect(try await second.sessions().first?.turnCount == 10)
}

@Test func tombstonedSessionsVanishFromListingsButStayInDatabase() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let keep = try await store.createSession(title: "Keep")
    let doomed = try await store.createSession(title: "Doomed")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "an unrepeatable secret"), to: doomed.id)
    try await store.deleteSession(id: doomed.id)

    #expect(try await store.sessions(filter: .active).map(\.id) == [keep.id])
    #expect(try await store.sessions(filter: .all).map(\.id) == [keep.id])
    #expect(try await store.sessions(filter: .archived).isEmpty)
    #expect(try await store.session(id: doomed.id) == nil)
    #expect(try await store.searchChats(query: "unrepeatable").isEmpty)

    let database = try ChatDatabase(url: dir.appendingPathComponent("chats.sqlite"))
    let sessionRow = try database.rows(
        "SELECT deleted_at, title FROM sessions WHERE id = ?", [.text(doomed.id)]
    ).first
    #expect(sessionRow?.optionalReal(0) != nil)
    #expect(sessionRow?.text(1) == "Doomed")
    let turnRow = try database.rows(
        "SELECT COUNT(*), MAX(content) FROM turns WHERE session_id = ?", [.text(doomed.id)]
    ).first
    #expect(turnRow?.integer(0) == 1)
    #expect(turnRow?.text(1) == "an unrepeatable secret")
}

@Test func writesToTombstonedSessionThrowLikeReads() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let session = try await store.createSession(title: "Doomed")
    var turn = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "streaming"), to: session.id)
    try await store.deleteSession(id: session.id)

    await #expect(throws: ChatStoreError.self) {
        _ = try await store.appendTurn(
            TurnDraft(role: .assistant, content: "too late"), to: session.id)
    }
    turn.content = "streaming, revised"
    await #expect(throws: ChatStoreError.self) {
        _ = try await store.updateTurn(turn)
    }

    let database = try ChatDatabase(url: dir.appendingPathComponent("chats.sqlite"))
    let counts = try database.rows(
        "SELECT COUNT(*), MAX(content) FROM turns WHERE session_id = ?", [.text(session.id)]
    ).first
    #expect(counts?.integer(0) == 1)
    #expect(counts?.text(1) == "streaming")
}

@Test func queuedWritesToTombstonedSessionThrowBeforeStoreOpens() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let blocked = dir.appendingPathComponent("store")
    try Data().write(to: blocked)
    let store = ChatStore(databaseURL: blocked.appendingPathComponent("chats.sqlite"))

    let session = try await store.createSession(title: "Doomed offline")
    var turn = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "streaming"), to: session.id)
    try await store.deleteSession(id: session.id)

    await #expect(throws: ChatStoreError.self) {
        _ = try await store.appendTurn(
            TurnDraft(role: .assistant, content: "too late"), to: session.id)
    }
    turn.content = "streaming, revised"
    await #expect(throws: ChatStoreError.self) {
        _ = try await store.updateTurn(turn)
    }

    try FileManager.default.removeItem(at: blocked)
    #expect(try await store.sessions(filter: .all).isEmpty)
    #expect(try await store.session(id: session.id) == nil)
}

@Test func queuedWritesFlushWhenStoreOpensLate() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let blocked = dir.appendingPathComponent("store")
    try Data().write(to: blocked)
    let store = ChatStore(databaseURL: blocked.appendingPathComponent("chats.sqlite"))

    let session = try await store.createSession(title: "Finished offline")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "ping"), to: session.id)
    var reply = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "po"), to: session.id)
    reply.content = "pong"
    _ = try await store.updateTurn(reply)
    try await store.renameSession(id: session.id, title: "Finished late")

    try FileManager.default.removeItem(at: blocked)

    let sessions = try await store.sessions()
    #expect(sessions.map(\.id) == [session.id])
    #expect(sessions[0].title == "Finished late")
    #expect(sessions[0].turnCount == 2)

    let reloaded = ChatStore(databaseURL: blocked.appendingPathComponent("chats.sqlite"))
    let transcript = try await reloaded.session(id: session.id)
    #expect(transcript?.turns.map(\.content) == ["ping", "pong"])
    #expect(try await reloaded.searchChats(query: "pong").count == 1)
}

@Test func forwardSchemaVersionFailsFast() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("chats.sqlite")
    _ = try await ChatStore(databaseURL: url).createSession(title: "Old world")

    let database = try ChatDatabase(url: url)
    try database.setUserVersion(99)

    let store = ChatStore(databaseURL: url)
    await #expect(throws: ChatStoreError.self) {
        _ = try await store.sessions()
    }
    await #expect(throws: ChatStoreError.self) {
        _ = try await store.createSession(title: "Should not land")
    }
}

@Test func supersededTurnsAreRetiredNotDeleted() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)

    let session = try await store.createSession(title: "Branching")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "First question"), to: session.id)
    var original = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "First answer"), to: session.id)
    let regenerated = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "Second answer"), to: session.id)
    original.supersededBy = regenerated.id
    _ = try await store.updateTurn(original)

    let transcript = try await store.session(id: session.id)
    let turns = try #require(transcript?.turns)
    #expect(turns.count == 3)
    #expect(turns[1].supersededBy == regenerated.id)
    #expect(turns[1].content == "First answer")
    #expect(turns[2].supersededBy == nil)
}

@Test func sessionMutationsThrowForUnknownAndTombstonedIDs() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))

    await #expect(throws: ChatStoreError.sessionNotFound("ghost")) {
        try await store.renameSession(id: "ghost", title: "new title")
    }
    await #expect(throws: ChatStoreError.sessionNotFound("ghost")) {
        try await store.setPinned(id: "ghost", true)
    }
    await #expect(throws: ChatStoreError.sessionNotFound("ghost")) {
        try await store.deleteSession(id: "ghost")
    }

    let session = try await store.createSession(title: "alive")
    try await store.deleteSession(id: session.id)
    await #expect(throws: ChatStoreError.sessionNotFound(session.id)) {
        try await store.renameSession(id: session.id, title: "still there?")
    }
}

@Test func shadowModeReportsDegradedPersistenceUntilTheQueueFlushes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let blocked = dir.appendingPathComponent("store")
    try Data().write(to: blocked)
    let store = ChatStore(databaseURL: blocked.appendingPathComponent("chats.sqlite"))

    #expect(await !store.persistenceDegraded())
    _ = try await store.createSession(title: "shadowed")
    #expect(await store.persistenceDegraded())

    try FileManager.default.removeItem(at: blocked)
    try FileManager.default.createDirectory(at: blocked, withIntermediateDirectories: true)
    _ = try await store.sessions()
    #expect(await !store.persistenceDegraded())
}

@Test func importRecountsTurnCountFromTheArchiveTurns() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(title: "imported")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "one"), to: session.id)
    _ = try await store.appendTurn(TurnDraft(role: .assistant, content: "two"), to: session.id)
    let transcript = try #require(try await store.session(id: session.id))
    var mangled = transcript.session
    mangled.turnCount = 99
    let doctored = ChatTranscript(session: mangled, turns: transcript.turns)

    let imported = try await store.importTranscript(doctored)
    #expect(imported.turnCount == 2)
    let reloaded = try #require(try await store.session(id: session.id))
    #expect(reloaded.session.turnCount == 2)
}

@Test func placeColumnMigratesPersistsClearsAndExports() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: "model-a")
    #expect(session.place == nil)

    try await store.setPlace(id: session.id, place: "/tmp/hedos-place")
    let placed = try #require(try await store.session(id: session.id)).session
    #expect(placed.place == "/tmp/hedos-place")

    let reloaded = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let persisted = try #require(try await reloaded.session(id: session.id)).session
    #expect(persisted.place == "/tmp/hedos-place")

    _ = try await reloaded.appendTurn(TurnDraft(role: .user, content: "hi"), to: session.id)
    let archive = try ChatExport.json(try #require(try await reloaded.session(id: session.id)))
    #expect(String(decoding: archive, as: UTF8.self).contains("hedos-place"))

    try await reloaded.setPlace(id: session.id, place: nil)
    let cleared = try #require(try await reloaded.session(id: session.id)).session
    #expect(cleared.place == nil)

    await #expect(throws: ChatStoreError.self) {
        try await reloaded.setPlace(id: "no-such-session", place: "/tmp/x")
    }
}

@Test func intentAndModeModelsPersistAndSurviveReopen() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = makeStore(in: dir)
    let session = try await store.createSession(title: "Modes", modelID: "chat-model")
    #expect(session.intent == .text)
    #expect(session.imageModelID == nil)
    #expect(session.voiceModelID == nil)

    try await store.setIntent(id: session.id, intent: .image)
    try await store.bindImageModel(id: session.id, modelID: "sdxl")
    try await store.bindVoiceModel(id: session.id, modelID: "kokoro")

    let reopened = makeStore(in: dir)
    let restored = try #require(try await reopened.session(id: session.id)).session
    #expect(restored.intent == .image)
    #expect(restored.imageModelID == "sdxl")
    #expect(restored.voiceModelID == "kokoro")
    #expect(restored.modelID == "chat-model")

    try await reopened.bindImageModel(id: session.id, modelID: nil)
    let cleared = try #require(try await makeStore(in: dir).session(id: session.id)).session
    #expect(cleared.imageModelID == nil)
    #expect(cleared.intent == .image)

    await #expect(throws: ChatStoreError.self) {
        try await reopened.setIntent(id: "no-such-session", intent: .speak)
    }
}

@Test func sessionsFromBeforeIntentColumnDefaultToText() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("chats.sqlite")
    do {
        let legacy = try ChatDatabase(url: url)
        try legacy.execute("PRAGMA journal_mode=WAL")
        let priorMigrations = Array(ChatStore.migrations.dropLast())
        for migration in priorMigrations { try legacy.execute(migration) }
        try legacy.setUserVersion(priorMigrations.count)
        _ = try legacy.run(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            [.text("legacy"), .text("Old world"), .real(0), .real(0)])
    }

    let store = ChatStore(databaseURL: url)
    let restored = try #require(try await store.session(id: "legacy")).session
    #expect(restored.intent == .text)
    #expect(restored.imageModelID == nil)
    #expect(restored.voiceModelID == nil)
    #expect(try await store.sessions(filter: .all).contains { $0.id == "legacy" })
}

@Test func legacySessionJSONDecodesWithModeDefaults() throws {
    let json = """
        {"id":"s","title":"t","createdAt":0,"updatedAt":0,"capabilityTags":[],
         "turnCount":0,"pinned":false,"archived":false}
        """
    let decoded = try JSONDecoder().decode(ChatSession.self, from: Data(json.utf8))
    #expect(decoded.intent == .text)
    #expect(decoded.imageModelID == nil)
    #expect(decoded.voiceModelID == nil)

    let roundTripped = try JSONDecoder().decode(
        ChatSession.self, from: try JSONEncoder().encode(decoded))
    #expect(roundTripped.intent == .text)
}
