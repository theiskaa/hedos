import Foundation

struct OpenAIEmbeddingsHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        let inputs = try Self.inputs(from: body)
        let format = body["encoding_format"] as? String ?? "float"
        guard format == "float" || format == "base64" else {
            throw GatewayError(
                .badRequest, "encoding_format '\(format)' is not supported",
                code: "unsupported_parameter")
        }
        if body["dimensions"] != nil {
            throw GatewayError(
                .badRequest, "dimensions is not supported — no local runtime truncates embeddings",
                code: "unsupported_parameter")
        }
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
                    "\(record.name) returned \(vectors.count) embeddings for \(inputs.count) inputs"
                )
            }

            let data = vectors.enumerated().map { index, vector -> [String: Any] in
                let embedding: Any = format == "base64" ? Self.base64(vector) : vector
                return ["object": "embedding", "embedding": embedding, "index": index]
            }
            let promptTokens = finalStats?.promptTokens ?? 0
            try await responder.respond(
                status: 200,
                body: WireJSON.serialize([
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
        if body["input"] is [Int] || body["input"] is [[Int]] {
            throw GatewayError(
                .badRequest, "token array input is not supported — send text")
        }
        throw GatewayError(.badRequest, "input is required")
    }

    static func base64(_ vector: [Double]) -> String {
        var data = Data(capacity: vector.count * 4)
        for value in vector {
            var little = Float(value).bitPattern.littleEndian
            withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
        }
        return data.base64EncodedString()
    }
}
