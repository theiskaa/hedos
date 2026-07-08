import Foundation
import Testing

@testable import HedosKernel

private func model(_ name: String, alias: String? = nil, ready: Bool = true) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(name).gguf")
    record.name = name
    record.alias = alias
    record.state = ready ? .ready : .unresolved
    return record
}

@Test func modelsListShapeAndReadyFilter() async throws {
    let ready = model("qwen3:latest")
    let broken = model("broken-model", ready: false)
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(records: [ready, broken]),
        routes: GatewayRouter.standardRoutes())
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/models"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    #expect(object["object"] as? String == "list")
    let entries = object["data"] as! [[String: Any]]
    #expect(entries.count == 1)
    #expect(entries[0]["id"] as? String == "qwen3:latest")
    #expect(entries[0]["object"] as? String == "model")
    #expect(entries[0]["owned_by"] as? String == "hedos")
    #expect(entries[0]["created"] as? Int == Int(ready.registeredAt.timeIntervalSince1970))
    await stack.stop()
}

@Test func modelsListRendersAliasAndFiltersScope() async throws {
    let allowed = model("allowed-model", alias: "my-favorite")
    let hidden = model("hidden-model")
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(records: [allowed, hidden]),
        routes: GatewayRouter.standardRoutes(),
        scopes: GatewayScopes(models: [allowed.id], capabilities: nil))
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request("GET", stack.url("/v1/models"), token: stack.token))
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let entries = object["data"] as! [[String: Any]]
    #expect(entries.count == 1)
    #expect(entries[0]["id"] as? String == "my-favorite")
    await stack.stop()
}
