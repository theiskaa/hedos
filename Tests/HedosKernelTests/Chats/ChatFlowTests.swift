import Foundation
import Testing

@testable import HedosKernel

private func flowStore() throws -> (store: ChatStore, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    return (ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite")), dir)
}

private func cannedTitling(
    store: ChatStore,
    shelf: [ModelRecord] = [],
    asked: AskedModels? = nil,
    chunks: @escaping @Sendable (String) -> [CapabilityChunk]
) -> ChatTitling {
    ChatTitling(
        chats: store,
        stream: { modelID, _ in
            await asked?.record(modelID)
            return AsyncThrowingStream { continuation in
                for chunk in chunks(modelID) {
                    continuation.yield(chunk)
                }
                continuation.finish()
            }
        },
        shelf: { shelf })
}

private func cannedFlow(
    store: ChatStore,
    shelf: [ModelRecord] = [],
    asked: AskedModels? = nil,
    chunks: @escaping @Sendable (String) -> [CapabilityChunk]
) -> ChatFlow {
    ChatFlow(
        chats: store,
        stream: { modelID, _, _ in
            await asked?.record(modelID)
            return AsyncThrowingStream { continuation in
                for chunk in chunks(modelID) {
                    continuation.yield(chunk)
                }
                continuation.finish()
            }
        },
        shelf: { shelf })
}

private actor AskedModels {
    var ids: [String] = []
    func record(_ id: String) {
        ids.append(id)
    }
}

private func readyChatModel(path: String, footprintMB: Int?) -> ModelRecord {
    var record = Fixtures.gguf(path: path)
    record.runtime = RuntimeRef(id: "llama.cpp", resolved: .auto, tier: .native)
    record.state = .ready
    record.footprintMB = footprintMB
    return record
}

@Test func sendChatPersistsBothTurnsWithTrueModelIDs() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let flow = cannedFlow(store: store) { _ in
        [
            .thinking("pondering"),
            .text("Hello"),
            .text(" world"),
            .done(GenerationStats(completionTokens: 2, durationMs: 40)),
        ]
    }

    var received: [CapabilityChunk] = []
    for try await chunk in try await flow.send(sessionID: session.id, text: "hi there") {
        received.append(chunk)
    }
    #expect(received.count == 4)

    try await store.rebindSession(id: session.id, modelID: "model-b")
    for try await _ in try await flow.send(sessionID: session.id, text: "again") {}

    let transcript = try #require(try await store.session(id: session.id))
    let turns = transcript.turns
    #expect(turns.map(\.role) == [.user, .assistant, .user, .assistant])
    #expect(turns[0].content == "hi there")
    #expect(turns[1].content == "Hello world")
    #expect(turns[1].thinking == "pondering")
    #expect(turns[1].modelID == "model-a")
    #expect(turns[1].stats?.completionTokens == 2)
    #expect(turns[3].modelID == "model-b")
    #expect(transcript.session.turnCount == 4)
}

@Test func continueSessionStreamsFromStoredHistoryWithoutNewUserTurn() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "stranded question"), to: session.id)
    let flow = cannedFlow(store: store) { _ in [.text("recovered answer"), .done(nil)] }

    for try await _ in try await flow.continueSession(sessionID: session.id) {}

    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user, .assistant])
    #expect(turns[1].content == "recovered answer")
}

@Test func abandonedStreamPersistsPartialAssistantTurn() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let flow = ChatFlow(
        chats: store,
        stream: { _, _, _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.text("partial"))
            }
        },
        shelf: { [] })

    let stream = try await flow.send(sessionID: session.id, text: "long question")
    let consumer = Task {
        for try await chunk in stream {
            if case .text = chunk { break }
        }
    }
    try await consumer.value

    for _ in 0..<100 {
        let turns = try #require(try await store.session(id: session.id)).turns
        if turns.count == 2, turns[1].content == "partial" { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.count == 2)
    #expect(turns.last?.content == "partial")
}

@Test func failureBeforeAnyContentLeavesOnlyUserTurn() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    let flow = ChatFlow(
        chats: store,
        stream: { _, _, _ in
            AsyncThrowingStream { continuation in
                continuation.finish(throwing: KernelError.runtimeUnavailable(hint: "daemon down"))
            }
        },
        shelf: { [] })

    await #expect(throws: KernelError.self) {
        for try await _ in try await flow.send(sessionID: session.id, text: "hi") {}
    }

    let turns = try #require(try await store.session(id: session.id)).turns
    #expect(turns.map(\.role) == [.user])
}

@Test func sendChatWithoutBoundModelThrowsBeforeTouchingHistory() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession()
    let flow = cannedFlow(store: store) { _ in [] }

    await #expect(throws: KernelError.self) {
        _ = try await flow.send(sessionID: session.id, text: "hi")
    }
}

@Test func autoTitleAsksSmallestReadyChatModelAndSanitizes() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let big = readyChatModel(path: "~/models/big.gguf", footprintMB: 40000)
    let small = readyChatModel(path: "~/models/small.gguf", footprintMB: 2000)
    let session = try await store.createSession(modelID: big.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "how does the borrow checker work?"), to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "It tracks ownership.", modelID: big.id),
        to: session.id)
    let asked = AskedModels()
    let flow = cannedTitling(store: store, shelf: [big, small], asked: asked) { _ in
        [.text("  \"Rust Borrow Checker Explained In Depth Fully.\"\nextra line")]
    }

    let title = try await flow.autoTitleIfNeeded(sessionID: session.id)

    #expect(title == "Rust Borrow Checker Explained In Depth")
    #expect(await asked.ids == [small.id])
    let stored = try #require(try await store.session(id: session.id)).session
    #expect(stored.title == "Rust Borrow Checker Explained In Depth")
}

