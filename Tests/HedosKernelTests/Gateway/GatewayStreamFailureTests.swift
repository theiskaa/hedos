import Foundation
import Testing

@testable import HedosKernel

private func failingModel() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/failing.gguf")
    record.name = "failing:latest"
    record.state = .ready
    record.runtime.id = .llamaCpp
    return record
}

@Test func openAIStreamFailureEmitsErrorFrameThenDone() async throws {
    var port = FakeGatewayPort(records: [failingModel()], chatScript: [.text("partial")])
    port.streamFailure = "model crashed"
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json([
        "model": "failing:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": true,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let events = String(data: data, encoding: .utf8)!.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }.map { String($0.dropFirst(6)) }
    #expect(events.last == "[DONE]")
    let errorFrame = events.dropLast().compactMap {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any]
    }.first { $0["error"] != nil }
    let message = (errorFrame?["error"] as? [String: Any])?["message"] as? String
    #expect(message == "the runtime failed to complete the request")
    await stack.stop()
}

@Test func ollamaStreamFailureEmitsErrorLine() async throws {
    var port = FakeGatewayPort(records: [failingModel()], chatScript: [.text("partial")])
    port.streamFailure = "model crashed"
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json([
        "model": "failing:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": true,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    let lines = String(data: data, encoding: .utf8)!.split(separator: "\n").map {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any] ?? [:]
    }
    let errorLine = lines.first { $0["error"] != nil }
    #expect(errorLine?["error"] as? String == "the runtime failed to complete the request")
    await stack.stop()
}

@Test func overLimitConnectionReceives503WithRetryAfter() async throws {
    var port = FakeGatewayPort(records: [failingModel()])
    port.streamHangs = true
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes(),
        configuration: GatewayServer.Configuration(port: 0, maxConnections: 1))

    let occupier = URLSession(configuration: .ephemeral)
    let holdBody = GatewayHarness.json([
        "model": "failing:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": true,
    ])
    async let held: (Data, URLResponse) = occupier.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: holdBody))

    var status = 0
    for _ in 0..<40 {
        try? await Task.sleep(for: .milliseconds(50))
        let probe = URLSession(configuration: .ephemeral)
        if let (_, response) = try? await probe.data(
            for: GatewayHarness.request("GET", stack.url("/api/version"), token: stack.token)),
            let http = response as? HTTPURLResponse
        {
            status = http.statusCode
            if status == 503 {
                #expect(http.value(forHTTPHeaderField: "Retry-After") == "1")
                break
            }
        }
        probe.invalidateAndCancel()
    }
    #expect(status == 503)
    occupier.invalidateAndCancel()
    _ = try? await held
    await stack.stop()
}

@Test func hungOpenAIChatStreamTimesOutAndServerStaysResponsive() async throws {
    var port = FakeGatewayPort(records: [failingModel()])
    port.streamHangs = true
    let routes = [
        GatewayRoute(
            "POST", "/v1/chat/completions", OpenAIChatHandler(runTimeoutSeconds: 1),
            inference: true, group: "OpenAI", summary: "chat"),
        GatewayRoute("GET", "/api/version", OllamaVersionHandler(), group: "Ollama", summary: "v"),
    ]
    let stack = try await GatewayHarness.stack(port: port, routes: routes)
    let body = GatewayHarness.json([
        "model": "failing:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": true,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let events = String(data: data, encoding: .utf8)!.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }.map { String($0.dropFirst(6)) }
    #expect(events.last == "[DONE]")
    let errorFrame = events.dropLast().compactMap {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any]
    }.first { $0["error"] != nil }
    let error = errorFrame?["error"] as? [String: Any]
    #expect(error?["message"] as? String == "the request timed out after 1s")
    #expect(error?["code"] as? String == "timeout")

    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/version"), token: stack.token))
    #expect((response as? HTTPURLResponse)?.statusCode == 200)
    await stack.stop()
}

@Test func hungOllamaChatStreamTimesOutWithErrorLine() async throws {
    var port = FakeGatewayPort(records: [failingModel()])
    port.streamHangs = true
    let routes = [
        GatewayRoute(
            "POST", "/api/chat", OllamaChatHandler(runTimeoutSeconds: 1),
            inference: true, group: "Ollama", summary: "chat")
    ]
    let stack = try await GatewayHarness.stack(port: port, routes: routes)
    let body = GatewayHarness.json([
        "model": "failing:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": true,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    let lines = String(data: data, encoding: .utf8)!.split(separator: "\n").map {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any] ?? [:]
    }
    let errorLine = lines.first { $0["error"] != nil }
    #expect(errorLine?["error"] as? String == "the request timed out after 1s")
    await stack.stop()
}

@Test func streamTimeoutRaceDetectsHangAndFastPath() async throws {
    let hung = try await StreamTimeout.race(seconds: 0) {
        try await Task.sleep(for: .seconds(30))
    }
    #expect(hung)

    let fast = try await StreamTimeout.race(seconds: 30) {}
    #expect(!fast)
}
