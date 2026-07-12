import Foundation
import Testing

@testable import HedosKernel

private func fidelityModel() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/fidelity.gguf")
    record.name = "fidelity:latest"
    record.state = .ready
    record.runtime.id = .llamaCpp
    return record
}

private func fidelityStack() async throws -> (GatewayStack, FakeGatewayPort) {
    let port = FakeGatewayPort(
        records: [fidelityModel()],
        chatScript: [.text("ok"), .done(GenerationStats(promptTokens: 1, completionTokens: 1))])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    return (stack, port)
}

private func post(_ stack: GatewayStack, _ path: String, _ body: [String: Any]) async throws
    -> (Int, [String: Any])
{
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url(path), token: stack.token, body: GatewayHarness.json(body)))
    let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    return ((response as! HTTPURLResponse).statusCode, object)
}

@Test func openAIStopSeedPenaltiesReachSampling() async throws {
    let (stack, port) = try await fidelityStack()
    let (status, _) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "stop": ["\n\n", "END"],
            "seed": 7,
            "frequency_penalty": 0.5,
            "presence_penalty": 0.25,
        ])
    #expect(status == 200)
    guard case .object(let payload)? = port.recorder.last?.payload else {
        Issue.record("no payload recorded")
        await stack.stop()
        return
    }
    #expect(payload["stop"] == .array([.string("\n\n"), .string("END")]))
    #expect(payload["seed"] == .int(7))
    #expect(payload["frequency_penalty"] == .double(0.5))
    #expect(payload["presence_penalty"] == .double(0.25))
    await stack.stop()
}

@Test func openAIDocumentedParamSweepHasNoSilentDrop() async throws {
    let (stack, _) = try await fidelityStack()
    let rejected = [
        "logit_bias", "logprobs", "top_logprobs", "metadata", "store",
        "parallel_tool_calls", "modalities", "audio", "prediction", "reasoning_effort",
        "service_tier",
    ]
    for key in rejected {
        let (status, object) = try await post(
            stack, "/v1/chat/completions",
            [
                "model": "fidelity:latest",
                "messages": [["role": "user", "content": "hi"]],
                key: key == "logit_bias" ? ["100": -1] as [String: Any] : 1,
            ])
        #expect(status == 400)
        let message = (object["error"] as? [String: Any])?["message"] as? String ?? ""
        #expect(message.contains(key), "sweep param \(key) should be named in the 400")
    }
    let (emptyBias, _) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "logit_bias": [String: Any](),
        ])
    #expect(emptyBias == 200)
    await stack.stop()
}

@Test func openAINGreaterThanOneAnswers400ButOneIsAccepted() async throws {
    let (stack, _) = try await fidelityStack()
    let (rejected, object) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest", "messages": [["role": "user", "content": "hi"]], "n": 2,
        ])
    #expect(rejected == 400)
    #expect(((object["error"] as? [String: Any])?["message"] as? String ?? "").contains("n "))
    let (accepted, _) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest", "messages": [["role": "user", "content": "hi"]], "n": 1,
        ])
    #expect(accepted == 200)
    await stack.stop()
}

@Test func openAIStopOverFourAnswers400() async throws {
    let (stack, _) = try await fidelityStack()
    let (status, _) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "stop": ["a", "b", "c", "d", "e"],
        ])
    #expect(status == 400)
    await stack.stop()
}

@Test func ollamaExtendedOptionsAndStopDecode() async throws {
    let port = FakeGatewayPort(
        records: [fidelityModel()],
        chatScript: [.text("ok"), .done(GenerationStats())])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let (status, _) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "stream": false,
            "options": [
                "top_k": 40, "seed": 3, "repeat_penalty": 1.1, "stop": ["\n\n"],
            ] as [String: Any],
        ])
    #expect(status == 200)
    guard case .object(let payload)? = port.recorder.last?.payload else {
        Issue.record("no payload recorded")
        await stack.stop()
        return
    }
    #expect(payload["top_k"] == .int(40))
    #expect(payload["seed"] == .int(3))
    #expect(payload["repeat_penalty"] == .double(1.1))
    #expect(payload["stop"] == .array([.string("\n\n")]))
    await stack.stop()
}

@Test func ollamaUnknownOptionAndTopLevelKeyAnswer400() async throws {
    let (stack, _) = try await fidelityStack()
    let (badOption, optionObject) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "options": ["mirostat": 2] as [String: Any],
        ])
    #expect(badOption == 400)
    #expect(
        ((optionObject["error"] as? String) ?? "").contains("mirostat"))
    let (badTop, _) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "raw": true,
        ])
    #expect(badTop == 400)
    await stack.stop()
}

@Test func openAIResponseFormatReachesHonoringRuntimeButRejectsOnOthers() async throws {
    let (stack, port) = try await fidelityStack()
    let (accepted, _) = try await post(
        stack, "/v1/chat/completions",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "response_format": ["type": "json_object"],
        ])
    #expect(accepted == 200)
    if case .object(let payload)? = port.recorder.last?.payload {
        #expect(payload["response_format"] != nil)
    } else {
        Issue.record("no payload recorded")
    }
    await stack.stop()

    var mlxModel = fidelityModel()
    mlxModel.runtime.id = .mlxSwift
    var narrowPort = FakeGatewayPort(records: [mlxModel], chatScript: [.text("x")])
    narrowPort.honoredKeys = ["temperature", "top_p", "max_tokens"]
    let mlxStack = try await GatewayHarness.stack(
        port: narrowPort, routes: GatewayRouter.standardRoutes())
    let (rejected, object) = try await post(
        mlxStack, "/v1/chat/completions",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "response_format": ["type": "json_object"],
        ])
    #expect(rejected == 400)
    let message = (object["error"] as? [String: Any])?["message"] as? String ?? ""
    #expect(message.contains("response_format"))
    #expect(message.contains("mlx-swift"))
    await mlxStack.stop()
}

@Test func ollamaKeepAliveAcceptedAndOptionWrongTypeRejected() async throws {
    let port = FakeGatewayPort(
        records: [fidelityModel()], chatScript: [.text("ok"), .done(GenerationStats())])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let (kept, _) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "stream": false,
            "keep_alive": "5m",
        ])
    #expect(kept == 200)
    let (wrongType, _) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi"]],
            "options": ["temperature": "hot"] as [String: Any],
        ])
    #expect(wrongType == 400)
    await stack.stop()
}

@Test func ollamaImagesInMessageAreDecodedNotRejected() async throws {
    let (stack, _) = try await fidelityStack()
    let encoded = Data([0x89, 0x50, 0x4E, 0x47]).base64EncodedString()
    let (status, _) = try await post(
        stack, "/api/chat",
        [
            "model": "fidelity:latest",
            "messages": [["role": "user", "content": "hi", "images": [encoded]]],
        ])
    #expect(status == 200)
    await stack.stop()
}
