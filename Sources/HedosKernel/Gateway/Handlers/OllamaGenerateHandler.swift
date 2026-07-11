import Foundation

struct OllamaGenerateHandler: GatewayHandling {
    static let honoredKeys: Set<String> = [
        "model", "prompt", "stream", "think", "format", "options",
    ]

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let prompt = body["prompt"] as? String else {
            throw GatewayError(.badRequest, "prompt is required")
        }
        try WireParamDecoding.rejectUnknownKeys(
            body, honored: Self.honoredKeys, label: "parameter")
        let options = try OllamaWire.decodeOptions(from: body)
        let stream = body["stream"] as? Bool ?? true

        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .complete, kind: .stream, port: port, identity: identity)
        try GatewayParamGuard.require(
            options,
            honoredBy: try await port.honoredParams(modelID: record.id, capability: .complete),
            runtime: record.runtime.id)

        var payload: [String: JSONValue] = ["prompt": .string(prompt)]
        for (key, value) in options { payload[key] = value }
        let outStream = try await port.invoke(record.id, .complete, payload: .object(payload))

        if stream {
            let writer = try await responder.beginStream(contentType: "application/x-ndjson")
            var finalStats: GenerationStats?
            for try await chunk in outStream {
                switch chunk {
                case .text(let text):
                    try await writer.write(
                        OllamaWire.line(OllamaWire.generateDelta(model: model, response: text)))
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector, .toolCall, .segment:
                    break
                }
            }
            try await writer.write(
                OllamaWire.line(OllamaWire.generateFinal(model: model, stats: finalStats)))
            try await writer.end()
        } else {
            var response = ""
            var finalStats: GenerationStats?
            for try await chunk in outStream {
                switch chunk {
                case .text(let text):
                    response += text
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector, .toolCall, .segment:
                    break
                }
            }
            var object = OllamaWire.generateFinal(model: model, stats: finalStats)
            object["response"] = response
            try await responder.respond(status: 200, body: WireJSON.serialize(object))
        }
        return .ok(model: record.id, capability: .complete)
    }
}
