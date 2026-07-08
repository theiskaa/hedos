import Foundation
import Testing

@testable import HedosKernel

private func session(
    id: String, updatedAt: Date, pinned: Bool = false, tags: [String] = []
) -> ChatSession {
    ChatSession(
        id: id,
        title: id,
        createdAt: updatedAt,
        updatedAt: updatedAt,
        capabilityTags: tags,
        pinned: pinned)
}

@Test func groupsSplitByAgeWithPinnedOnTopAndEmptyBucketsOmitted() throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = try #require(TimeZone(identifier: "Europe/Berlin"))
    let now = try #require(
        calendar.date(from: DateComponents(year: 2026, month: 7, day: 6, hour: 14)))
    let sessions = [
        session(id: "pinned-old", updatedAt: now.addingTimeInterval(-40 * 86400), pinned: true),
        session(id: "this-morning", updatedAt: now.addingTimeInterval(-6 * 3600)),
        session(id: "late-yesterday", updatedAt: now.addingTimeInterval(-16 * 3600)),
        session(id: "four-days-ago", updatedAt: now.addingTimeInterval(-4 * 86400)),
        session(id: "last-month", updatedAt: now.addingTimeInterval(-30 * 86400)),
    ]

    let groups = SessionGrouping.groups(sessions, now: now, calendar: calendar)

    #expect(groups.map(\.title) == ["Pinned", "Today", "Yesterday", "This Week", "Older"])
    #expect(groups[0].sessions.map(\.id) == ["pinned-old"])
    #expect(groups[1].sessions.map(\.id) == ["this-morning"])
    #expect(groups[2].sessions.map(\.id) == ["late-yesterday"])
    #expect(groups[3].sessions.map(\.id) == ["four-days-ago"])
    #expect(groups[4].sessions.map(\.id) == ["last-month"])

    let sparse = SessionGrouping.groups(
        [session(id: "only-today", updatedAt: now)], now: now, calendar: calendar)
    #expect(sparse.map(\.title) == ["Today"])
}

@Test func groupingPreservesStoreOrderWithinEachBucket() throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = try #require(TimeZone(identifier: "Europe/Berlin"))
    let now = try #require(
        calendar.date(from: DateComponents(year: 2026, month: 7, day: 6, hour: 14)))
    let newer = session(id: "newer", updatedAt: now.addingTimeInterval(-3600))
    let newest = session(id: "newest", updatedAt: now.addingTimeInterval(-60))

    let groups = SessionGrouping.groups([newest, newer], now: now, calendar: calendar)

    #expect(groups.map(\.title) == ["Today"])
    #expect(groups[0].sessions.map(\.id) == ["newest", "newer"])
}

@Test func thinkingTagLandsOnSessionEvenWhenTurnPredatesThinking() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: "model-a")
    let flow = ChatFlow(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.text("first"))
                continuation.yield(.thinking("hmm"))
                continuation.yield(.text(" second"))
                continuation.yield(.done(nil))
                continuation.finish()
            }
        },
        shelf: { [] })

    for try await _ in try await flow.send(sessionID: session.id, text: "question") {}

    let stored = try #require(try await store.session(id: session.id)).session
    #expect(stored.capabilityTags == [SessionTag.thinking])

    let plain = try await store.createSession(modelID: "model-a")
    let plainFlow = ChatFlow(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.text("no thoughts"))
                continuation.finish()
            }
        },
        shelf: { [] })
    for try await _ in try await plainFlow.send(sessionID: plain.id, text: "hi") {}
    let plainStored = try #require(try await store.session(id: plain.id)).session
    #expect(plainStored.capabilityTags.isEmpty)
}

@Test func renamePinArchivePersistAcrossReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = dir.appendingPathComponent("chats.sqlite")
    let store = ChatStore(databaseURL: url)
    let renamed = try await store.createSession(modelID: "model-a")
    let pinnedAndArchived = try await store.createSession(modelID: "model-a")
    try await store.renameSession(id: renamed.id, title: "Field Notes")
    try await store.setPinned(id: pinnedAndArchived.id, true)
    try await store.setArchived(id: pinnedAndArchived.id, true)

    let reloaded = ChatStore(databaseURL: url)
    let active = try await reloaded.sessions(filter: .active)
    let archived = try await reloaded.sessions(filter: .archived)
    #expect(active.map(\.id) == [renamed.id])
    #expect(active.first?.title == "Field Notes")
    #expect(archived.map(\.id) == [pinnedAndArchived.id])
    #expect(archived.first?.pinned == true)
}

@Test func listingSessionsTouchesNoTurnRows() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    for index in 0..<5 {
        let session = try await store.createSession(modelID: "model-a")
        _ = try await store.appendTurn(
            TurnDraft(role: .user, content: "question \(index)"), to: session.id)
        _ = try await store.appendTurn(
            TurnDraft(role: .assistant, content: "answer \(index)", modelID: "model-a"),
            to: session.id)
    }

    try await store.enableStatementLogging()
    try await store.resetStatementLog()
    let sessions = try await store.sessions()
    #expect(sessions.count == 5)

    let statements = try await store.statementLog()
    #expect(!statements.isEmpty)
    #expect(statements.allSatisfy { !$0.contains("turns") })
}
