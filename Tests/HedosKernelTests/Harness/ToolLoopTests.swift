import Foundation
import Testing

@testable import HedosKernel

private final class ScriptedStreams: @unchecked Sendable {
    private let lock = NSLock()
    private var passes: [[CapabilityChunk]]
    private(set) var toolsSeen: [[ToolSpec]] = []
    private(set) var historiesSeen: [[ChatMessage]] = []

    init(passes: [[CapabilityChunk]]) {
        self.passes = passes
    }

    func next(
        _ messages: [ChatMessage], _ tools: [ToolSpec]
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        lock.lock()
        toolsSeen.append(tools)
        historiesSeen.append(messages)
        let chunks = passes.isEmpty ? [] : passes.removeFirst()
        lock.unlock()
        return AsyncThrowingStream { continuation in
            for chunk in chunks {
                continuation.yield(chunk)
            }
            continuation.finish()
        }
    }
}

private final class ExecutedCalls: @unchecked Sendable {
    private let lock = NSLock()
    private(set) var calls: [ToolCall] = []
    var result: @Sendable (ToolCall) -> String = { call in "result for \(call.name)" }
    var hang = false

    func record(_ call: ToolCall) async -> String {
        lock.withLock { calls.append(call) }
        if hang {
            try? await Task.sleep(for: .seconds(30))
        }
        return result(call)
    }
}

private func loopStore() throws -> (store: ChatStore, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    return (ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite")), dir)
}

private func loopFlow(
    store: ChatStore, streams: ScriptedStreams, executed: ExecutedCalls,
    tools: [ToolSpec] = HarnessTools.specs()
) -> ChatFlow {
    ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in streams.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in tools },
        execute: { _, call in await executed.record(call) })
}

private func timeCall(id: String = "call-1") -> ToolCall {
    ToolCall(id: id, name: "get_time", arguments: .object(["zone": .string("UTC")]))
}

@Test func callResultAnswerRoundTripPersistsTurnsInOrder() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let call = timeCall()
    let streams = ScriptedStreams(passes: [
        [.toolCall(call), .done(GenerationStats(finishReason: "tool_calls"))],
        [.text("It is noon."), .done(nil)],
    ])
    let executed = ExecutedCalls()
    let flow = loopFlow(store: store, streams: streams, executed: executed)

    for try await _ in try await flow.send(sessionID: session.id, text: "time?") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user, .assistant, .tool, .assistant])
    #expect(turns[1].toolCalls == [call])
    #expect(turns[2].toolCallID == call.id)
    #expect(turns[2].toolName == "get_time")
    #expect(turns[2].content == "result for get_time")
    #expect(turns[3].content == "It is noon.")
    #expect(executed.calls == [call])

    #expect(streams.historiesSeen.count == 2)
    let second = streams.historiesSeen[1]
    #expect(second.contains { $0.role == .tool && $0.content == "result for get_time" })
    #expect(second.contains { !$0.toolCalls.isEmpty })
}

@Test func iterationCapWithholdsToolsOnTheFinalPass() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    var passes: [[CapabilityChunk]] = []
    for index in 0..<ChatFlow.maxToolCallsReadOnly {
        passes.append([.toolCall(timeCall(id: "call-\(index)")), .done(nil)])
    }
    passes.append([.text("Final answer."), .done(nil)])
    let streams = ScriptedStreams(passes: passes)
    let executed = ExecutedCalls()
    let flow = loopFlow(store: store, streams: streams, executed: executed)

    for try await _ in try await flow.send(sessionID: session.id, text: "loop forever") {}

    #expect(executed.calls.count == ChatFlow.maxToolCallsReadOnly)
    #expect(streams.toolsSeen.count == ChatFlow.maxToolCallsReadOnly + 1)
    #expect(streams.toolsSeen.last?.isEmpty == true)

    let turns = try #require(try await store.session(id: session.id)).turns
    let lastToolTurn = try #require(turns.last { $0.role == .tool })
    #expect(lastToolTurn.content.contains("tool-call limit reached"))
    #expect(turns.last?.content == "Final answer.")
}

@Test func cancelMidLoopLeavesOnlyWholeTurns() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let streams = ScriptedStreams(passes: [
        [.toolCall(timeCall()), .done(nil)],
        [.text("never reached"), .done(nil)],
    ])
    let executed = ExecutedCalls()
    executed.hang = true
    let flow = loopFlow(store: store, streams: streams, executed: executed)

    let stream = try await flow.send(sessionID: session.id, text: "time?")
    let consumer = Task {
        for try await _ in stream {}
    }
    for _ in 0..<200 {
        if !executed.calls.isEmpty { break }
        try await Task.sleep(for: .milliseconds(10))
    }
    #expect(!executed.calls.isEmpty)
    consumer.cancel()
    _ = try? await consumer.value

    for _ in 0..<200 {
        let turns = try #require(try await store.session(id: session.id)).turns
        if turns.contains(where: { $0.role == .tool }) { break }
        try await Task.sleep(for: .milliseconds(10))
    }
    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user, .assistant, .tool])
    #expect(turns[1].toolCalls == [timeCall()])
    #expect(turns[2].toolCallID == timeCall().id)
    #expect(turns[2].content.contains("cancelled before this tool ran"))

    let followup = ScriptedStreams(passes: [[.text("still works"), .done(nil)]])
    let plainFlow = loopFlow(
        store: store, streams: followup, executed: ExecutedCalls(), tools: [])
    for try await _ in try await plainFlow.send(sessionID: session.id, text: "again") {}
    let after = try #require(try await store.session(id: session.id)).turns
    #expect(after.last?.content == "still works")
}

