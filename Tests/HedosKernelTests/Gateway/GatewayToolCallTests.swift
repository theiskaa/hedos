import Foundation
import Testing

@testable import HedosKernel

private func toolModel() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/tool-chat.gguf")
    record.name = "tool-chat:latest"
    record.state = .ready
    return record
}

private func toolStack(
    script: [CapabilityChunk], toolCapable: Bool = true
) async throws -> (stack: GatewayStack, port: FakeGatewayPort) {
    let model = toolModel()
    var port = FakeGatewayPort(records: [model], chatScript: script)
    if toolCapable { port.toolCapableModels = [model.id] }
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    return (stack, port)
}

private func sseEvents(_ raw: String) -> [String] {
    raw.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }
        .map { String($0.dropFirst(6)) }
}

private func toolsBody() -> [String: Any] {
    [
    "type": "function",
    "function": [
        "name": "get_time",
        "description": "Reads the clock",
        "parameters": [
            "type": "object",
            "properties": ["zone": ["type": "string"]],
        ],
    ]
    ]
}

@Test func openAIStreamingToolCallArrivesAsDeltaWithToolCallsFinish() async throws {
    let call = ToolCall(
        id: "call-7", name: "get_time", arguments: .object(["zone": .string("UTC")]))
    let (stack, port) = try await toolStack(script: [
        .toolCall(call),
        .done(GenerationStats(promptTokens: 4, completionTokens: 9)),
    ])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "what time is it"]],
        "stream": true,
        "tools": [toolsBody()],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)

    let events = sseEvents(String(data: data, encoding: .utf8)!)
    #expect(events.last == "[DONE]")
    var sawCall = false
    var finish: String?
    for event in events.dropLast() {
        let object = try JSONSerialization.jsonObject(with: Data(event.utf8)) as! [String: Any]
        let choice = (object["choices"] as! [[String: Any]])[0]
        let delta = choice["delta"] as! [String: Any]
        if let calls = delta["tool_calls"] as? [[String: Any]] {
            sawCall = true
            #expect(calls[0]["id"] as? String == "call-7")
            let function = calls[0]["function"] as! [String: Any]
            #expect(function["name"] as? String == "get_time")
            let arguments = function["arguments"] as! String
            let parsed =
                try JSONSerialization.jsonObject(with: Data(arguments.utf8)) as! [String: Any]
            #expect(parsed["zone"] as? String == "UTC")
        }
        if let reason = choice["finish_reason"] as? String { finish = reason }
    }
    #expect(sawCall)
    #expect(finish == "tool_calls")

    if case .object(let payload) = port.recorder.last!.payload {
        if case .array(let tools)? = payload["tools"] {
            #expect(tools.count == 1)
        } else {
            Issue.record("tools should reach the kernel payload")
        }
    }
    await stack.stop()
}

@Test func openAINonStreamingToolCallReturnsMessageToolCalls() async throws {
    let call = ToolCall(
        id: "call-9", name: "get_time", arguments: .object([:]))
    let (stack, _) = try await toolStack(script: [.toolCall(call), .done(nil)])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "time?"]],
        "stream": false,
        "tools": [toolsBody()],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let choice = (object["choices"] as! [[String: Any]])[0]
    #expect(choice["finish_reason"] as? String == "tool_calls")
    let message = choice["message"] as! [String: Any]
    #expect(message["content"] is NSNull)
    let calls = message["tool_calls"] as! [[String: Any]]
    #expect(calls[0]["id"] as? String == "call-9")
    #expect((calls[0]["function"] as! [String: Any])["arguments"] as? String == "{}")
    await stack.stop()
}

@Test func openAIToolRoleAndAssistantNullContentDecodeAs200() async throws {
    let (stack, port) = try await toolStack(script: [.text("noon"), .done(nil)])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [
            ["role": "user", "content": "what time is it"],
            [
                "role": "assistant",
                "content": NSNull(),
                "tool_calls": [
                    [
                        "id": "call-1",
                        "type": "function",
                        "function": ["name": "get_time", "arguments": "{\"zone\":\"UTC\"}"],
                    ]
                ],
            ],
            ["role": "tool", "tool_call_id": "call-1", "content": "12:00"],
        ],
        "stream": false,
        "tools": [toolsBody()],
    ])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)

    if case .object(let payload) = port.recorder.last!.payload,
        case .array(let messages)? = payload["messages"]
    {
        #expect(messages.count == 3)
        let assistant = messages[1].objectValue!
        #expect(assistant["tool_calls"] != nil)
        let tool = messages[2].objectValue!
        #expect(tool["role"]?.stringValue == "tool")
        #expect(tool["tool_call_id"]?.stringValue == "call-1")
    } else {
        Issue.record("payload should carry the extended messages")
    }
    await stack.stop()
}

