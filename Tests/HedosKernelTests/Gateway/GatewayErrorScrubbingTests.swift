import Foundation
import Testing

@testable import HedosKernel

private struct BoomError: Error {}

private struct BoomHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        throw BoomError()
    }
}

private struct RuntimeFailedHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        throw KernelError.runtimeFailed("secret-path-detail")
    }
}

@Test func plainSwiftErrorAnswersGenericBodyButAuditsBoomError() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [GatewayRoute("POST", "/v1/boom", BoomHandler())])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("POST", stack.url("/v1/boom"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 500)
    let bodyString = String(data: data, encoding: .utf8) ?? ""
    #expect(!bodyString.contains("BoomError"))
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let errorObject = body["error"] as? [String: Any]
    #expect(errorObject?["message"] as? String == "internal error")

    let audited = await stack.audit.tail(limit: 1)
    #expect(audited.last?.outcome == "error")
    #expect(audited.last?.detail?.contains("BoomError") == true)
    await stack.stop()
}

@Test func runtimeFailedAnswersGenericBodyButAuditsSecretDetail() async throws {
    let stack = try await GatewayHarness.stack(
        routes: [GatewayRoute("POST", "/v1/runtime-boom", RuntimeFailedHandler())])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request("POST", stack.url("/v1/runtime-boom"), token: stack.token))
    #expect((response as! HTTPURLResponse).statusCode == 500)
    let bodyString = String(data: data, encoding: .utf8) ?? ""
    #expect(!bodyString.contains("secret-path-detail"))
    let body = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let errorObject = body["error"] as? [String: Any]
    #expect(errorObject?["message"] as? String == "the runtime failed to complete the request")

    let audited = await stack.audit.tail(limit: 1)
    #expect(audited.last?.outcome == "error")
    #expect(audited.last?.detail?.contains("secret-path-detail") == true)
    await stack.stop()
}
