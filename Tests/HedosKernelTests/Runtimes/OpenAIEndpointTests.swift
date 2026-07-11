import Foundation
import Testing

@testable import HedosKernel

private func startSSEServer(mode: String = "open") throws -> (process: Process, port: Int) {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appendingPathComponent("FakeSSEServer.py")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["python3", script.path, mode]
    let stdout = Pipe()
    process.standardOutput = stdout
    process.standardError = Pipe()
    try process.run()
    guard let line = stdout.fileHandleForReading.availableData.split(separator: 0x0A).first,
        let port = Int(String(decoding: line, as: UTF8.self))
    else {
        process.terminate()
        throw KernelError.runtimeFailed("fake sse server never reported a port")
    }
    return (process, port)
}

private func endpointRecord(port: Int, model: String = "fake-chat-1") -> ModelRecord {
    ModelRecord(
        name: model,
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(kind: .endpoint, path: "http://127.0.0.1:\(port)", repo: model),
        runtime: RuntimeRef(id: "generic:openai-server", resolved: .user, tier: .native),
        execution: .stream,
        state: .ready)
}

@Test func streamsChatThroughSSEFixtureServer() async throws {
    let server = try startSSEServer()
    defer { server.process.terminate() }
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore())
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hi")])
        ])
    ])

    var text = ""
    var stats: GenerationStats?
    let record = endpointRecord(port: server.port)
    for try await chunk in adapter.invoke(record, .chat, payload: payload) {
        switch chunk {
        case .text(let delta): text += delta
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(text == "Hello from fake-chat-1")
    #expect(stats?.promptTokens == 7)
    #expect(stats?.completionTokens == 4)
    #expect(stats?.durationMs != nil)
}

@Test func sendsBearerHeaderWhenKeyStored() async throws {
    let server = try startSSEServer()
    defer { server.process.terminate() }
    let secrets = InMemorySecretStore()
    try secrets.set("sk-test", account: "127.0.0.1:\(server.port)")

    let models = try await OpenAIEndpointAdapter.listModels(
        baseURL: "http://127.0.0.1:\(server.port)", key: "sk-test")
    #expect(models.contains("auth-ok"))

    let anonymous = try await OpenAIEndpointAdapter.listModels(
        baseURL: "http://127.0.0.1:\(server.port)", key: nil)
    #expect(anonymous.contains("anon"))
}

@Test func http401MapsToRuntimeUnavailableHint() async throws {
    let server = try startSSEServer(mode: "locked")
    defer { server.process.terminate() }
    do {
        _ = try await OpenAIEndpointAdapter.listModels(
            baseURL: "http://127.0.0.1:\(server.port)", key: nil)
        Issue.record("expected the 401 to surface")
    } catch {
        #expect(String(describing: error).contains("refused the API key"))
    }
}

@Test func connectionRefusedMapsToRuntimeUnavailable() async throws {
    do {
        _ = try await OpenAIEndpointAdapter.listModels(
            baseURL: "http://127.0.0.1:9", key: nil)
        Issue.record("expected the connection failure to surface")
    } catch {
        #expect(String(describing: error).contains("reachable"))
    }
}

@Test func normalizedBaseHandlesSchemeSlashesAndV1() {
    #expect(
        OpenAIEndpointAdapter.normalizedBase(" 127.0.0.1:8080/v1/ ")
            == "http://127.0.0.1:8080")
    #expect(
        OpenAIEndpointAdapter.normalizedBase("https://server.local/")
            == "https://server.local")
    #expect(
        OpenAIEndpointAdapter.normalizedBase("http://x:11434/V1")
            == "http://x:11434")
    #expect(
        OpenAIEndpointAdapter.account(for: "http://127.0.0.1:11434")
            == "http://127.0.0.1:11434")
    #expect(
        OpenAIEndpointAdapter.account(for: "http://127.0.0.1:11434/v1/")
            == "http://127.0.0.1:11434")
    #expect(
        OpenAIEndpointAdapter.account(for: "http://host:8080")
            != OpenAIEndpointAdapter.account(for: "https://host:8080"))
}

