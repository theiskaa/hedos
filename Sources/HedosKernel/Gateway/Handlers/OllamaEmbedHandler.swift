import Foundation

struct OllamaEmbedHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        if body["truncate"] != nil {
            throw GatewayError(
                .badRequest, "the parameter 'truncate' is not supported",
                code: "unsupported_parameter")
        }
        if body["dimensions"] != nil {
            throw GatewayError(
                .badRequest, "the parameter 'dimensions' is not supported",
                code: "unsupported_parameter")
        }
        let legacy = request.path == "/api/embeddings"
        let inputs = try Self.inputs(from: body, legacy: legacy)
        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .embed, kind: .stream, port: port, identity: identity)

        let inputPayload: JSONValue =
            inputs.count == 1 ? .string(inputs[0]) : .array(inputs.map(JSONValue.string))
        do {
            let stream = try await port.invoke(
                record.id, .embed, payload: .object(["input": inputPayload]))
            var vectors: [[Double]] = []
            var finalStats: GenerationStats?
            for try await chunk in stream {
                switch chunk {
                case .vector(let vector):
                    vectors.append(vector)
                case .done(let stats):
                    finalStats = stats
                case .text, .thinking, .audio, .status, .toolCall, .segment:
                    break
                }
            }
            guard !vectors.isEmpty else {
                throw GatewayError(.serverError, "\(record.name) produced no embeddings")
            }
            guard vectors.count == inputs.count else {
                throw GatewayError(
                    .serverError,
                    "\(record.name) returned \(vectors.count) embeddings for \(inputs.count) inputs")
            }
            if legacy {
                try await responder.respond(
                    status: 200,
                    body: WireJSON.serialize(["embedding": vectors[0]]))
            } else {
                var object: [String: Any] = ["model": model, "embeddings": vectors]
                if let promptTokens = finalStats?.promptTokens {
                    object["prompt_eval_count"] = promptTokens
                }
                try await responder.respond(status: 200, body: WireJSON.serialize(object))
            }
            return .ok(model: record.id, capability: .embed)
        } catch KernelError.capabilityUnsupported {
            throw GatewayError(
                .notSupported, "\(record.name) has no embeddings runtime on this machine",
                code: "capability_unsupported")
        }
    }

    static func inputs(from body: [String: Any], legacy: Bool) throws -> [String] {
        let key = legacy ? "prompt" : "input"
        if let single = body[key] as? String {
            guard !single.isEmpty else { throw GatewayError(.badRequest, "\(key) is required") }
            return [single]
        }
        if !legacy, let array = body["input"] as? [String] {
            guard !array.isEmpty else { throw GatewayError(.badRequest, "input is required") }
            return array
        }
        throw GatewayError(.badRequest, "\(key) is required")
    }
}
