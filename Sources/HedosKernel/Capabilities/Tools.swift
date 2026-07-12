import Foundation

public struct ToolSpec: Codable, Sendable, Hashable {
    public var name: String
    public var description: String
    public var parameters: JSONValue

    public init(name: String, description: String, parameters: JSONValue) {
        self.name = name
        self.description = description
        self.parameters = parameters
    }
}

public struct ToolCall: Codable, Sendable, Hashable {
    public var id: String
    public var name: String
    public var arguments: JSONValue

    public init(id: String = UUID().uuidString.lowercased(), name: String, arguments: JSONValue) {
        self.id = id
        self.name = name
        self.arguments = arguments
    }
}

extension ToolSpec {
    public var payloadValue: JSONValue {
        .object([
            "name": .string(name),
            "description": .string(description),
            "parameters": parameters,
        ])
    }
}

extension ToolCall {
    public var payloadValue: JSONValue {
        .object([
            "id": .string(id),
            "name": .string(name),
            "arguments": arguments,
        ])
    }

    public static func fromPayload(_ value: JSONValue) -> ToolCall? {
        guard case .object(let object) = value,
            let name = object["name"]?.stringValue
        else { return nil }
        let arguments = object["arguments"] ?? .object([:])
        guard case .object = arguments else { return nil }
        if let id = object["id"]?.stringValue, !id.isEmpty {
            return ToolCall(id: id, name: name, arguments: arguments)
        }
        return ToolCall(name: name, arguments: arguments)
    }
}

extension ChatMessage {
    public var payloadValue: JSONValue {
        var object: [String: JSONValue] = [
            "role": .string(role.rawValue),
            "content": .string(content),
        ]
        if !toolCalls.isEmpty {
            object["tool_calls"] = .array(toolCalls.map(\.payloadValue))
        }
        if let toolCallID { object["tool_call_id"] = .string(toolCallID) }
        if let toolName { object["tool_name"] = .string(toolName) }
        let images = attachments.filter { $0.kind == .image }
        if !images.isEmpty {
            object["images"] = .array(images.map { .string($0.data.base64EncodedString()) })
        }
        return .object(object)
    }
}

extension JSONValue {
    public static func fromAny(_ any: Any) -> JSONValue? {
        switch any {
        case is NSNull:
            return .null
        case let number as NSNumber:
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return .bool(number.boolValue)
            }
            let objCType = String(cString: number.objCType)
            if !objCType.contains("d"), !objCType.contains("f") {
                return .int(number.intValue)
            }
            return .double(number.doubleValue)
        case let string as String:
            return .string(string)
        case let array as [Any]:
            var values: [JSONValue] = []
            for element in array {
                guard let value = fromAny(element) else { return nil }
                values.append(value)
            }
            return .array(values)
        case let object as [String: Any]:
            var fields: [String: JSONValue] = [:]
            for (key, element) in object {
                guard let value = fromAny(element) else { return nil }
                fields[key] = value
            }
            return .object(fields)
        default:
            return nil
        }
    }

    public var jsonString: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        guard let data = try? encoder.encode(self) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    public var anyValue: Any {
        switch self {
        case .null: NSNull()
        case .bool(let value): value
        case .int(let value): value
        case .double(let value): value
        case .string(let value): value
        case .array(let values): values.map(\.anyValue)
        case .object(let fields): fields.mapValues(\.anyValue)
        }
    }
}

enum ToolWireDecoding {
    static func specs(
        from raw: Any?, badRequest: (String) -> Error
    ) throws -> [ToolSpec] {
        guard let raw else { return [] }
        guard let entries = raw as? [[String: Any]] else {
            throw badRequest("tools must be an array of function tools")
        }
        return try entries.map { entry in
            guard entry["type"] as? String ?? "function" == "function",
                let function = entry["function"] as? [String: Any],
                let name = function["name"] as? String, !name.isEmpty
            else {
                throw badRequest("each tool must be {type: \"function\", function: {name}}")
            }
            let parameters =
                (function["parameters"] as? [String: Any]).flatMap { JSONValue.fromAny($0) }
                ?? .object([:])
            return ToolSpec(
                name: name,
                description: function["description"] as? String ?? "",
                parameters: parameters)
        }
    }
}

extension ToolSpec {
    public static func fromPayload(_ value: JSONValue) -> ToolSpec? {
        guard case .object(let object) = value,
            let name = object["name"]?.stringValue
        else { return nil }
        return ToolSpec(
            name: name,
            description: object["description"]?.stringValue ?? "",
            parameters: object["parameters"] ?? .object([:]))
    }

    public static func fromPayloadArray(_ value: JSONValue?) -> [ToolSpec] {
        guard case .array(let entries)? = value else { return [] }
        return entries.compactMap(fromPayload)
    }
}

extension ChatMessage {
    public static func fromPayload(_ value: JSONValue) -> ChatMessage? {
        guard case .object(let fields) = value,
            let rawRole = fields["role"]?.stringValue,
            let role = Role(rawValue: rawRole)
        else { return nil }
        let content = fields["content"]?.stringValue ?? ""
        var toolCalls: [ToolCall] = []
        if case .array(let calls)? = fields["tool_calls"] {
            toolCalls = calls.compactMap(ToolCall.fromPayload)
        }
        return ChatMessage(
            role: role, content: content, toolCalls: toolCalls,
            toolCallID: fields["tool_call_id"]?.stringValue,
            toolName: fields["tool_name"]?.stringValue)
    }

    public static func parseStrict(_ value: JSONValue, index: Int) throws -> ChatMessage {
        guard case .object(let fields) = value else {
            throw KernelError.payloadInvalid("message at index \(index) is not an object")
        }
        guard let rawRole = fields["role"]?.stringValue, let role = Role(rawValue: rawRole) else {
            throw KernelError.payloadInvalid(
                "message at index \(index) has a missing or unknown role")
        }
        if let content = fields["content"], content != .null {
            guard case .string = content else {
                throw KernelError.payloadInvalid(
                    "message at index \(index) has non-string content")
            }
        }
        var toolCalls: [ToolCall] = []
        if case .array(let calls)? = fields["tool_calls"] {
            toolCalls = calls.compactMap(ToolCall.fromPayload)
        }
        return ChatMessage(
            role: role, content: fields["content"]?.stringValue ?? "", toolCalls: toolCalls,
            toolCallID: fields["tool_call_id"]?.stringValue,
            toolName: fields["tool_name"]?.stringValue)
    }

    public static func parseAll(from object: [String: JSONValue]) throws -> [ChatMessage] {
        if case .array(let rawMessages)? = object["messages"] {
            return try rawMessages.enumerated().map { try parseStrict($1, index: $0) }
        }
        if case .string(let prompt)? = object["prompt"] {
            return [ChatMessage(role: .user, content: prompt)]
        }
        throw KernelError.payloadInvalid("chat payload must carry a messages array or a prompt")
    }

    public var inlinedToolTranscript: ChatMessage {
        guard role == .assistant, !toolCalls.isEmpty else { return self }
        let blocks = toolCalls.map { call in
            "<tool_call>{\"name\": \"\(call.name)\", "
                + "\"arguments\": \(call.arguments.jsonString)}</tool_call>"
        }
        let joined = ([content] + blocks).filter { !$0.isEmpty }.joined(separator: "\n")
        return ChatMessage(role: .assistant, content: joined)
    }
}
