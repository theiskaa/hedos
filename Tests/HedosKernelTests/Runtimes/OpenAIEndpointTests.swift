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