@Test func unreachableEndpointDemotesRecordAndSuccessHeals() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)

    var record = endpointRecord(port: 9)
    try await registry.register(record)

    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])

    var threw = false
    do {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
    } catch let KernelError.runtimeUnavailable(hint) {
        threw = true
        #expect(!hint.contains("127.0.0.1"))
        #expect(!hint.contains("http"))
    } catch {
        threw = true
    }
    #expect(threw)
    let demoted = try #require(try await registry.get(id: record.id))
    #expect(demoted.state == .missing)

    let server = try startSSEServer()
    defer { server.process.terminate() }
    record.source = ModelSource(
        kind: .endpoint, path: "http://127.0.0.1:\(server.port)", repo: record.source.repo)

    var text = ""
    for try await chunk in adapter.invoke(record, .chat, payload: payload) {
        if case .text(let delta) = chunk { text += delta }
    }
    #expect(text == "Hello from fake-chat-1")
    let healed = try #require(try await registry.get(id: record.id))
    #expect(healed.state == .ready)
}

@Test func http400DoesNotDemoteRecord() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)
    let server = try startSSEServer(mode: "badrequest")
    defer { server.process.terminate() }
    let record = endpointRecord(port: server.port)
    try await registry.register(record)

    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    do {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
        Issue.record("a 400 must surface as an error")
    } catch let KernelError.runtimeUnavailable(hint) {
        #expect(hint.contains("HTTP 400"))
    }
    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .ready)
}

@Test func http401StillDemotes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)
    let server = try startSSEServer(mode: "locked")
    defer { server.process.terminate() }
    let record = endpointRecord(port: server.port)
    try await registry.register(record)

    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    do {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
        Issue.record("a 401 must surface as an error")
    } catch {
        #expect(String(describing: error).contains("refused the API key"))
    }
    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .missing)
}

@Test func cancelledInvokeDoesNotDemoteHealthyEndpoint() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let server = try startSSEServer(mode: "slow")
    defer { server.process.terminate() }
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)

    let record = endpointRecord(port: server.port)
    try await registry.register(record)

    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])

    let consumer = Task {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
    }
    try await Task.sleep(for: .milliseconds(150))
    consumer.cancel()
    _ = await consumer.result

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .ready)
}

@Test func malformedPayloadDoesNotDemoteHealthyEndpoint() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let adapter = OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)

    let record = endpointRecord(port: 9)
    try await registry.register(record)

    let badPayload: JSONValue = .object([:])
    var threw = false
    do {
        for try await _ in adapter.invoke(record, .chat, payload: badPayload) {}
    } catch {
        threw = true
        #expect(String(describing: error).contains("messages"))
    }
    #expect(threw)

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .ready)
}

@Test func oversizedLineErrorDoesNotDemoteHealthyEndpoint() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let server = try startSSEServer(mode: "huge")
    defer { server.process.terminate() }
    let adapter = OpenAIEndpointAdapter(
        secrets: InMemorySecretStore(), registry: registry,
        concurrencyGate: OpenAIEndpointConcurrencyGate(), maxResponseBytes: 8 * 1024 * 1024,
        maxLineBytes: 1024 * 1024)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    let record = endpointRecord(port: server.port)
    try await registry.register(record)

    var threw = false
    do {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
    } catch {
        threw = true
    }
    #expect(threw)

    let after = try #require(try await registry.get(id: record.id))
    #expect(after.state == .ready)
}