@Test func outOfPlaceCallComesBackAsRefusalAndTheConversationContinues() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    let place = PlaceBoundary.canonical(placeDir.path)
    let session = try await store.createSession(modelID: "model-a")
    let escape = ToolCall(
        id: "call-esc", name: "read_file",
        arguments: .object(["path": .string("/etc/passwd")]))
    let streams = ScriptedStreams(passes: [
        [.toolCall(escape), .done(nil)],
        [.text("Understood, I cannot read that."), .done(nil)],
    ])
    let executed = ExecutedCalls()
    executed.result = { call in "" }
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in streams.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in HarnessTools.specs() },
        execute: { sessionID, call in await Harness.execute(call, place: place, context: HarnessActContext(sessionID: sessionID, ask: alwaysDeclineConsent, state: HarnessActState())) })

    for try await _ in try await flow.send(sessionID: session.id, text: "read passwd") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    let toolTurn = try #require(turns.first { $0.role == .tool })
    #expect(toolTurn.content.contains("outside this conversation's folder"))
    #expect(turns.last?.content == "Understood, I cannot read that.")
    #expect(!FileManager.default.fileExists(atPath: place + "/etc/passwd"))
}

@Test func toolResultsTruncateToTheContextBudgetWithAStatement() {
    let huge = String(repeating: "x", count: ChatFlow.toolResultContextBudgetBytes + 1000)
    let truncated = ChatFlow.truncatedToolResult(huge)
    #expect(
        truncated.hasPrefix(
            "[truncated: showing first \(ChatFlow.toolResultContextBudgetBytes) of \(ChatFlow.toolResultContextBudgetBytes + 1000) bytes]"
        ))
    #expect(truncated.utf8.count <= ChatFlow.toolResultContextBudgetBytes + 100)

    let small = "small result"
    #expect(ChatFlow.truncatedToolResult(small) == small)
}

@Test func conversationWithoutToolsCarriesNoToolsKeyAtThePayloadLevel() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let streams = ScriptedStreams(passes: [[.text("plain"), .done(nil)]])
    let flow = loopFlow(
        store: store, streams: streams, executed: ExecutedCalls(), tools: [])

    for try await _ in try await flow.send(sessionID: session.id, text: "hi") {}
    #expect(streams.toolsSeen == [[]])
}

@Test func parallelCallsStraddlingTheCapAllGetResultTurns() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    var passes: [[CapabilityChunk]] = []
    for index in 0..<(ChatFlow.maxToolCallsReadOnly - 1) {
        passes.append([.toolCall(timeCall(id: "call-\(index)")), .done(nil)])
    }
    passes.append([
        .toolCall(timeCall(id: "straddle-a")),
        .toolCall(timeCall(id: "straddle-b")),
        .toolCall(timeCall(id: "straddle-c")),
        .done(nil),
    ])
    passes.append([.text("Final answer."), .done(nil)])
    let streams = ScriptedStreams(passes: passes)
    let executed = ExecutedCalls()
    let flow = loopFlow(store: store, streams: streams, executed: executed)

    for try await _ in try await flow.send(sessionID: session.id, text: "go") {}

    #expect(executed.calls.count == ChatFlow.maxToolCallsReadOnly)
    let turns = try #require(try await store.session(id: session.id)).turns
    let advertised = turns.flatMap(\.toolCalls).map(\.id)
    let answered = turns.filter { $0.role == .tool }.compactMap(\.toolCallID)
    #expect(Set(advertised) == Set(answered))
    let skipped = turns.filter { $0.role == .tool && $0.content.contains("skipped") }
    #expect(skipped.count == 2)
    #expect(turns.last?.content == "Final answer.")
}

@Test func mentionedFileIsReadBeforeTheModelStreams() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    try Data("# Zephyrwing docs".utf8).write(to: placeDir.appendingPathComponent("README.md"))
    let place = PlaceBoundary.canonical(placeDir.path)
    let session = try await store.createSession(modelID: "model-a")
    try await store.setPlace(id: session.id, place: place)

    let streams = ScriptedStreams(passes: [[.text("It documents Zephyrwing."), .done(nil)]])
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in streams.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in HarnessTools.specs() },
        execute: { sessionID, call in await Harness.execute(call, place: place, context: HarnessActContext(sessionID: sessionID, ask: alwaysDeclineConsent, state: HarnessActState())) })

    for try await _ in try await flow.send(
        sessionID: session.id, text: "what does @README.md say?") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user, .assistant, .tool, .assistant])
    #expect(turns[1].toolCalls.first?.name == "read_file")
    #expect(turns[2].toolName == "read_file")
    #expect(turns[2].content.contains("Zephyrwing docs"))
    #expect(turns[3].content == "It documents Zephyrwing.")

    let history = streams.historiesSeen[0]
    #expect(history.contains { $0.role == .tool && $0.content.contains("Zephyrwing docs") })
}

