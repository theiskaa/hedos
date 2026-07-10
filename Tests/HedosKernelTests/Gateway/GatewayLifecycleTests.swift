import Foundation
import Testing

@testable import HedosKernel

@Test func serverBindsEphemeralPortAndReportsIt() async throws {
    let stack = try await GatewayHarness.stack()
    #expect(stack.port > 0)
    let status = await stack.server.status
    #expect(status.running)
    #expect(status.port == stack.port)
    await stack.stop()
}

@Test func unknownRouteAnswers404WithDialectErrorShapes() async throws {
    let stack = try await GatewayHarness.stack()
    let (openAIData, openAIResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/nothing"), token: stack.token))
    #expect((openAIResponse as! HTTPURLResponse).statusCode == 404)
    let openAIBody = try JSONSerialization.jsonObject(with: openAIData) as! [String: Any]
    let errorObject = openAIBody["error"] as? [String: Any]
    #expect(errorObject?["type"] as? String == "not_found_error")

    let (ollamaData, ollamaResponse) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/nothing"), token: stack.token))
    #expect((ollamaResponse as! HTTPURLResponse).statusCode == 404)
    let ollamaBody = try JSONSerialization.jsonObject(with: ollamaData) as! [String: Any]
    #expect(ollamaBody["error"] is String)
    await stack.stop()
}

@Test func wrongMethodAnswers405() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [GatewayRoute("POST", "/v1/chat/completions", EchoHandler())])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "GET", stack.url("/v1/chat/completions"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 405)
    await stack.stop()
}

@Test func startIsIdempotentWhileRunning() async throws {
    let stack = try await GatewayHarness.stack()
    let second = try await stack.server.start()
    #expect(second == stack.port)
    await stack.stop()
}

@Test func stopFreesThePortForRebinding() async throws {
    let stack = try await GatewayHarness.stack()
    let port = stack.port
    await stack.server.stop()
    let status = await stack.server.status
    #expect(!status.running)
    #expect(status.port == nil)

    do {
        _ = try await URLSession.shared.data(
            for: GatewayHarness.request("GET", stack.url("/v1/models"), token: stack.token))
        Issue.record("connection should be refused after stop")
    } catch {}

    let reborn = try await GatewayHarness.stack(
        configuration: GatewayServer.Configuration(port: port))
    #expect(reborn.port == port)
    await reborn.stop()
    await stack.stop()
}

@Test func gatewaySettingsDefaultOffAndRoundTrip() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)
    let defaults = await store.gateway()
    #expect(defaults.enabled == false)
    #expect(defaults.port == 43367)
    #expect(defaults.maxConnections == 128)
    #expect(defaults.maxConcurrentInference == 4)

    var changed = defaults
    changed.enabled = true
    changed.port = 50505
    try await store.save(changed)
    let reloaded = await SettingsStore(directory: dir).gateway()
    #expect(reloaded.enabled == true)
    #expect(reloaded.port == 50505)
}

@Test func kernelStartGatewayLifecycle() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [], secrets: InMemorySecretStore())

    await kernel.startGatewayIfEnabled()
    #expect(await kernel.gatewayStatus().running == false)

    let status = try await kernel.startGateway(portOverride: 0)
    #expect(status.running)
    let port = status.port!
    let creation = try await kernel.gatewayClientStore.create(name: "kernel-test", scopes: .all)
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "GET", "http://127.0.0.1:\(port)/v1/models", token: creation.token))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["object"] as? String == "list")

    let audited = await kernel.gatewayAuditLog.tail(limit: 5)
    #expect(audited.last?.outcome == "ok")

    await kernel.stopGateway()
    #expect(await kernel.gatewayStatus().running == false)
}

@Test func kernelStartGatewayIfEnabledHonorsSetting() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [], secrets: InMemorySecretStore())
    var settings = await kernel.settings.gateway()
    settings.enabled = true
    settings.port = 0
    try await kernel.settings.save(settings)

    await kernel.startGatewayIfEnabled()
    let status = await kernel.gatewayStatus()
    #expect(status.running)
    #expect(status.port! > 0)
    await kernel.stopGateway()
}

@Test func oversizedBodyAnswers413() async throws {
    let stack = try await GatewayHarness.stack(
        configuration: GatewayServer.Configuration(port: 0, maxBodyBytes: 128))
    let big = Data(repeating: 65, count: 4096)
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: big))
    #expect((response as! HTTPURLResponse).statusCode == 413)
    await stack.stop()
}