@Test func oversizedLineIsRejectedInsteadOfExhaustingMemory() async throws {
    let server = try startSSEServer(mode: "huge")
    defer { server.process.terminate() }
    let adapter = OpenAIEndpointAdapter(
        secrets: InMemorySecretStore(), registry: nil,
        concurrencyGate: OpenAIEndpointConcurrencyGate(), maxResponseBytes: 8 * 1024 * 1024,
        maxLineBytes: 1024 * 1024)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    let record = endpointRecord(port: server.port)

    var threw = false
    do {
        for try await _ in adapter.invoke(record, .chat, payload: payload) {}
    } catch {
        threw = true
        #expect(String(describing: error).contains("larger than"))
    }
    #expect(threw)
}

@Test func perServerConcurrencyIsBoundedAndRecoversAfterRelease() async throws {
    let server = try startSSEServer()
    defer { server.process.terminate() }
    let gate = OpenAIEndpointConcurrencyGate(limit: 1)
    let record = endpointRecord(port: server.port)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    let adapter = OpenAIEndpointAdapter(
        secrets: InMemorySecretStore(), registry: nil, concurrencyGate: gate)

    var firstText = ""
    for try await chunk in adapter.invoke(record, .chat, payload: payload) {
        if case .text(let delta) = chunk { firstText += delta }
    }
    #expect(firstText == "Hello from fake-chat-1")

    var secondText = ""
    for try await chunk in adapter.invoke(record, .chat, payload: payload) {
        if case .text(let delta) = chunk { secondText += delta }
    }
    #expect(secondText == "Hello from fake-chat-1")
    #expect(gate.acquire(OpenAIEndpointAdapter.normalizedBase(record.source.path)))
}

@Test func removingOneSchemeKeepsSiblingSchemeKey() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = InMemorySecretStore()
    let kernel = Kernel(directory: dir, secrets: secrets)
    try secrets.set("plain-key", account: "http://host:8080")
    try secrets.set("tls-key", account: "https://host:8080")

    let plain = try await kernel.registerEndpoint(baseURL: "http://host:8080", model: "a")
    _ = try await kernel.registerEndpoint(baseURL: "https://host:8080", model: "b")

    try await kernel.removeEndpoint(plain.id)
    #expect(try secrets.get(account: "http://host:8080") == nil)
    #expect(try secrets.get(account: "https://host:8080") == "tls-key")
}

@Test func endpointRequestBodyPassesToolsThroughWithStringArguments() throws {
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("time?")]),
            .object([
                "role": .string("assistant"), "content": .string(""),
                "tool_calls": .array([
                    .object([
                        "id": .string("call-1"), "name": .string("get_time"),
                        "arguments": .object(["zone": .string("UTC")]),
                    ])
                ]),
            ]),
            .object([
                "role": .string("tool"), "content": .string("12:00"),
                "tool_call_id": .string("call-1"), "tool_name": .string("get_time"),
            ]),
        ]),
        "tools": .array([
            .object([
                "name": .string("get_time"), "description": .string("clock"),
                "parameters": .object(["type": .string("object")]),
            ])
        ]),
        "tool_choice": .string("auto"),
    ])
    let data = try OpenAIEndpointAdapter.requestBody(model: "served", payload: payload)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]

    let tools = object["tools"] as! [[String: Any]]
    #expect(tools[0]["type"] as? String == "function")
    #expect(object["tool_choice"] as? String == "auto")

    let messages = object["messages"] as! [[String: Any]]
    let calls = messages[1]["tool_calls"] as! [[String: Any]]
    #expect(calls[0]["id"] as? String == "call-1")
    let function = calls[0]["function"] as! [String: Any]
    #expect(function["name"] as? String == "get_time")
    let argumentsString = function["arguments"] as! String
    let parsed =
        try JSONSerialization.jsonObject(with: Data(argumentsString.utf8)) as! [String: Any]
    #expect(parsed["zone"] as? String == "UTC")
    #expect(messages[1]["content"] is NSNull)
    #expect(messages[2]["name"] as? String == "get_time")
    #expect(messages[2]["tool_name"] == nil)
}
