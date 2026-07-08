import Foundation

enum OpenAIWire {
    struct ChatRequest {
        var model: String
        var messages: [ChatMessage]
        var stream: Bool
        var sampling: [String: JSONValue]
    }

    static func decodeChatRequest(_ body: [String: Any]) throws -> ChatRequest {
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let rawMessages = body["messages"] as? [[String: Any]], !rawMessages.isEmpty else {
            throw GatewayError(.badRequest, "messages is required")
        }
        let messages = try rawMessages.map(decodeMessage)
        var sampling: [String: JSONValue] = [:]
        if let temperature = body["temperature"] as? Double {
            sampling["temperature"] = .double(temperature)
        }
        if let topP = body["top_p"] as? Double {
            sampling["top_p"] = .double(topP)
        }
        if let maxTokens = body["max_tokens"] as? Int {
            sampling["max_tokens"] = .int(maxTokens)
        } else if let maxTokens = body["max_completion_tokens"] as? Int {
            sampling["max_tokens"] = .int(maxTokens)
        }
        return ChatRequest(
            model: model,
            messages: messages,
            stream: body["stream"] as? Bool ?? false,
            sampling: sampling)
    }

    private static func decodeMessage(_ raw: [String: Any]) throws -> ChatMessage {
        let rawRole = raw["role"] as? String ?? ""
        let role: ChatMessage.Role
        switch rawRole {
        case "system", "developer": role = .system
        case "user": role = .user
        case "assistant": role = .assistant
        default:
            throw GatewayError(.badRequest, "unsupported message role \(rawRole)")
        }
        if let content = raw["content"] as? String {
            return ChatMessage(role: role, content: content)
        }
        if let parts = raw["content"] as? [[String: Any]] {
            var texts: [String] = []
            for part in parts {
                guard part["type"] as? String == "text", let text = part["text"] as? String
                else {
                    throw GatewayError(
                        .badRequest, "only text content parts are supported")
                }
                texts.append(text)
            }
            return ChatMessage(role: role, content: texts.joined())
        }
        throw GatewayError(.badRequest, "message content must be a string or text parts")
    }

    static func chatPayload(_ request: ChatRequest) -> JSONValue {
        var payload: [String: JSONValue] = [
            "messages": .array(
                request.messages.map {
                    .object([
                        "role": .string($0.role.rawValue),
                        "content": .string($0.content),
                    ])
                })
        ]
        for (key, value) in request.sampling {
            payload[key] = value
        }
        return .object(payload)
    }

    static func serialize(_ object: [String: Any]) -> Data {
        (try? JSONSerialization.data(withJSONObject: object)) ?? Data()
    }

    static func sseFrame(_ object: [String: Any]) -> Data {
        var frame = Data("data: ".utf8)
        frame.append(serialize(object))
        frame.append(contentsOf: Data("\n\n".utf8))
        return frame
    }

    static let sseDone = Data("data: [DONE]\n\n".utf8)

    static func chunkFrame(
        id: String, created: Int, model: String, content: String? = nil,
        reasoning: String? = nil, finishReason: String? = nil, role: Bool = false
    ) -> [String: Any] {
        var delta: [String: Any] = [:]
        if role { delta["role"] = "assistant" }
        if let content { delta["content"] = content }
        if let reasoning { delta["reasoning_content"] = reasoning }
        var choice: [String: Any] = ["index": 0, "delta": delta]
        choice["finish_reason"] = finishReason ?? NSNull()
        return [
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [choice],
        ]
    }

    static func usage(_ stats: GenerationStats?) -> [String: Any] {
        let prompt = stats?.promptTokens ?? 0
        let completion = stats?.completionTokens ?? 0
        return [
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": prompt + completion,
        ]
    }

    static func completion(
        id: String, created: Int, model: String, content: String, stats: GenerationStats?
    ) -> [String: Any] {
        [
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [
                [
                    "index": 0,
                    "message": ["role": "assistant", "content": content],
                    "finish_reason": "stop",
                ]
            ],
            "usage": usage(stats),
        ]
    }

    static func modelsList(_ records: [ModelRecord]) -> [String: Any] {
        [
            "object": "list",
            "data": records.map { record in
                [
                    "id": record.alias ?? record.name,
                    "object": "model",
                    "created": Int(record.registeredAt.timeIntervalSince1970),
                    "owned_by": "hedos",
                ]
            },
        ]
    }
}
