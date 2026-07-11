import Foundation
import Testing

@testable import HedosKernel

private func seededSession(
    _ store: ChatStore
) async throws -> (session: ChatSession, user: ChatTurn, assistant: ChatTurn) {
    let session = try await store.createSession(modelID: "model-a")
    let user = try await store.appendTurn(
        TurnDraft(role: .user, content: "original question"), to: session.id)
    let assistant = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "original answer", modelID: "model-a"),
        to: session.id)
    return (session, user, assistant)
}

private func textFlow(_ store: ChatStore, reply: String) -> ChatFlow {
    ChatFlow(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.text(reply))
                continuation.yield(.done(GenerationStats(completionTokens: 3, durationMs: 90)))
                continuation.finish()
            }
        },
        shelf: { [] })
}

@Test func editUserTurnRetiresOldChainAndRegenerates() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let (session, user, assistant) = try await seededSession(store)
    let flow = textFlow(store, reply: "revised answer")

    for try await _ in try await flow.editUserTurn(
        sessionID: session.id, turnID: user.id, text: "revised question") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    let active = turns.filter { $0.supersededBy == nil }
    #expect(active.map(\.role) == [.user, .assistant])
    #expect(active[0].content == "revised question")
    #expect(active[1].content == "revised answer")

    let retiredUser = try #require(turns.first { $0.id == user.id })
    let retiredAssistant = try #require(turns.first { $0.id == assistant.id })
    #expect(retiredUser.supersededBy == active[0].id)
    #expect(retiredAssistant.supersededBy == active[1].id)
    #expect(retiredUser.content == "original question")
    #expect(retiredAssistant.content == "original answer")
}

@Test func editUserTurnKeepsTheOldChainWhenTheStreamFails() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let (session, user, assistant) = try await seededSession(store)

    let failing = ChatFlow(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.finish(throwing: KernelError.runtimeUnavailable(hint: "down"))
            }
        },
        shelf: { [] })
    await #expect(throws: KernelError.self) {
        for try await _ in try await failing.editUserTurn(
            sessionID: session.id, turnID: user.id, text: "revised question") {}
    }

    let turns = try #require(try await store.session(id: session.id)).turns
    let originalUser = try #require(turns.first { $0.id == user.id })
    let originalAssistant = try #require(turns.first { $0.id == assistant.id })
    #expect(originalUser.supersededBy == nil)
    #expect(originalAssistant.supersededBy == nil)

    let flow = textFlow(store, reply: "revised answer")
    for try await _ in try await flow.editUserTurn(
        sessionID: session.id, turnID: user.id, text: "revised question again") {}

    let after = try #require(try await store.session(id: session.id)).turns
    let active = after.filter { $0.supersededBy == nil }
    #expect(active.map(\.role) == [.user, .assistant])
    #expect(active[0].content == "revised question again")
    #expect(active[1].content == "revised answer")
}

@Test func regenerateRetiresAssistantTurnOnlyOnceReplacementLands() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let (session, _, assistant) = try await seededSession(store)

    let failing = ChatFlow(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.finish(throwing: KernelError.runtimeUnavailable(hint: "down"))
            }
        },
        shelf: { [] })
    await #expect(throws: KernelError.self) {
        for try await _ in try await failing.regenerate(
            sessionID: session.id, turnID: assistant.id) {}
    }
    let afterFailure = try #require(try await store.session(id: session.id)).turns
    #expect(afterFailure.count == 2)
    #expect(afterFailure.allSatisfy { $0.supersededBy == nil })

    let flow = textFlow(store, reply: "second opinion")
    for try await _ in try await flow.regenerate(
        sessionID: session.id, turnID: assistant.id) {}

    let turns = try #require(try await store.session(id: session.id)).turns
    let active = turns.filter { $0.supersededBy == nil }
    #expect(active.map(\.content) == ["original question", "second opinion"])
    let retired = try #require(turns.first { $0.id == assistant.id })
    #expect(retired.supersededBy == active[1].id)
}

@Test func statsCarryTTFTAndSurviveStorage() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: "model-a")
    let flow = textFlow(store, reply: "measured")

    for try await _ in try await flow.send(sessionID: session.id, text: "time me") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    let stats = try #require(turns.last?.stats)
    #expect(stats.completionTokens == 3)
    #expect(stats.durationMs == 90)
    #expect(stats.ttftMs != nil)
    #expect(stats.ttftMs! >= 0)
}

