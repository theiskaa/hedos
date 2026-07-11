import Foundation
import Testing

@testable import HedosKernel

private struct ScopedHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }
    var modelID: String
    var capability: Capability

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        try identity.require(modelID: modelID, capability: capability)
        try await responder.respond(status: 200, body: Data("{\"ok\":true}".utf8))
        return .ok(model: modelID, capability: capability)
    }
}

@Test func missingTokenAnswers401AndAudits() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [GatewayRoute("GET", "/v1/models", EchoHandler())])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/models")))
    #expect((response as! HTTPURLResponse).statusCode == 401)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = body["error"] as? [String: Any]
    #expect(error?["type"] as? String == "authentication_error")

    let entries = await stack.audit.tail(limit: 10)
    #expect(entries.count == 1)
    #expect(entries[0].outcome == "unauthorized")
    #expect(entries[0].status == 401)
    #expect(entries[0].client == nil)
    #expect(entries[0].route == "/v1/models")
    await stack.stop()
}

@Test func invalidTokenAnswers401OnBothDialects() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [
            GatewayRoute("GET", "/v1/models", EchoHandler()),
            GatewayRoute("GET", "/api/tags", EchoHandler()),
        ])
    let (_, openAIResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "GET", stack.url("/v1/models"), token: "hd_bogus.bogus"))
    #expect((openAIResponse as! HTTPURLResponse).statusCode == 401)

    let (ollamaData, ollamaResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/tags"), token: "hd_bogus.bogus"))
    #expect((ollamaResponse as! HTTPURLResponse).statusCode == 401)
    let body = try JSONSerialization.jsonObject(with: ollamaData) as! [String: Any]
    #expect(body["error"] is String)

    let entries = await stack.audit.tail(limit: 10)
    #expect(entries.count == 1)
    #expect(entries.allSatisfy { $0.outcome == "unauthorized" })
    await stack.stop()
}

@Test func outOfScopeModelAnswers403AndAudits() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [
            GatewayRoute(
                "POST", "/v1/chat/completions",
                ScopedHandler(modelID: "forbidden-model", capability: .chat))
        ],
        scopes: GatewayScopes(models: ["allowed-model"], capabilities: nil))
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token,
            body: GatewayHarness.json(["model": "forbidden-model"])))
    #expect((response as! HTTPURLResponse).statusCode == 403)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = body["error"] as? [String: Any]
    #expect(error?["type"] as? String == "permission_error")

    let entries = await stack.audit.tail(limit: 10)
    #expect(entries.count == 1)
    #expect(entries[0].outcome == "forbidden")
    #expect(entries[0].clientName == "test-client")
    await stack.stop()
}

@Test func outOfScopeCapabilityAnswers403() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [
            GatewayRoute(
                "POST", "/v1/audio/speech",
                ScopedHandler(modelID: "tts-model", capability: .speak))
        ],
        scopes: GatewayScopes(models: nil, capabilities: ["chat"]))
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/audio/speech"), token: stack.token,
            body: GatewayHarness.json(["model": "tts-model"])))
    #expect((response as! HTTPURLResponse).statusCode == 403)
    await stack.stop()
}

@Test func validTokenPassesAndAuditsOk() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [
            GatewayRoute(
                "POST", "/v1/chat/completions",
                ScopedHandler(modelID: "m1", capability: .chat))
        ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token,
            body: GatewayHarness.json(["model": "m1"])))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(body["ok"] as? Bool == true)

    let entries = await stack.audit.tail(limit: 10)
    #expect(entries.count == 1)
    #expect(entries[0].outcome == "ok")
    #expect(entries[0].model == "m1")
    #expect(entries[0].capability == "chat")
    #expect(entries[0].clientName == "test-client")
    await stack.stop()
}

@Test func revokedTokenIsRefusedOnTheNextRequest() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [GatewayRoute("GET", "/v1/models", EchoHandler())])
    let first = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/models"), token: stack.token))
    #expect((first.1 as! HTTPURLResponse).statusCode == 200)

    let clients = await stack.clients.list()
    try await stack.clients.revoke(id: clients[0].id)
    let second = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/models"), token: stack.token))
    #expect((second.1 as! HTTPURLResponse).statusCode == 401)
    await stack.stop()
}
