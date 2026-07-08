import Foundation

struct OpenAIImagesHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let prompt = body["prompt"] as? String, !prompt.isEmpty else {
            throw GatewayError(.badRequest, "prompt is required")
        }
        if let n = body["n"] as? Int, n > 1 {
            throw GatewayError(.badRequest, "only one image per request is available")
        }
        if let format = body["response_format"] as? String, format != "b64_json" {
            throw GatewayError(
                .badRequest, "only b64_json output is available — images never leave this machine")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .image)
        try await GatewayBackpressure.require(port, record: record, kind: .job)

        var payload: [String: JSONValue] = ["prompt": .string(prompt)]
        if let size = body["size"] as? String { payload["size"] = .string(size) }
        let jobID = try await port.submit(record.id, .image, payload: .object(payload))

        let events = await port.jobEvents(id: jobID)
        let artifactIDs = try await withTaskCancellationHandler {
            var result: [String] = []
            for await event in events {
                switch event {
                case .done(let artifacts):
                    result = artifacts
                case .failed(let message):
                    throw GatewayError(.serverError, message)
                case .cancelled:
                    throw GatewayError(.serverError, "generation was cancelled")
                default:
                    continue
                }
            }
            return result
        } onCancel: {
            Task { await port.cancel(jobID: jobID) }
        }

        guard let artifactID = artifactIDs.first,
            let imageData = try await port.artifactData(id: artifactID)
        else {
            throw GatewayError(.serverError, "\(record.name) produced no image")
        }
        try await responder.respond(
            status: 200,
            body: OpenAIWire.serialize([
                "created": Int(Date().timeIntervalSince1970),
                "data": [["b64_json": imageData.base64EncodedString()]],
            ]))
        return .ok(model: record.id, capability: .image)
    }
}
