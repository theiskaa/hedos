import Foundation
import Testing

@testable import HedosKernel

private func chatModel(name: String = "test-chat:latest") -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(name).gguf")
    record.name = name
    record.state = .ready
    return record
}

private func chatStack(
    script: [CapabilityChunk], records: [ModelRecord]? = nil
) async throws -> (stack: GatewayStack, port: FakeGatewayPort) {
    let model = chatModel()
    let port = FakeGatewayPort(records: records ?? [model], chatScript: script)
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    return (stack, port)
}

private func sseEvents(_ raw: String) -> [String] {
    raw.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }
        .map { String($0.dropFirst(6)) }
}

@Test func streamingChatEmitsDeltasUsageAndDone() async throws {
    let (stack, port) = try await chatStack(script: [
        .text("Hel"),
        .text("lo"),
        .thinking("pondering"),
        .done(GenerationStats(promptTokens: 12, completionTokens: 40, durationMs: 900)),
    ])
    let body = GatewayHarness.json([
        "model": "test-chat:latest",
        "messages": [["role": "user", "content": "say hello"]],
        "stream": true,
        "temperature": 0.2,
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    #expect(http.value(forHTTPHeaderField: "Content-Type")?.contains("text/event-stream") == true)

    let events = sseEvents(String(data: data, encoding: .utf8)!)
    #expect(events.last == "[DONE]")

    var contents: [String] = []
    var reasonings: [String] = []
    var sawStop = false
    var usage: [String: Any]?
    for event in events.dropLast() {
        let object = try JSONSerialization.jsonObject(with: Data(event.utf8)) as! [String: Any]
        #expect(object["object"] as? String == "chat.completion.chunk")
        let choices = object["choices"] as! [[String: Any]]
        let delta = choices[0]["delta"] as! [String: Any]
        if let text = delta["content"] as? String { contents.append(text) }
        if let thought = delta["reasoning_content"] as? String { reasonings.append(thought) }
        if choices[0]["finish_reason"] as? String == "stop" {
            sawStop = true
            usage = object["usage"] as? [String: Any]
        }
    }
    #expect(contents.joined() == "Hello")
    #expect(reasonings == ["pondering"])
    #expect(sawStop)
    #expect(usage?["prompt_tokens"] as? Int == 12)
    #expect(usage?["completion_tokens"] as? Int == 40)
    #expect(usage?["total_tokens"] as? Int == 52)

    let invocation = port.recorder.last
    #expect(invocation?.capability == .chat)
    if case .object(let payload) = invocation!.payload {
        #expect(payload["temperature"] == .double(0.2))
        if case .array(let messages) = payload["messages"]! {
            #expect(messages.count == 1)
        } else {
            Issue.record("messages should be an array")
        }
    } else {
        Issue.record("payload should be an object")
    }
    await stack.stop()
}

@Test func nonStreamingChatAccumulatesContent() async throws {
    let (stack, _) = try await chatStack(script: [
        .text("All "),
        .text("done"),
        .done(GenerationStats(promptTokens: 3, completionTokens: 2, durationMs: 40)),
    ])
    let body = GatewayHarness.json([
        "model": "test-chat",
        "messages": [["role": "user", "content": "finish"]],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["object"] as? String == "chat.completion")
    let choices = object["choices"] as! [[String: Any]]
    let message = choices[0]["message"] as! [String: Any]
    #expect(message["content"] as? String == "All done")
    #expect(choices[0]["finish_reason"] as? String == "stop")
    let usage = object["usage"] as! [String: Any]
    #expect(usage["total_tokens"] as? Int == 5)
    await stack.stop()
}

@Test func unknownModelAnswers404ErrorObject() async throws {
    let (stack, _) = try await chatStack(script: [.text("x"), .done(nil)])
    let body = GatewayHarness.json([
        "model": "no-such-model",
        "messages": [["role": "user", "content": "hi"]],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 404)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect(error?["type"] as? String == "not_found_error")
    await stack.stop()
}

@Test func textPartsFlattenAndImagePartsRefuse() async throws {
    let (stack, port) = try await chatStack(script: [.text("ok"), .done(nil)])
    let flattened = GatewayHarness.json([
        "model": "test-chat",
        "messages": [
            [
                "role": "user",
                "content": [
                    ["type": "text", "text": "part one "],
                    ["type": "text", "text": "part two"],
                ],
            ]
        ],
    ])
    let (_, okResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: flattened))
    #expect((okResponse as! HTTPURLResponse).statusCode == 200)
    if case .object(let payload) = port.recorder.last!.payload,
        case .array(let messages) = payload["messages"]!,
        case .object(let message) = messages[0]
    {
        #expect(message["content"] == .string("part one part two"))
    } else {
        Issue.record("payload should carry the flattened message")
    }

    let vision = GatewayHarness.json([
        "model": "test-chat",
        "messages": [
            [
                "role": "user",
                "content": [
                    ["type": "image_url", "image_url": ["url": "data:image/png;base64,xx"]]
                ],
            ]
        ],
    ])
    let (_, badResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: vision))
    #expect((badResponse as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}

@Test func missingModelOrMessagesAnswer400() async throws {
    let (stack, _) = try await chatStack(script: [.text("x"), .done(nil)])
    let noModel = GatewayHarness.json(["messages": [["role": "user", "content": "hi"]]])
    let (_, first) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: noModel))
    #expect((first as! HTTPURLResponse).statusCode == 400)

    let noMessages = GatewayHarness.json(["model": "test-chat"])
    let (_, second) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: noMessages))
    #expect((second as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}
