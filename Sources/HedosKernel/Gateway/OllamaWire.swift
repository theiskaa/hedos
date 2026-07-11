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
        var tools: [ToolSpec] = []
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
            let toolCalls = try decodeToolCalls(raw["tool_calls"])
            let content = raw["content"] as? String
            if role == .tool {
                return ChatMessage(
                    role: .tool, content: content ?? "",
                    toolCallID: raw["tool_call_id"] as? String,
                    toolName: raw["tool_name"] as? String)
            }
            if role == .assistant, !toolCalls.isEmpty {
                return ChatMessage(
                    role: .assistant, content: content ?? "", toolCalls: toolCalls)
            }
            guard let content else {
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
        let tools = try ToolWireDecoding.specs(from: body["tools"]) {
            GatewayError(.badRequest, $0)
        }
        return ChatRequest(
            model: model,
            messages: messages,
            stream: body["stream"] as? Bool ?? true,
            options: options,
            tools: tools)
    }

    private static func decodeToolCalls(_ raw: Any?) throws -> [ToolCall] {
        guard let raw else { return [] }
        guard let entries = raw as? [[String: Any]] else {
            throw GatewayError(.badRequest, "tool_calls must be an array")
        }
        return try entries.map { entry in
            guard let function = entry["function"] as? [String: Any],
                let name = function["name"] as? String, !name.isEmpty
            else {
                throw GatewayError(.badRequest, "each tool call must carry function.name")
            }
            let rawArguments = function["arguments"] as? [String: Any] ?? [:]
            guard let arguments = JSONValue.fromAny(rawArguments) else {
                throw GatewayError(.badRequest, "tool call arguments must be a JSON object")
            }
            let id = entry["id"] as? String
            return id.map { ToolCall(id: $0, name: name, arguments: arguments) }
                ?? ToolCall(name: name, arguments: arguments)
        }
    }

    static func chatPayload(_ request: ChatRequest) -> JSONValue {
        var payload: [String: JSONValue] = [
            "messages": .array(request.messages.map(\.payloadValue))
        ]
        if !request.tools.isEmpty {
            payload["tools"] = .array(request.tools.map(\.payloadValue))
        }
        for (key, value) in request.options {
            payload[key] = value
        }
        return .object(payload)
    }

    static func timestamp() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    static func line(_ object: [String: Any]) -> Data {
        var data = WireJSON.serialize(object)
        data.append(contentsOf: [0x0A])
        return data
    }

    static func delta(
        model: String, content: String? = nil, thinking: String? = nil,
        toolCall: ToolCall? = nil
    ) -> [String: Any] {
        var message: [String: Any] = ["role": "assistant", "content": content ?? ""]
        if let thinking { message["thinking"] = thinking }
        if let toolCall {
            message["tool_calls"] = [
                [
                    "function": [
                        "name": toolCall.name,
                        "arguments": toolCall.arguments.anyValue,
                    ]
                ]
            ]
        }
        return [
            "model": model,
            "created_at": timestamp(),
            "message": message,
            "done": false,
        ]
    }

    static func final(
        model: String, content: String = "", stats: GenerationStats?,
        toolCalls: [ToolCall] = []
    ) -> [String: Any] {
        var message: [String: Any] = ["role": "assistant", "content": content]
        if !toolCalls.isEmpty {
            message["tool_calls"] = toolCalls.map { call in
                [
                    "function": [
                        "name": call.name,
                        "arguments": call.arguments.anyValue,
                    ]
                ] as [String: Any]
            }
        }
        let doneReason =
            !toolCalls.isEmpty || stats?.finishReason == "tool_calls"
            ? "stop" : stats?.finishReason ?? "stop"
        var object: [String: Any] = [
            "model": model,
            "created_at": timestamp(),
            "message": message,
            "done": true,
            "done_reason": doneReason,
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
