import Foundation
import Testing

@testable import HedosKernel

private func readyModel(_ suffix: String, name: String, capabilities: [Capability]) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(suffix).gguf")
    record.name = name
    record.capabilities = capabilities
    record.state = .ready
    return record
}

private func pipeline(
    _ name: String, _ stages: [(ModelRecord, Capability)]
) -> Pipeline {
    Pipeline(
        name: name,
        stages: stages.map { PipelineStage(modelID: $0.0.id, capability: $0.1) })
}

@Test func pipelineListReturnsScopePermittedOnly() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let tts = readyModel("tts", name: "kokoro", capabilities: [.speak])
    let allowed = pipeline("allowed", [(chat, .chat)])
    let hidden = pipeline("hidden", [(chat, .chat), (tts, .speak)])
    var port = FakeGatewayPort(records: [chat, tts])
    port.pipelinesList = [allowed, hidden]

    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes(),
        scopes: GatewayScopes(models: [chat.id], capabilities: ["chat"]))
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/pipelines"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let entries = object["pipelines"] as! [[String: Any]]
    #expect(entries.count == 1)
    #expect(entries[0]["name"] as? String == "allowed")
    #expect(entries[0]["input"] as? String == "text")
    #expect(entries[0]["output"] as? String == "text")
    await stack.stop()
}

@Test func pipelineRunStreamsTextTailAsSSE() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let pipe = pipeline("answerer", [(chat, .chat)])
    var port = FakeGatewayPort(records: [chat])
    port.pipelinesList = [pipe]
    port.pipelineEventScript = [
        .stageStarted(index: 0, capability: .chat),
        .delta(index: 0, "Hello "),
        .delta(index: 0, "world"),
        .completed,
    ]
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "hi"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    #expect(http.value(forHTTPHeaderField: "Content-Type")?.contains("text/event-stream") == true)
    let text = String(data: data, encoding: .utf8)!
    let events = text.split(separator: "\n").filter { $0.hasPrefix("data: ") }.map {
        String($0.dropFirst(6))
    }
    #expect(events.last == "[DONE]")
    var content = ""
    for event in events.dropLast() {
        let object = try JSONSerialization.jsonObject(with: Data(event.utf8)) as! [String: Any]
        let choices = object["choices"] as! [[String: Any]]
        if let delta = choices.first?["delta"] as? [String: Any],
            let piece = delta["content"] as? String
        {
            content += piece
        }
    }
    #expect(content == "Hello world")
    await stack.stop()
}

@Test func pipelineRunReturnsWavForAudioTail() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let tts = readyModel("tts", name: "kokoro", capabilities: [.speak])
    let pipe = pipeline("voice", [(chat, .chat), (tts, .speak)])
    var port = FakeGatewayPort(records: [chat, tts])
    port.pipelinesList = [pipe]
    let pcm = Data((0..<400).flatMap { _ in [UInt8](repeating: 0, count: 4) })
    port.pipelineEventScript = [
        .audio(AudioFrame(data: pcm, sampleRate: 24000)),
        .completed,
    ]
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "say hi"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    #expect(http.value(forHTTPHeaderField: "Content-Type")?.contains("audio/wav") == true)
    #expect(String(data: data.prefix(4), encoding: .ascii) == "RIFF")
    await stack.stop()
}

@Test func pipelineRunReturnsB64ForImageTail() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    var flux = Fixtures.flux()
    flux.state = .ready
    let pipe = pipeline("illustrator", [(chat, .chat), (flux, .image)])
    var port = FakeGatewayPort(records: [chat, flux])
    port.pipelinesList = [pipe]
    let png = Data([0x89, 0x50, 0x4E, 0x47])
    port.artifacts = ["art-1": png]
    port.pipelineEventScript = [.artifact(id: "art-1"), .completed]
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "draw"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let images = object["data"] as! [[String: Any]]
    #expect(Data(base64Encoded: images[0]["b64_json"] as! String) == png)
    await stack.stop()
}

