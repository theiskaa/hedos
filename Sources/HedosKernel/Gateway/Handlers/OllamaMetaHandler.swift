import Foundation

struct OllamaVersionHandler: GatewayHandling {
    var surface: GatewaySurface { .ollama }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        try await responder.respond(
            status: 200, body: OpenAIWire.serialize(["version": "0.1.0-hedos"]))
        return .ok
    }
}

struct OllamaShowHandler: GatewayHandling {
    var surface: GatewaySurface { .ollama }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let requested = (body["model"] as? String) ?? (body["name"] as? String) else {
            throw GatewayError(.badRequest, "model is required")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(requested, shelf: shelf)
        guard identity.scopes.permitsModel(record.id) else {
            throw GatewayError(.forbidden, "this token is not scoped for that model")
        }
        try await responder.respond(
            status: 200,
            body: OpenAIWire.serialize([
                "details": OllamaWire.details(record),
                "capabilities": record.capabilities.contains(.chat) ? ["completion"] : [],
                "model_info": [:],
            ]))
        return .ok(model: record.id, capability: nil)
    }
}
