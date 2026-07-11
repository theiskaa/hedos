import Foundation
import Testing

@testable import HedosKernel

private func metaModel(name: String, capabilities: [Capability]) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(name).gguf")
    record.name = name
    record.state = .ready
    record.runtime.id = .llamaCpp
    record.capabilities = capabilities
    return record
}

@Test func streamingOmitsUsageWhenNotRequested() async throws {
    let port = FakeGatewayPort(
        records: [metaModel(name: "m:latest", capabilities: [.chat, .complete])],
        chatScript: [.text("hi"), .done(GenerationStats(promptTokens: 1, completionTokens: 1))])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token,
            body: GatewayHarness.json([
                "model": "m:latest",
                "messages": [["role": "user", "content": "hi"]],
                "stream": true,
            ])))
    let events = String(data: data, encoding: .utf8)!.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }.map { String($0.dropFirst(6)) }
    let anyUsage = events.dropLast().contains {
        ((try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any])?["usage"]
            != nil
    }
    #expect(!anyUsage)
    await stack.stop()
}

@Test func errorTypesUseOpenAIVocabulary() async throws {
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(records: [metaModel(name: "m:latest", capabilities: [.chat])]),
        routes: GatewayRouter.standardRoutes())
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token,
            body: GatewayHarness.json([
                "model": "ghost", "messages": [["role": "user", "content": "hi"]],
            ])))
    #expect((response as! HTTPURLResponse).statusCode == 404)
    let error = (try JSONSerialization.jsonObject(with: data) as! [String: Any])["error"]
        as! [String: Any]
    #expect(error["type"] as? String == "not_found_error")
    await stack.stop()
}

@Test func modelScopedTokenProbingAnotherModelGets404() async throws {
    let visible = metaModel(name: "allowed:latest", capabilities: [.chat])
    let hidden = metaModel(name: "secret:latest", capabilities: [.chat])
    let port = FakeGatewayPort(records: [visible, hidden], chatScript: [.text("hi")])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes(),
        scopes: GatewayScopes(models: [visible.id]))
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token,
            body: GatewayHarness.json([
                "model": "secret:latest", "messages": [["role": "user", "content": "hi"]],
            ])))
    #expect((response as! HTTPURLResponse).statusCode == 404)
    let error = (try JSONSerialization.jsonObject(with: data) as! [String: Any])["error"]
        as! [String: Any]
    #expect(error["type"] as? String == "not_found_error")
    await stack.stop()
}

@Test func showListsCapabilitiesAndVersionIsBareSemver() async throws {
    let embedder = metaModel(name: "embedder:latest", capabilities: [.embed])
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(records: [embedder]), routes: GatewayRouter.standardRoutes())
    let (showData, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/show"), token: stack.token,
            body: GatewayHarness.json(["model": "embedder:latest"])))
    let show = try JSONSerialization.jsonObject(with: showData) as! [String: Any]
    #expect((show["capabilities"] as? [String])?.contains("embedding") == true)

    let (versionData, _) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/api/version"), token: stack.token))
    let version = (try JSONSerialization.jsonObject(with: versionData) as! [String: Any])["version"]
        as! String
    #expect(version.split(separator: ".").count == 3)
    #expect(version.allSatisfy { $0.isNumber || $0 == "." })
    await stack.stop()
}

@Test func tagDetailsParseParameterSizeAndQuantization() {
    #expect(OllamaWire.parameterSize(from: "qwen2.5-7b-instruct-q4_k_m.gguf") == "7B")
    #expect(OllamaWire.quantizationLevel(from: "qwen2.5-7b-instruct-q4_k_m.gguf") == "Q4_K_M")
    #expect(OllamaWire.parameterSize(from: "gemma-2b") == "2B")
    #expect(OllamaWire.quantizationLevel(from: "model-f16.gguf") == "F16")
    #expect(OllamaWire.parameterSize(from: "plain-model") == "")
}

@Test func normalTrafficFlushesPendingUnauthorizedSummary() async throws {
    let directory = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let audit = GatewayAuditLog(directory: directory)
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    for offset in 0..<5 {
        await audit.appendUnauthorized(
            GatewayAuditEntry(
                ts: base.addingTimeInterval(Double(offset) * 0.01), method: "GET",
                route: "/v1/models", outcome: "unauthorized", status: 401, durationMs: 0))
    }
    await audit.append(
        GatewayAuditEntry(
            ts: base.addingTimeInterval(1), method: "GET", route: "/v1/models", outcome: "ok",
            status: 200, durationMs: 1))
    let entries = await audit.tail(limit: 10)
    #expect(entries.count == 3)
    #expect(entries.contains { $0.detail?.contains("4 more") == true })
    #expect(entries.last?.outcome == "ok")
}

@Test func unauthorizedFloodAggregatesToOneEntryPerWindow() async throws {
    let directory = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let audit = GatewayAuditLog(directory: directory)
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    for offset in 0..<1000 {
        await audit.appendUnauthorized(
            GatewayAuditEntry(
                ts: base.addingTimeInterval(Double(offset) * 0.01), method: "GET",
                route: "/v1/models", outcome: "unauthorized", status: 401, durationMs: 0))
    }
    let duringFlood = await audit.tail(limit: 5000)
    #expect(duringFlood.count == 1)

    await audit.appendUnauthorized(
        GatewayAuditEntry(
            ts: base.addingTimeInterval(120), method: "GET", route: "/v1/models",
            outcome: "unauthorized", status: 401, durationMs: 0))
    let afterWindow = await audit.tail(limit: 5000)
    #expect(afterWindow.count == 3)
    #expect(afterWindow.contains { $0.detail?.contains("999 more") == true })
}
