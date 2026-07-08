import Foundation

enum OllamaWire {
    static let optionKeys: [(option: String, payload: String)] = [
        ("temperature", "temperature"),
        ("top_p", "top_p"),
        ("num_predict", "max_tokens"),
        ("num_ctx", "context_length"),
    ]

    struct ChatRequest {
        var model: String
        var messages: [ChatMessage]
        var stream: Bool
        var options: [String: JSONValue]
    }

    static func decodeChatRequest(_ body: [String: Any]) throws -> ChatRequest {
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let rawMessages = body["messages"] as? [[String: Any]], !rawMessages.isEmpty else {
            throw GatewayError(.badRequest, "messages is required")
        }
        let messages = try rawMessages.map { raw -> ChatMessage in
            let rawRole = raw["role"] as? String ?? ""
            guard let role = ChatMessage.Role(rawValue: rawRole) else {
                throw GatewayError(.badRequest, "unsupported message role \(rawRole)")
            }
            guard let content = raw["content"] as? String else {
                throw GatewayError(.badRequest, "message content must be a string")
            }
            return ChatMessage(role: role, content: content)
        }
        var options: [String: JSONValue] = [:]
        if let rawOptions = body["options"] as? [String: Any] {
            for (option, payload) in optionKeys {
                if let int = rawOptions[option] as? Int {
                    options[payload] = .int(int)
                } else if let double = rawOptions[option] as? Double {
                    options[payload] = .double(double)
                }
            }
        }
        if let think = body["think"] as? Bool {
            options["thinking"] = .bool(think)
        }
        return ChatRequest(
            model: model,
            messages: messages,
            stream: body["stream"] as? Bool ?? true,
            options: options)
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
        for (key, value) in request.options {
            payload[key] = value
        }
        return .object(payload)
    }

    static func timestamp() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    static func line(_ object: [String: Any]) -> Data {
        var data = (try? JSONSerialization.data(withJSONObject: object)) ?? Data()
        data.append(contentsOf: [0x0A])
        return data
    }

    static func delta(model: String, content: String? = nil, thinking: String? = nil)
        -> [String: Any]
    {
        var message: [String: Any] = ["role": "assistant", "content": content ?? ""]
        if let thinking { message["thinking"] = thinking }
        return [
            "model": model,
            "created_at": timestamp(),
            "message": message,
            "done": false,
        ]
    }

    static func final(model: String, content: String = "", stats: GenerationStats?)
        -> [String: Any]
    {
        var object: [String: Any] = [
            "model": model,
            "created_at": timestamp(),
            "message": ["role": "assistant", "content": content],
            "done": true,
            "done_reason": "stop",
        ]
        if let durationMs = stats?.durationMs {
            object["total_duration"] = durationMs * 1_000_000
        }
        if let promptTokens = stats?.promptTokens {
            object["prompt_eval_count"] = promptTokens
        }
        if let completionTokens = stats?.completionTokens {
            object["eval_count"] = completionTokens
        }
        return object
    }

    static func tags(_ records: [ModelRecord]) -> [String: Any] {
        let formatter = ISO8601DateFormatter()
        return [
            "models": records.map { record -> [String: Any] in
                let name = record.alias ?? record.name
                return [
                    "name": name,
                    "model": name,
                    "modified_at": formatter.string(from: record.registeredAt),
                    "size": (record.footprintMB ?? 0) * 1_048_576,
                    "digest": "",
                    "details": details(record),
                ]
            }
        ]
    }

    static func details(_ record: ModelRecord) -> [String: Any] {
        var format = ""
        if let weightPath = record.primaryWeightPath {
            let ext = URL(fileURLWithPath: weightPath).pathExtension.lowercased()
            if !ext.isEmpty { format = ext }
        }
        return [
            "parent_model": "",
            "format": format,
            "family": "",
            "families": [],
            "parameter_size": "",
            "quantization_level": "",
        ]
    }
}