@Test func pipelineRunRefuses403WhenStageOutOfScope() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let tts = readyModel("tts", name: "kokoro", capabilities: [.speak])
    let pipe = pipeline("voice", [(chat, .chat), (tts, .speak)])
    var port = FakeGatewayPort(records: [chat, tts])
    port.pipelinesList = [pipe]
    port.pipelineEventScript = [.completed]
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes(),
        scopes: GatewayScopes(models: nil, capabilities: ["chat"]))
    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "hi"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 403)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect(error?["type"] as? String == "permission_error")

    let entries = await stack.audit.tail(limit: 5)
    #expect(entries.last?.outcome == "forbidden")
    await stack.stop()
}

@Test func pipelineRunReturns404ForUnknownPipeline() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    var port = FakeGatewayPort(records: [chat])
    port.pipelinesList = []
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["pipeline": "ghost", "input": ["text": "hi"]])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 404)
    await stack.stop()
}

@Test func pipelineRunTimesOutHangingTextStageAndKeepsServerResponsive() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let pipe = pipeline("answerer", [(chat, .chat)])
    var port = FakeGatewayPort(records: [chat])
    port.pipelinesList = [pipe]
    port.pipelineHangs = true
    var routes = GatewayRouter.standardRoutes()
    let runIndex = try #require(routes.firstIndex { $0.path == "/v1/pipelines/run" })
    routes[runIndex] = GatewayRoute(
        "POST", "/v1/pipelines/run", PipelineRunHandler(runTimeout: .milliseconds(200)),
        inference: true, group: "Pipelines", summary: routes[runIndex].summary)
    let stack = try await GatewayHarness.stack(port: port, routes: routes)

    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "hi"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    let text = String(data: data, encoding: .utf8)!
    let events = text.split(separator: "\n").filter { $0.hasPrefix("data: ") }.map {
        String($0.dropFirst(6))
    }
    #expect(events.last == "[DONE]")
    let errorFrame = try #require(
        events.dropLast().compactMap {
            try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any]
        }.first { $0["error"] != nil })
    let error = errorFrame["error"] as! [String: Any]
    #expect(error["type"] as? String == "timeout_error")

    let (_, listResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/pipelines"), token: stack.token))
    #expect((listResponse as! HTTPURLResponse).statusCode == 200)
    await stack.stop()
}

@Test func pipelineRunTimesOutHangingAudioStageWith504AndKeepsServerResponsive() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let tts = readyModel("tts", name: "kokoro", capabilities: [.speak])
    let pipe = pipeline("voice", [(chat, .chat), (tts, .speak)])
    var port = FakeGatewayPort(records: [chat, tts])
    port.pipelinesList = [pipe]
    port.pipelineHangs = true
    var routes = GatewayRouter.standardRoutes()
    let runIndex = try #require(routes.firstIndex { $0.path == "/v1/pipelines/run" })
    routes[runIndex] = GatewayRoute(
        "POST", "/v1/pipelines/run", PipelineRunHandler(runTimeout: .milliseconds(200)),
        inference: true, group: "Pipelines", summary: routes[runIndex].summary)
    let stack = try await GatewayHarness.stack(port: port, routes: routes)

    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "say hi"]])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 504)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as! [String: Any]
    #expect(error["type"] as? String == "timeout_error")

    let (_, listResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/pipelines"), token: stack.token))
    #expect((listResponse as! HTTPURLResponse).statusCode == 200)
    await stack.stop()
}

@Test func pipelineRunAuditsPipelineID() async throws {
    let chat = readyModel("chat", name: "gemma", capabilities: [.chat])
    let pipe = pipeline("answerer", [(chat, .chat)])
    var port = FakeGatewayPort(records: [chat])
    port.pipelinesList = [pipe]
    port.pipelineEventScript = [.delta(index: 0, "hi"), .completed]
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["pipeline": pipe.id, "input": ["text": "hi"]])
    _ = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/pipelines/run"), token: stack.token, body: body))
    let entries = await stack.audit.tail(limit: 5)
    #expect(entries.last?.model == pipe.id)
    #expect(entries.last?.outcome == "ok")
    await stack.stop()
}
