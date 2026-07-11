import Foundation

enum OpenAIWire {
    struct ChatRequest {
        var model: String
        var messages: [ChatMessage]
        var stream: Bool
        var includeUsage: Bool = false
        var sampling: [String: JSONValue]
        var tools: [ToolSpec] = []
        var toolChoice: JSONValue?
    }

    static let honoredKeys: Set<String> = [
        "model", "messages", "stream", "stream_options", "temperature", "top_p",
        "max_tokens", "max_completion_tokens", "stop", "seed", "n", "frequency_penalty",
        "presence_penalty", "response_format", "tools", "tool_choice", "user",
    ]

    static func decodeChatRequest(_ rawBody: [String: Any]) throws -> ChatRequest {
        var body = rawBody
        if let bias = body["logit_bias"] as? [String: Any], bias.isEmpty {
            body.removeValue(forKey: "logit_bias")
        }
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let rawMessages = body["messages"] as? [[String: Any]], !rawMessages.isEmpty else {
            throw GatewayError(.badRequest, "messages is required")
        }
        try WireParamDecoding.rejectUnknownKeys(body, honored: honoredKeys, label: "parameter")
        let messages = try rawMessages.map(decodeMessage)
        var sampling = try decodeSampling(body)
        var includeUsage = false
        if let streamOptions = body["stream_options"] as? [String: Any] {
            includeUsage = streamOptions["include_usage"] as? Bool ?? false
        }
        if let responseFormat = try decodeResponseFormat(body["response_format"]) {
            sampling["response_format"] = responseFormat
        }
        let tools = try ToolWireDecoding.specs(from: body["tools"]) {
            GatewayError(.badRequest, $0)
        }
        var toolChoice: JSONValue?
        if let rawChoice = body["tool_choice"] {
            guard let choice = JSONValue.fromAny(rawChoice) else {
                throw GatewayError(.badRequest, "tool_choice must be a string or object")
            }
            toolChoice = choice
        }
        return ChatRequest(
            model: model,
            messages: messages,
            stream: body["stream"] as? Bool ?? false,
            includeUsage: includeUsage,
            sampling: sampling,
            tools: tools,
            toolChoice: toolChoice)
    }

    static func decodeSampling(_ body: [String: Any]) throws -> [String: JSONValue] {
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
        if let stop = try WireParamDecoding.stop(body["stop"], maxCount: 4) {
            sampling["stop"] = stop
        }
        if let seed = body["seed"] as? Int {
            sampling["seed"] = .int(seed)
        }
        if let n = body["n"] as? Int, n != 1 {
            throw GatewayError(
                .badRequest, "n greater than 1 is not supported", code: "unsupported_parameter")
        }
        if let frequencyPenalty = body["frequency_penalty"] as? Double {
            sampling["frequency_penalty"] = .double(frequencyPenalty)
        }
        if let presencePenalty = body["presence_penalty"] as? Double {
            sampling["presence_penalty"] = .double(presencePenalty)
        }
        return sampling
    }

    static func decodeResponseFormat(_ raw: Any?) throws -> JSONValue? {
        guard let raw else { return nil }
        guard let object = raw as? [String: Any], let type = object["type"] as? String else {
            throw GatewayError(.badRequest, "response_format must be an object with a type")
        }
        switch type {
        case "text":
            return nil
        case "json_object", "json_schema":
            guard let value = JSONValue.fromAny(object) else {
                throw GatewayError(.badRequest, "response_format is malformed")
            }
            return value
        default:
            throw GatewayError(
                .badRequest, "response_format type '\(type)' is not supported",
                code: "unsupported_parameter")
        }
    }

    private static func decodeMessage(_ raw: [String: Any]) throws -> ChatMessage {
        let rawRole = raw["role"] as? String ?? ""
        let role: ChatMessage.Role
        switch rawRole {
        case "system", "developer": role = .system
        case "user": role = .user
        case "assistant": role = .assistant
        case "tool": role = .tool
        default:
            throw GatewayError(.badRequest, "unsupported message role \(rawRole)")
        }
        if role == .tool {
            guard let callID = raw["tool_call_id"] as? String, !callID.isEmpty else {
                throw GatewayError(.badRequest, "tool messages require tool_call_id")
            }
            let content = raw["content"] as? String ?? ""
            return ChatMessage(
                role: .tool, content: content,
                toolCallID: callID, toolName: raw["name"] as? String)
        }
        let toolCalls = try decodeToolCalls(raw["tool_calls"])
        if let content = raw["content"] as? String {
            return ChatMessage(role: role, content: content, toolCalls: toolCalls)
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
            return ChatMessage(role: role, content: texts.joined(), toolCalls: toolCalls)
        }
        if role == .assistant, !toolCalls.isEmpty {
            return ChatMessage(role: .assistant, content: "", toolCalls: toolCalls)
        }
        throw GatewayError(.badRequest, "message content must be a string or text parts")
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
                throw GatewayError(
                    .badRequest, "each tool call must carry function.name")
            }
            let arguments: JSONValue
            if let encoded = function["arguments"] as? String {
                guard let data = encoded.data(using: .utf8),
                    let parsed = try? JSONSerialization.jsonObject(with: data)
                        as? [String: Any],
                    let value = JSONValue.fromAny(parsed)
                else {
                    throw GatewayError(
                        .badRequest, "tool call arguments must be a JSON-encoded object")
                }
                arguments = value
            } else if let object = function["arguments"] as? [String: Any],
                let value = JSONValue.fromAny(object)
            {
                arguments = value
            } else {
                arguments = .object([:])
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
        if let toolChoice = request.toolChoice {
            payload["tool_choice"] = toolChoice
        }
        for (key, value) in request.sampling {
            payload[key] = value
        }
        return .object(payload)
    }

    static func sseFrame(_ object: [String: Any]) -> Data {
        var frame = Data("data: ".utf8)
        frame.append(WireJSON.serialize(object))
        frame.append(contentsOf: Data("\n\n".utf8))
        return frame
    }

    static let sseDone = Data("data: [DONE]\n\n".utf8)

    static func chunkFrame(
        id: String, created: Int, model: String, content: String? = nil,
        reasoning: String? = nil, toolCall: ToolCall? = nil, toolCallIndex: Int = 0,
        finishReason: String? = nil, role: Bool = false
    ) -> [String: Any] {
        var delta: [String: Any] = [:]
        if role { delta["role"] = "assistant" }
        if let content { delta["content"] = content }
        if let reasoning { delta["reasoning_content"] = reasoning }
        if let toolCall {
            delta["tool_calls"] = [
                [
                    "index": toolCallIndex,
                    "id": toolCall.id,
                    "type": "function",
                    "function": [
                        "name": toolCall.name,
                        "arguments": toolCall.arguments.jsonString,
                    ],
                ]
            ]
        }
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
        id: String, created: Int, model: String, content: String, stats: GenerationStats?,
        toolCalls: [ToolCall] = []
    ) -> [String: Any] {
        var message: [String: Any] = ["role": "assistant", "content": content]
        if !toolCalls.isEmpty {
            message["tool_calls"] = toolCalls.enumerated().map { index, call in
                [
                    "index": index,
                    "id": call.id,
                    "type": "function",
                    "function": [
                        "name": call.name,
                        "arguments": call.arguments.jsonString,
                    ],
                ] as [String: Any]
            }
            if content.isEmpty { message["content"] = NSNull() }
        }
        let finishReason =
            !toolCalls.isEmpty ? "tool_calls" : stats?.finishReason ?? "stop"
        return [
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [
                [
                    "index": 0,
                    "message": message,
                    "finish_reason": finishReason,
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
