import Foundation

struct OllamaVersionHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        try await responder.respond(
            status: 200, body: WireJSON.serialize(["version": "0.5.0"]))
        return .ok
    }
}

struct OllamaShowHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let requested = (body["model"] as? String) ?? (body["name"] as? String) else {
            throw GatewayError(.badRequest, "model is required")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(
            requested, shelf: shelf, scopes: identity.scopes)
        var capabilities: [String] = []
        if record.capabilities.contains(.chat) || record.capabilities.contains(.complete) {
            capabilities.append("completion")
        }
        if record.capabilities.contains(.embed) { capabilities.append("embedding") }
        if record.capabilities.contains(.see) { capabilities.append("vision") }
        if try await port.supportsTools(modelID: record.id) { capabilities.append("tools") }
        try await responder.respond(
            status: 200,
            body: WireJSON.serialize([
                "details": OllamaWire.details(record),
                "capabilities": capabilities,
                "model_info": [:],
            ]))
        return .ok(model: record.id, capability: nil)
    }
}