@Test func autoTitleUsesBoundModelWhenItIsSmall() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bound = readyChatModel(path: "~/models/bound.gguf", footprintMB: 4000)
    let tiny = readyChatModel(path: "~/models/tiny.gguf", footprintMB: 900)
    let session = try await store.createSession(modelID: bound.id)
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hello"), to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "hi", modelID: bound.id), to: session.id)
    let asked = AskedModels()
    let flow = cannedTitling(store: store, shelf: [tiny, bound], asked: asked) { _ in
        [.text("Friendly Greeting")]
    }

    _ = try await flow.autoTitleIfNeeded(sessionID: session.id)

    #expect(await asked.ids == [bound.id])
}

@Test func autoTitleFallsBackToFirstLineTruncation() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    _ = try await store.appendTurn(
        TurnDraft(role: .user, content: "explain quantum tunnelling\nwith detail"),
        to: session.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "sure", modelID: "model-a"), to: session.id)
    let flow = ChatTitling(
        chats: store,
        stream: { _, _ in
            AsyncThrowingStream { continuation in
                continuation.finish(throwing: KernelError.runtimeUnavailable(hint: "down"))
            }
        },
        shelf: { [readyChatModel(path: "~/models/small.gguf", footprintMB: 2000)] })

    let title = try await flow.autoTitleIfNeeded(sessionID: session.id)

    #expect(title == "explain quantum tunnelling")
}

@Test func autoTitleLeavesCustomTitlesAndUnfinishedExchangesAlone() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let flow = cannedTitling(store: store) { _ in [.text("Ignored")] }

    let renamed = try await store.createSession(modelID: "model-a")
    try await store.renameSession(id: renamed.id, title: "My Title")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hi"), to: renamed.id)
    _ = try await store.appendTurn(
        TurnDraft(role: .assistant, content: "hello", modelID: "model-a"), to: renamed.id)
    #expect(try await flow.autoTitleIfNeeded(sessionID: renamed.id) == nil)
    let renamedStored = try #require(try await store.session(id: renamed.id)).session
    #expect(renamedStored.title == "My Title")

    let unanswered = try await store.createSession(modelID: "model-a")
    _ = try await store.appendTurn(TurnDraft(role: .user, content: "hi"), to: unanswered.id)
    #expect(try await flow.autoTitleIfNeeded(sessionID: unanswered.id) == nil)
}

@Test func sanitizedTitleClipsQuotesPunctuationAndWordCount() {
    #expect(ChatTitling.sanitizedTitle("\"A Very Long Title With Many Extra Words\"") == "A Very Long Title With Many")
    #expect(ChatTitling.sanitizedTitle("Title: The Sequel!") == "Title: The Sequel")
    #expect(ChatTitling.sanitizedTitle("\n\n  Plain answer  \n") == "Plain answer")
    #expect(ChatTitling.sanitizedTitle("   ") == nil)
    #expect(ChatTitling.sanitizedTitle("") == nil)
}

@Test func rebindSessionUpdatesBindingAndPersists() async throws {
    let (store, dir) = try flowStore()
    defer { try? FileManager.default.removeItem(at: dir) }
    let session = try await store.createSession(modelID: "model-a")
    try await store.rebindSession(id: session.id, modelID: "model-b")

    let reloaded = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let stored = try #require(try await reloaded.session(id: session.id)).session
    #expect(stored.modelID == "model-b")
}

@Test func defaultChatModelSettingRoundTripsAndDrivesLauncher() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    #expect(await store.defaultChatModelID() == nil)
    try await store.setDefaultChatModelID("model-z")
    #expect(await SettingsStore(directory: dir).defaultChatModelID() == "model-z")

    let first = readyChatModel(path: "~/models/first.gguf", footprintMB: 3000)
    let preferred = readyChatModel(path: "~/models/preferred.gguf", footprintMB: 9000)
    let shelf = [first, preferred]
    #expect(Launcher.defaultChatModel(in: shelf)?.id == first.id)
    #expect(Launcher.defaultChatModel(in: shelf, preferring: preferred.id)?.id == preferred.id)
    #expect(Launcher.defaultChatModel(in: shelf, preferring: "missing")?.id == first.id)
}

@Test func persistFailureDuringStreamSurfacesAsAThrownStreamError() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ChatStore(databaseURL: dir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(title: "doomed", modelID: "m1")
    let (gate, gateContinuation) = AsyncStream.makeStream(of: Void.self)

    let flow = ChatFlow(
        chats: store,
        stream: { _, _, _ in
            AsyncThrowingStream { continuation in
                let task = Task {
                    var opened = gate.makeAsyncIterator()
                    _ = await opened.next()
                    continuation.yield(.text("hello"))
                    continuation.yield(.done(nil))
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        },
        shelf: { [] })

    let stream = try await flow.send(sessionID: session.id, text: "hi")
    try await store.deleteSession(id: session.id)
    gateContinuation.yield(())
    gateContinuation.finish()

    var text = ""
    do {
        for try await chunk in stream {
            if case .text(let delta) = chunk { text += delta }
        }
        Issue.record("a failed persist must surface as a stream error")
    } catch let error as ChatStoreError {
        #expect(error == .sessionNotFound(session.id))
    }
    #expect(text == "hello")
}
