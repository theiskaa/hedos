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
        let inputs = try Self.inputs(from: body)
        if let format = body["encoding_format"] as? String, format != "float" {
            throw GatewayError(
                .badRequest, "only float output is available — set encoding_format to float")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .embed)
        try await GatewayBackpressure.require(port, record: record, kind: .stream)

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
                case .text, .thinking, .audio, .status:
                    break
                }
            }
            guard !vectors.isEmpty else {
                throw GatewayError(.serverError, "\(record.name) produced no embeddings")
            }
            guard vectors.count == inputs.count else {
                throw GatewayError(
                    .serverError,
                    "\(record.name) returned \(vectors.count) embeddings for \(inputs.count) inputs"
                )
            }

            let data = vectors.enumerated().map { index, vector -> [String: Any] in
                ["object": "embedding", "embedding": vector, "index": index]
            }
            let promptTokens = finalStats?.promptTokens ?? 0
            try await responder.respond(
                status: 200,
                body: OpenAIWire.serialize([
                    "object": "list",
                    "data": data,
                    "model": model,
                    "usage": [
                        "prompt_tokens": promptTokens,
                        "total_tokens": promptTokens,
                    ],
                ]))
            return .ok(model: record.id, capability: .embed)
        } catch KernelError.capabilityUnsupported {
            throw GatewayError(
                .notSupported, "\(record.name) has no embeddings runtime on this machine",
                code: "capability_unsupported")
        }
    }

    private static func inputs(from body: [String: Any]) throws -> [String] {
        if let single = body["input"] as? String {
            guard !single.isEmpty else { throw GatewayError(.badRequest, "input is required") }
            return [single]
        }
        if let array = body["input"] as? [String] {
            guard !array.isEmpty else { throw GatewayError(.badRequest, "input is required") }
            return array
        }
        throw GatewayError(.badRequest, "input is required")
    }
}
