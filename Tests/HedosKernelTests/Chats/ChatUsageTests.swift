import Foundation
import Testing

@testable import HedosKernel

@Test func usageByDayCountsChatMessagesAndTokensExcludingDeletedAndNonChatRoles() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))

    let session = try await store.createSession(title: "Active", modelID: "m")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hi"), to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(
            role: .assistant, content: "hello",
            statsJSON: GenerationStats(promptTokens: 10, completionTokens: 20).turnStatsJSON),
        to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(
            role: .assistant, content: "more",
            statsJSON: GenerationStats(promptTokens: 5, completionTokens: 15).turnStatsJSON),
        to: session.id)
    _ = try await store.appendTurn(TurnDraft(role: .tool, content: "tool output"), to: session.id)

    let deleted = try await store.createSession(title: "Deleted", modelID: "m")
    _ = try await store.appendTurn(
        TurnDraft(
            role: .assistant, content: "ghost",
            statsJSON: GenerationStats(promptTokens: 99, completionTokens: 99).turnStatsJSON),
        to: deleted.id)
    try await store.deleteSession(id: deleted.id)

    let usage = await store.usageByDay(since: Date(timeIntervalSince1970: 0))
    #expect(usage.count == 1)
    let today = try #require(usage.first)
    #expect(today.messages == 3)
    #expect(today.promptTokens == 15)
    #expect(today.completionTokens == 35)
    #expect(today.tokens == 50)
    #expect(today.day == Calendar.current.startOfDay(for: .now))
}

@Test func usageByDayIsEmptyWithNoChats() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let usage = await store.usageByDay(since: Date(timeIntervalSince1970: 0))
    #expect(usage.isEmpty)
}