@Test func openAIUnparseableToolCallArgumentsAnswer400() async throws {
    let (stack, _) = try await toolStack(script: [.text("x"), .done(nil)])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [
            [
                "role": "assistant",
                "content": NSNull(),
                "tool_calls": [
                    [
                        "id": "call-1",
                        "type": "function",
                        "function": ["name": "get_time", "arguments": "not json"],
                    ]
                ],
            ]
        ],
        "stream": false,
    ])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}

@Test func toolsAgainstANonToolRuntimeAnswer400OnBothSurfaces() async throws {
    let (stack, _) = try await toolStack(script: [.text("x"), .done(nil)], toolCapable: false)
    let openAIBody = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "hi"]],
        "tools": [toolsBody()],
    ])
    let (openAIData, openAIResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: openAIBody))
    #expect((openAIResponse as! HTTPURLResponse).statusCode == 400)
    #expect(String(decoding: openAIData, as: UTF8.self).contains("tool"))

    let ollamaBody = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "hi"]],
        "tools": [toolsBody()],
        "stream": false,
    ])
    let (_, ollamaResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: ollamaBody))
    #expect((ollamaResponse as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}

@Test func ollamaToolCallsRoundTripWithObjectArgumentsAndHonestDoneReason() async throws {
    let call = ToolCall(
        id: "call-3", name: "get_time", arguments: .object(["zone": .string("UTC")]))
    let (stack, port) = try await toolStack(script: [.toolCall(call), .done(nil)])

    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [
            ["role": "user", "content": "time?"],
            [
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    ["function": ["name": "get_time", "arguments": ["zone": "UTC"]]]
                ],
            ],
            ["role": "tool", "tool_name": "get_time", "content": "12:00"],
        ],
        "tools": [toolsBody()],
        "stream": false,
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["done_reason"] as? String == "stop")
    let message = object["message"] as! [String: Any]
    let calls = message["tool_calls"] as! [[String: Any]]
    let function = calls[0]["function"] as! [String: Any]
    #expect(function["name"] as? String == "get_time")
    #expect((function["arguments"] as! [String: Any])["zone"] as? String == "UTC")

    if case .object(let payload) = port.recorder.last!.payload {
        #expect(payload["tools"] != nil)
    }
    await stack.stop()
}

@Test func ollamaStreamingToolCallEmitsDeltaLineAndToolCallsDoneReason() async throws {
    let call = ToolCall(name: "get_time", arguments: .object([:]))
    let (stack, _) = try await toolStack(script: [.toolCall(call), .done(nil)])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "time?"]],
        "tools": [toolsBody()],
        "stream": true,
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/chat"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let lines = String(decoding: data, as: UTF8.self)
        .split(separator: "\n").map(String.init)
    var sawCall = false
    var doneReason: String?
    for line in lines {
        let object = try JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
        if let message = object["message"] as? [String: Any],
            message["tool_calls"] != nil
        {
            sawCall = true
        }
        if object["done"] as? Bool == true {
            doneReason = object["done_reason"] as? String
        }
    }
    #expect(sawCall)
    #expect(doneReason == "stop")
    await stack.stop()
}

@Test func maxTokensTruncationReportsLengthFinishReason() async throws {
    let (stack, _) = try await toolStack(script: [
        .text("cut"),
        .done(GenerationStats(finishReason: "length")),
    ])
    let body = GatewayHarness.json([
        "model": "tool-chat:latest",
        "messages": [["role": "user", "content": "long story"]],
        "stream": true,
    ])
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let events = sseEvents(String(data: data, encoding: .utf8)!)
    var finish: String?
    for event in events.dropLast() {
        let object = try JSONSerialization.jsonObject(with: Data(event.utf8)) as! [String: Any]
        let choice = (object["choices"] as! [[String: Any]])[0]
        if let reason = choice["finish_reason"] as? String { finish = reason }
    }
    #expect(finish == "length")
    await stack.stop()
}