@Test func actEnabledConversationRaisesTheToolCallCap() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    var passes: [[CapabilityChunk]] = []
    for index in 0..<ChatFlow.maxToolCallsActing {
        passes.append([.toolCall(timeCall(id: "call-\(index)")), .done(nil)])
    }
    passes.append([.text("Done."), .done(nil)])
    let streams = ScriptedStreams(passes: passes)
    let executed = ExecutedCalls()
    let flow = loopFlow(
        store: store, streams: streams, executed: executed,
        tools: HarnessTools.specs() + HarnessActTools.specs())

    for try await _ in try await flow.send(sessionID: session.id, text: "keep going") {}

    #expect(executed.calls.count == ChatFlow.maxToolCallsActing)
    #expect(ChatFlow.maxToolCallsActing > ChatFlow.maxToolCallsReadOnly)
}

@Test func theLoopYieldsAVisibleStepCount() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let streams = ScriptedStreams(passes: [
        [.toolCall(timeCall()), .done(nil)],
        [.text("noon"), .done(nil)],
    ])
    let flow = loopFlow(store: store, streams: streams, executed: ExecutedCalls())

    var statuses: [String] = []
    for try await chunk in try await flow.send(sessionID: session.id, text: "time?") {
        if case .status(let text) = chunk { statuses.append(text) }
    }
    #expect(statuses.contains { $0.contains("step 1 of \(ChatFlow.maxToolCallsReadOnly)") })
}

@Test func consentIsAwaitedInsideTheLoopWithoutDeadlockingCancellation() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    let place = PlaceBoundary.canonical(placeDir.path)
    let session = try await store.createSession(modelID: "model-a")
    let write = ToolCall(
        id: "call-w", name: "write_file",
        arguments: .object(["path": .string("note.txt"), "content": .string("hi\n")]))
    let streams = ScriptedStreams(passes: [
        [.toolCall(write), .done(nil)],
        [.text("Wrote it."), .done(nil)],
    ])
    let state = HarnessActState()
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in streams.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in HarnessTools.specs() + HarnessActTools.specs() },
        execute: { sessionID, call in
            await Harness.execute(
                call, place: place,
                context: HarnessActContext(
                    sessionID: sessionID,
                    ask: { _ in .approved(dontAskAgain: false) },
                    state: state))
        })

    for try await _ in try await flow.send(sessionID: session.id, text: "write note") {}

    #expect((try? String(contentsOfFile: place + "/note.txt")) == "hi\n")
    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.last?.content == "Wrote it.")
}

@Test func mentionScanRulesResolveStripAndCap() throws {
    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "plain"] {
        try Data("x".utf8).write(to: placeDir.appendingPathComponent(name))
    }
    let place = PlaceBoundary.canonical(placeDir.path)

    #expect(ChatFlow.mentionedFiles(in: "see a.txt, and (b.txt)!", place: place) == ["a.txt", "b.txt"])
    #expect(ChatFlow.mentionedFiles(in: "@plain please", place: place) == ["plain"])
    #expect(ChatFlow.mentionedFiles(in: "the plain truth", place: place) == [])
    #expect(ChatFlow.mentionedFiles(in: "a.txt a.txt", place: place) == ["a.txt"])
    #expect(
        ChatFlow.mentionedFiles(
            in: "a.txt b.txt c.txt d.txt e.txt", place: place
        ).count == ChatFlow.maxMentionReadsPerSend)
    #expect(ChatFlow.mentionedFiles(in: "read /etc/passwd", place: place) == [])
    #expect(ChatFlow.mentionedFiles(in: "read a.txt", place: nil) == [])
}

@Test func mentionOfAMissingFilePersistsAnHonestResultAndContinues() async throws {
    let (store, dir) = try loopStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    try Data("x".utf8).write(to: placeDir.appendingPathComponent("real.txt"))
    let place = PlaceBoundary.canonical(placeDir.path)
    let session = try await store.createSession(modelID: "model-a")
    try await store.setPlace(id: session.id, place: place)

    let streams = ScriptedStreams(passes: [[.text("Understood."), .done(nil)]])
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, offered, _ in streams.next(messages, offered) },
        shelf: { [] },
        toolbox: { _ in HarnessTools.specs() },
        execute: { sessionID, call in await Harness.execute(call, place: place, context: HarnessActContext(sessionID: sessionID, ask: alwaysDeclineConsent, state: HarnessActState())) })

    for try await _ in try await flow.send(
        sessionID: session.id, text: "read @gone.txt now") {}

    let turns = try #require(try await store.session(id: session.id)).turns
    let toolTurn = try #require(turns.first { $0.role == .tool })
    #expect(toolTurn.content.contains("does not exist"))
    #expect(turns.last?.content == "Understood.")
}
