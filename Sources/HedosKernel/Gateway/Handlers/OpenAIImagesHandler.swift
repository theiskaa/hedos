import Foundation

struct OpenAIImagesHandler: GatewayHandling {
    static let runTimeoutSeconds = 600

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
        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .image, kind: .job, port: port, identity: identity)

        var payload: [String: JSONValue] = ["prompt": .string(prompt)]
        if let size = body["size"] as? String { payload["size"] = .string(size) }
        let jobID = try await port.submit(record.id, .image, payload: .object(payload))

        let events = await port.jobEvents(id: jobID)
        let artifactIDs = try await withTaskCancellationHandler {
            try await withThrowingTaskGroup(of: [String].self) { group in
                group.addTask {
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
                }
                group.addTask {
                    try await Task.sleep(for: .seconds(Self.runTimeoutSeconds))
                    await port.cancel(jobID: jobID)
                    throw GatewayError(
                        .timeout, "image generation ran longer than \(Self.runTimeoutSeconds)s")
                }
                let result = try await group.next()!
                group.cancelAll()
                return result
            }
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
            body: WireJSON.serialize([
                "created": Int(Date().timeIntervalSince1970),
                "data": [["b64_json": imageData.base64EncodedString()]],
            ]))
        return .ok(model: record.id, capability: .image)
    }
}