@Test func jsonExportDeleteReimportRoundTripsByteEqual() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let (session, _, _) = try await seededSession(store)
    let flow = textFlow(store, reply: "with stats")
    for try await _ in try await flow.send(sessionID: session.id, text: "more") {}

    let exported = try #require(try await store.session(id: session.id))
    let archive = try ChatExport.json(exported)

    try await store.deleteSession(id: session.id)
    #expect(try await store.session(id: session.id) == nil)

    let restored = try await store.importTranscript(ChatExport.decode(archive))
    #expect(restored.id == session.id)
    let reexported = try #require(try await store.session(id: session.id))
    #expect(try ChatExport.json(reexported) == archive)
}

@Test func markdownExportListsTitleMetadataAndActiveTurns() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let (session, user, _) = try await seededSession(store)
    try await store.renameSession(id: session.id, title: "Original Thoughts")
    let flow = textFlow(store, reply: "revised answer")
    for try await _ in try await flow.editUserTurn(
        sessionID: session.id, turnID: user.id, text: "revised question") {}

    let transcript = try #require(try await store.session(id: session.id))
    let markdown = ChatExport.markdown(transcript)

    #expect(markdown.hasPrefix("# Original Thoughts"))
    #expect(markdown.contains("model: model-a"))
    #expect(markdown.contains("## User"))
    #expect(markdown.contains("revised question"))
    #expect(markdown.contains("revised answer"))
    #expect(!markdown.contains("original question"))
}

@Test func markdownParserSplitsBlocksAndSurvivesUnclosedFence() {
    let text = """
        # Title

        A paragraph with **bold** text
        continuing on a second line.

        - one
        - two

        1. first
        2. second

        > a quote
        > continued

        | Name | Size |
        | --- | --- |
        | tiny | 1 GB |
        | big | 40 GB |

        ---

        ```swift
        let x = 1
        """

    let blocks = MarkdownBlocks.parse(text)

    #expect(blocks.count == 8)
    #expect(blocks[0] == .heading(level: 1, text: "Title"))
    #expect(
        blocks[1]
            == .paragraph("A paragraph with **bold** text\ncontinuing on a second line."))
    #expect(blocks[2] == .list(items: ["one", "two"], ordered: false))
    #expect(blocks[3] == .list(items: ["first", "second"], ordered: true))
    #expect(blocks[4] == .quote("a quote\ncontinued"))
    #expect(
        blocks[5]
            == .table(header: ["Name", "Size"], rows: [["tiny", "1 GB"], ["big", "40 GB"]]))
    #expect(blocks[6] == .rule)
    #expect(blocks[7] == .code(language: "swift", code: "let x = 1", closed: false))
}

@Test func markdownParserIsStreamingStableAcrossPrefixes() {
    let text = "Intro paragraph.\n\n```python\nprint(1)\nprint(2)\n```\n\nOutro."
    var previousBlocks: [MarkdownBlock] = []
    for length in 0...text.count {
        let prefix = String(text.prefix(length))
        let blocks = MarkdownBlocks.parse(prefix)
        if blocks.count >= 2 {
            #expect(blocks[0] == previousBlocks.first ?? blocks[0])
        }
        previousBlocks = blocks
    }
    #expect(
        previousBlocks == [
            .paragraph("Intro paragraph."),
            .code(language: "python", code: "print(1)\nprint(2)", closed: true),
            .paragraph("Outro."),
        ])
}

@Test func highlighterClassifiesKeywordsStringsCommentsNumbers() {
    let tokens = CodeHighlighter.tokens(
        "let x = \"hi\" // note\nreturn 42", language: "swift")

    func kinds(of text: String) -> [CodeTokenKind] {
        tokens.filter { $0.text == text }.map(\.kind)
    }
    #expect(kinds(of: "let") == [.keyword])
    #expect(tokens.contains { $0.kind == .plain && $0.text.contains("x") })
    #expect(kinds(of: "\"hi\"") == [.string])
    #expect(kinds(of: "// note") == [.comment])
    #expect(kinds(of: "return") == [.keyword])
    #expect(kinds(of: "42") == [.number])
    #expect(tokens.map(\.text).joined() == "let x = \"hi\" // note\nreturn 42")

    let python = CodeHighlighter.tokens("# comment\nname2 = 3", language: "python")
    #expect(python.first?.kind == .comment)
    #expect(python.contains { $0.kind == .plain && $0.text.contains("name2") })
    #expect(python.contains { $0.text == "3" && $0.kind == .number })
}
