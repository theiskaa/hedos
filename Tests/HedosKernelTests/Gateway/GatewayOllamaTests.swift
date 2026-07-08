import Foundation
import Testing

@testable import HedosKernel

private func ollamaModel(name: String = "gemma-test:latest") -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(name).gguf")
    record.name = name
    record.state = .ready
    record.footprintMB = 4800
    record.primaryWeightPath = "/tmp/hedos-fixtures/\(name).gguf"
    return record
}

private func ollamaStack(
    script: [CapabilityChunk] = []
) async throws -> (stack: GatewayStack, port: FakeGatewayPort) {
    let port = FakeGatewayPort(records: [ollamaModel()], chatScript: script)
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    return (stack, port)
}

private func ndjsonObjects(_ data: Data) throws -> [[String: Any]] {
    String(data: data, encoding: .utf8)!
        .split(separator: "\n")
        .map { try! JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any] }
}

@Test func ollamaChatStreamsNDJSONWithFinalStats() async throws {
    let (stack, port) = try await ollamaStack(script: [
        .text("Hi "),
        .text("there"),
        .thinking("hmm"),
        .done(GenerationStats(promptTokens: 7, completionTokens: 21, durationMs: 640)),
    ])
    let body = GatewayHarness.json([
        "model": "gemma-test",
        "messages": [["role": "user", "content": "greet me"]],
        "options": ["temperature": 0.5, "num_predict": 128],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    #expect(http.value(forHTTPHeaderField: "Content-Type")?.contains("application/x-ndjson") == true)

    let lines = try ndjsonObjects(data)
    #expect(lines.count == 4)
    for line in lines {
        #expect(line["model"] as? String == "gemma-test")
        #expect(line["created_at"] is String)
    }
    let deltas = lines.filter { ($0["done"] as? Bool) == false }
    let texts = deltas.compactMap { ($0["message"] as? [String: Any])?["content"] as? String }
    #expect(texts.joined() == "Hi there")
    let thinkings = deltas.compactMap { ($0["message"] as? [String: Any])?["thinking"] as? String }
    #expect(thinkings == ["hmm"])

    let final = lines.last!
    #expect(final["done"] as? Bool == true)
    #expect(final["done_reason"] as? String == "stop")
    #expect(final["prompt_eval_count"] as? Int == 7)
    #expect(final["eval_count"] as? Int == 21)
    #expect(final["total_duration"] as? Int == 640_000_000)

    if case .object(let payload) = port.recorder.last!.payload {
        #expect(payload["temperature"] == .double(0.5))
        #expect(payload["max_tokens"] == .int(128))
    } else {
        Issue.record("payload should be an object")
    }
    await stack.stop()
}

@Test func ollamaChatNonStreamingReturnsOneObject() async throws {
    let (stack, _) = try await ollamaStack(script: [
        .text("full "),
        .text("answer"),
        .done(GenerationStats(promptTokens: 2, completionTokens: 3, durationMs: 50)),
    ])
    let body = GatewayHarness.json([
        "model": "gemma-test:latest",
        "messages": [["role": "user", "content": "hi"]],
        "stream": false,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["done"] as? Bool == true)
    let message = object["message"] as! [String: Any]
    #expect(message["content"] as? String == "full answer")
    #expect(object["eval_count"] as? Int == 3)
    await stack.stop()
}

@Test func ollamaStreamParserRoundTripsOwnOutput() async throws {
    let (stack, _) = try await ollamaStack(script: [
        .text("alpha"),
        .thinking("beta"),
        .done(GenerationStats(promptTokens: 1, completionTokens: 2, durationMs: 30)),
    ])
    let body = GatewayHarness.json([
        "model": "gemma-test",
        "messages": [["role": "user", "content": "loop"]],
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    let chunks = String(data: data, encoding: .utf8)!
        .split(separator: "\n")
        .compactMap { OllamaStreamParser.parse(line: String($0)) }
    #expect(chunks.contains(.text("alpha")))
    #expect(chunks.contains(.thinking("beta")))
    if case .done(let stats) = chunks.last {
        #expect(stats?.promptTokens == 1)
        #expect(stats?.completionTokens == 2)
        #expect(stats?.durationMs == 30)
    } else {
        Issue.record("last chunk should be done")
    }
    await stack.stop()
}

@Test func ollamaTagsShapeMatchesStockClients() async throws {
    let (stack, _) = try await ollamaStack()
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/tags"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let models = object["models"] as! [[String: Any]]
    #expect(models.count == 1)
    let entry = models[0]
    #expect(entry["name"] as? String == "gemma-test:latest")
    #expect(entry["model"] as? String == "gemma-test:latest")
    #expect(entry["size"] as? Int == 4800 * 1_048_576)
    #expect(entry["modified_at"] is String)
    #expect(entry["digest"] is String)
    let details = entry["details"] as! [String: Any]
    #expect(details["format"] as? String == "gguf")
    #expect(details["families"] is [Any])
    await stack.stop()
}

@Test func ollamaErrorsUseStringShape() async throws {
    let (stack, _) = try await ollamaStack(script: [.text("x"), .done(nil)])
    let body = GatewayHarness.json([
        "model": "unknown-model",
        "messages": [["role": "user", "content": "hi"]],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 404)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["error"] is String)
    await stack.stop()
}

@Test func ollamaVersionAndShowHandshake() async throws {
    let (stack, _) = try await ollamaStack()
    let (versionData, versionResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/version"), token: stack.token))
    #expect((versionResponse as! HTTPURLResponse).statusCode == 200)
    let version = try JSONSerialization.jsonObject(with: versionData) as! [String: Any]
    #expect(version["version"] is String)

    let (showData, showResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/show"), token: stack.token,
            body: GatewayHarness.json(["model": "gemma-test"])))
    #expect((showResponse as! HTTPURLResponse).statusCode == 200)
    let show = try JSONSerialization.jsonObject(with: showData) as! [String: Any]
    #expect(show["details"] is [String: Any])
    let capabilities = show["capabilities"] as! [String]
    #expect(capabilities.contains("completion"))
    await stack.stop()
}
