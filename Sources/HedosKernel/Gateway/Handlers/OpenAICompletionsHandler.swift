import Foundation

struct OpenAICompletionsHandler: GatewayHandling {
    static let honoredKeys: Set<String> = [
        "model", "prompt", "stream", "stream_options", "temperature", "top_p",
        "max_tokens", "max_completion_tokens", "stop", "seed", "n", "best_of",
        "frequency_penalty", "presence_penalty", "user",
    ]

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        let prompt = try Self.prompt(from: body)
        try WireParamDecoding.rejectUnknownKeys(
            body, honored: Self.honoredKeys, label: "parameter")
        if let best = body["best_of"] as? Int, best != 1 {
            throw GatewayError(
                .badRequest, "best_of greater than 1 is not supported",
                code: "unsupported_parameter")
        }
        let sampling = try OpenAIWire.decodeSampling(body)
        let stream = body["stream"] as? Bool ?? false
        var includeUsage = false
        if let streamOptions = body["stream_options"] as? [String: Any] {
            includeUsage = streamOptions["include_usage"] as? Bool ?? false
        }

        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .complete, kind: .stream, port: port, identity: identity)
        try GatewayParamGuard.require(
            sampling,
            honoredBy: try await port.honoredParams(modelID: record.id, capability: .complete),
            runtime: record.runtime.id)

        var payload: [String: JSONValue] = ["prompt": .string(prompt)]
        for (key, value) in sampling { payload[key] = value }
        let outStream = try await port.invoke(record.id, .complete, payload: .object(payload))
        let completionID = "cmpl-\(UUID().uuidString.lowercased())"
        let created = Int(Date().timeIntervalSince1970)

        if stream {
            let writer = try await responder.beginStream(contentType: "text/event-stream")
            var finalStats: GenerationStats?
            for try await chunk in outStream {
                switch chunk {
                case .text(let text):
                    try await writer.write(
                        OpenAIWire.sseFrame(
                            Self.chunk(
                                id: completionID, created: created, model: model, text: text,
                                finishReason: nil)))
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector, .toolCall, .segment:
                    break
                }
            }
            try await writer.write(
                OpenAIWire.sseFrame(
                    Self.chunk(
                        id: completionID, created: created, model: model, text: "",
                        finishReason: finalStats?.finishReason ?? "stop")))
            if includeUsage {
                try await writer.write(
                    OpenAIWire.sseFrame([
                        "id": completionID,
                        "object": "text_completion",
                        "created": created,
                        "model": model,
                        "choices": [],
                        "usage": OpenAIWire.usage(finalStats),
                    ]))
            }
            try await writer.write(OpenAIWire.sseDone)
            try await writer.end()
        } else {
            var text = ""
            var finalStats: GenerationStats?
            for try await chunk in outStream {
                switch chunk {
                case .text(let value):
                    text += value
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector, .toolCall, .segment:
                    break
                }
            }
            try await responder.respond(
                status: 200,
                body: WireJSON.serialize(
                    Self.completion(
                        id: completionID, created: created, model: model, text: text,
                        stats: finalStats)))
        }
        return .ok(model: record.id, capability: .complete)
    }

    static func prompt(from body: [String: Any]) throws -> String {
        if let single = body["prompt"] as? String { return single }
        if let array = body["prompt"] as? [String], array.count == 1 { return array[0] }
        if body["prompt"] is [Any] {
            throw GatewayError(
                .badRequest, "prompt array must hold exactly one string",
                code: "unsupported_parameter")
        }
        throw GatewayError(.badRequest, "prompt is required")
    }

    static func chunk(
        id: String, created: Int, model: String, text: String, finishReason: String?
    ) -> [String: Any] {
        var choice: [String: Any] = ["text": text, "index": 0, "logprobs": NSNull()]
        choice["finish_reason"] = finishReason ?? NSNull()
        return [
            "id": id,
            "object": "text_completion",
            "created": created,
            "model": model,
            "choices": [choice],
        ]
    }

    static func completion(
        id: String, created: Int, model: String, text: String, stats: GenerationStats?
    ) -> [String: Any] {
        [
            "id": id,
            "object": "text_completion",
            "created": created,
            "model": model,
            "choices": [
                [
                    "text": text,
                    "index": 0,
                    "logprobs": NSNull(),
                    "finish_reason": stats?.finishReason ?? "stop",
                ] as [String: Any]
            ],
            "usage": OpenAIWire.usage(stats),
        ]
    }
}
