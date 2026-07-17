import Foundation
import Testing

@testable import HedosKernel

private func daemonReachable() async -> Bool {
    var request = URLRequest(url: URL(string: "http://127.0.0.1:11434/api/tags")!)
    request.timeoutInterval = 2
    guard let (data, response) = try? await URLSession.shared.data(for: request),
        (response as? HTTPURLResponse)?.statusCode == 200
    else { return false }
    return String(decoding: data, as: UTF8.self).contains("qwen3.5:9b")
}

private func qwenRecord() -> ModelRecord {
    ModelRecord(
        name: "qwen3.5:9b",
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(kind: .ollama, path: "/fake/ollama", repo: "qwen3.5:9b"),
        runtime: RuntimeRef(id: .ollama, resolved: .auto, tier: .native),
        execution: .stream,
        state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Suite(.serialized) struct HarnessOllamaLive {

@Test(.timeLimit(.minutes(10))) func liveQwenReadsThePlaceAndAnswersFromAFile() async throws {
    guard await daemonReachable() else { return }

    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    try Data(
        "# Zephyrwing\n\nZephyrwing is a paper-airplane flight simulator for macOS.\n".utf8
    ).write(to: placeDir.appendingPathComponent("README.md"))
    try Data("let wingspan = 21\n".utf8).write(to: placeDir.appendingPathComponent("main.swift"))
    let place = PlaceBoundary.canonical(placeDir.path)

    let storeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: storeDir) }
    let store = ChatStore(databaseURL: storeDir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: qwenRecord().id)

    let record = qwenRecord()
    let adapter = OllamaAdapter()
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, tools, _ in
            var payload: [String: JSONValue] = [
                "messages": .array(messages.map(\.payloadValue))
            ]
            if !tools.isEmpty {
                payload["tools"] = .array(tools.map(\.payloadValue))
            }
            return adapter.invoke(record, .chat, payload: .object(payload))
        },
        shelf: { [record] },
        toolbox: { _ in HarnessTools.specs() },
        execute: { sessionID, call in ToolOutcome(text: await Harness.execute(call, place: place, context: HarnessActContext(sessionID: sessionID, ask: alwaysDeclineConsent, state: HarnessActState()))) })

    func grounded(_ turns: [ChatTurn]) -> Bool {
        guard turns.contains(where: { $0.role == .tool }),
            let answer = turns.last(where: { $0.role == .assistant }),
            !answer.content.isEmpty
        else { return false }
        let lowered = answer.content.lowercased()
        return lowered.contains("zephyrwing") || lowered.contains("paper")
    }
    var attempts = 0
    var turns: [ChatTurn] = []
    repeat {
        attempts += 1
        do {
            for try await _ in try await flow.send(
                sessionID: session.id,
                text: "What is this project about? Use the read_file tool on README.md to find out.")
            {}
        } catch {
            if attempts >= 3 { throw error }
            continue
        }
        turns = try #require(try await store.session(id: session.id)).turns
    } while !grounded(turns) && attempts < 3
    #expect(grounded(turns))

    for try await _ in try await flow.send(
        sessionID: session.id, text: "And what is the wingspan constant in main.swift?") {}
    let after = try #require(try await store.session(id: session.id)).turns
    let followup = try #require(after.last { $0.role == .assistant })
    #expect(!followup.content.isEmpty)
}

@Test(.timeLimit(.minutes(6))) func liveQwenAnswersFromAnAtMention() async throws {
    guard await daemonReachable() else { return }

    let placeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: placeDir) }
    try Data(
        "# Zephyrwing\n\nZephyrwing is a paper-airplane flight simulator for macOS.\n".utf8
    ).write(to: placeDir.appendingPathComponent("README.md"))
    let place = PlaceBoundary.canonical(placeDir.path)

    let storeDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: storeDir) }
    let store = ChatStore(databaseURL: storeDir.appendingPathComponent("chats.sqlite"))
    let session = try await store.createSession(modelID: qwenRecord().id)
    try await store.setPlace(id: session.id, place: place)

    let record = qwenRecord()
    let adapter = OllamaAdapter()
    let flow = ChatFlow(
        chats: store,
        stream: { _, messages, tools, _ in
            var payload: [String: JSONValue] = [
                "messages": .array(messages.map(\.payloadValue))
            ]
            if !tools.isEmpty {
                payload["tools"] = .array(tools.map(\.payloadValue))
            }
            return adapter.invoke(record, .chat, payload: .object(payload))
        },
        shelf: { [record] },
        toolbox: { _ in HarnessTools.specs() },
        execute: { sessionID, call in ToolOutcome(text: await Harness.execute(call, place: place, context: HarnessActContext(sessionID: sessionID, ask: alwaysDeclineConsent, state: HarnessActState()))) })

    func grounded(_ turns: [ChatTurn]) -> Bool {
        guard turns.contains(where: { $0.role == .tool && $0.content.contains("Zephyrwing") }),
            let answer = turns.last(where: { $0.role == .assistant }),
            !answer.content.isEmpty
        else { return false }
        let lowered = answer.content.lowercased()
        return lowered.contains("zephyrwing") || lowered.contains("paper")
    }
    var attempts = 0
    var turns: [ChatTurn] = []
    repeat {
        attempts += 1
        do {
            for try await _ in try await flow.send(
                sessionID: session.id, text: "what does @README.md say?") {}
        } catch {
            if attempts >= 3 { throw error }
            continue
        }
        turns = try #require(try await store.session(id: session.id)).turns
    } while !grounded(turns) && attempts < 3
    #expect(grounded(turns))
}

}
