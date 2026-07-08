import Foundation

struct OpenAIEmbeddingsHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .embed)
        throw GatewayError(
            .notSupported, "\(record.name) has no embeddings runtime on this machine",
            code: "capability_unsupported")
    }
}
