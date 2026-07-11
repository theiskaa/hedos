import Foundation

enum ToolGrammarError: Error, Equatable {
    case noTools
    case unsupportedSchema(tool: String, detail: String)
}

enum ToolGrammar {
    static let callOpen = "<tool_call>"
    static let callClose = "</tool_call>"
    static let triggerPattern = "[\\s\\S]*?(<tool_call>[\\s\\S]*)"

    static func grammar(for tools: [ToolSpec]) throws -> String {
        guard !tools.isEmpty else { throw ToolGrammarError.noTools }
        var rules: [String] = []
        var callRules: [String] = []
        for (index, tool) in tools.enumerated() {
            guard tool.name.allSatisfy({ $0.isASCII && !$0.isNewline && $0 != "\"" && $0 != "\\" })
            else {
                throw ToolGrammarError.unsupportedSchema(
                    tool: tool.name, detail: "tool names must be plain ASCII")
            }
            let ruleName = "call-\(index)"
            let argsRule = "args-\(index)"
            callRules.append(ruleName)
            rules.append(
                "\(ruleName) ::= \"{\" space \"\\\"name\\\"\" space \":\" space "
                    + "\"\\\"\(tool.name)\\\"\" space \",\" space \"\\\"arguments\\\"\" "
                    + "space \":\" space \(argsRule) space \"}\"")
            rules.append(
                contentsOf: try valueRules(
                    named: argsRule, schema: tool.parameters, tool: tool.name))
        }
        var grammar = """
            root ::= "\(callOpen)" space call space "\(callClose)"
            call ::= \(callRules.joined(separator: " | "))
            space ::= [ \\t\\n\\r]*
            string ::= "\\"" ([^"\\\\] | "\\\\" .)* "\\""
            number ::= "-"? [0-9]+ ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
            integer ::= "-"? [0-9]+
            boolean ::= "true" | "false"

            """
        grammar += rules.joined(separator: "\n")
        grammar += "\n"
        return grammar
    }

    private static func valueRules(
        named name: String, schema: JSONValue, tool: String
    ) throws -> [String] {
        guard case .object(let fields) = schema else {
            throw ToolGrammarError.unsupportedSchema(tool: tool, detail: "schema must be an object")
        }
        if case .array(let options)? = fields["enum"] {
            let literals = try options.map { option -> String in
                guard let text = option.stringValue else {
                    throw ToolGrammarError.unsupportedSchema(
                        tool: tool, detail: "enum values must be strings")
                }
                return "\"\\\"\(escaped(text))\\\"\""
            }
            return ["\(name) ::= \(literals.joined(separator: " | "))"]
        }
        let type = fields["type"]?.stringValue ?? "object"
        switch type {
        case "string":
            return ["\(name) ::= string"]
        case "number":
            return ["\(name) ::= number"]
        case "integer":
            return ["\(name) ::= integer"]
        case "boolean":
            return ["\(name) ::= boolean"]
        case "array":
            guard let items = fields["items"] else {
                throw ToolGrammarError.unsupportedSchema(
                    tool: tool, detail: "arrays need an items schema")
            }
            let itemRule = "\(name)-item"
            var rules = [
                "\(name) ::= \"[\" space (\(itemRule) (space \",\" space \(itemRule))*)? space \"]\""
            ]
            rules.append(contentsOf: try valueRules(named: itemRule, schema: items, tool: tool))
            return rules
        case "object":
            return try objectRules(named: name, fields: fields, tool: tool)
        default:
            throw ToolGrammarError.unsupportedSchema(
                tool: tool, detail: "unsupported type \(type)")
        }
    }

    private static func objectRules(
        named name: String, fields: [String: JSONValue], tool: String
    ) throws -> [String] {
        guard case .object(let properties)? = fields["properties"] ?? .object([:]) else {
            throw ToolGrammarError.unsupportedSchema(
                tool: tool, detail: "properties must be an object")
        }
        var required: Set<String> = []
        if case .array(let names)? = fields["required"] {
            for entry in names {
                guard let text = entry.stringValue else {
                    throw ToolGrammarError.unsupportedSchema(
                        tool: tool, detail: "required entries must be strings")
                }
                required.insert(text)
            }
        }
        let ordered = properties.keys.sorted()
        guard !ordered.isEmpty else {
            return ["\(name) ::= \"{\" space \"}\""]
        }
        for key in ordered {
            guard key.allSatisfy({ $0.isASCII && !$0.isNewline && $0 != "\"" && $0 != "\\" })
            else {
                throw ToolGrammarError.unsupportedSchema(
                    tool: tool, detail: "property names must be plain ASCII")
            }
        }
        var rules: [String] = []
        var pairs: [(key: String, pair: String)] = []
        for (position, key) in ordered.enumerated() {
            let valueRule = "\(name)-p\(position)"
            pairs.append(
                (key, "\"\\\"\(escaped(key))\\\"\" space \":\" space \(valueRule)"))
            rules.append(
                contentsOf: try valueRules(
                    named: valueRule, schema: properties[key]!, tool: tool))
        }
        let requiredPairs = pairs.filter { required.contains($0.key) }
        let optionalPairs = pairs.filter { !required.contains($0.key) }
        let body: String
        if requiredPairs.isEmpty {
            let branches = optionalPairs.indices.map { start -> String in
                var branch = optionalPairs[start].pair
                for later in optionalPairs.indices.dropFirst(start + 1) {
                    branch += " (space \",\" space \(optionalPairs[later].pair))?"
                }
                return branch
            }
            body = "( \(branches.joined(separator: " | ")) )?"
        } else {
            var segments: [String] = []
            for (index, entry) in requiredPairs.enumerated() {
                segments.append(index == 0 ? entry.pair : "space \",\" space \(entry.pair)")
            }
            for entry in optionalPairs {
                segments.append("(space \",\" space \(entry.pair))?")
            }
            body = segments.joined(separator: " ")
        }
        rules.insert("\(name) ::= \"{\" space \(body) space \"}\"", at: 0)
        return rules
    }

    private static func escaped(_ text: String) -> String {
        text.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
    }

    static func systemBlock(for tools: [ToolSpec]) -> String {
        var lines = [
            "You can call tools. The available tools are:",
            "",
        ]
        for tool in tools {
            lines.append(
                "- \(tool.name): \(tool.description) Parameters schema: \(tool.parameters.jsonString)"
            )
        }
        lines.append("")
        lines.append(
            "To call a tool, reply with exactly one block of the form "
                + "\(callOpen){\"name\": \"<tool name>\", \"arguments\": {…}}\(callClose) "
                + "and nothing after it. Only call a tool when it is needed to answer.")
        return lines.joined(separator: "\n")
    }
}

struct ToolCallScanner {
    private var pending = ""
    private var inCall = false
    private var callBody = ""

    mutating func feed(_ piece: String) -> (text: String, call: ToolCall?) {
        pending += piece
        if inCall {
            return drainCall()
        }
        if let openRange = pending.range(of: ToolGrammar.callOpen) {
            let text = String(pending[..<openRange.lowerBound])
            callBody = String(pending[openRange.upperBound...])
            pending = ""
            inCall = true
            var result = drainCall()
            result.text = text + result.text
            return result
        }
        let safe = emittablePrefix()
        return (safe, nil)
    }

    mutating func flush() -> String {
        defer {
            pending = ""
            callBody = ""
            inCall = false
        }
        if inCall {
            return ""
        }
        return pending
    }

    private mutating func drainCall() -> (text: String, call: ToolCall?) {
        callBody += pending
        pending = ""
        guard let closeRange = callBody.range(of: ToolGrammar.callClose) else {
            return ("", nil)
        }
        let json = String(callBody[..<closeRange.lowerBound])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let remainder = String(callBody[closeRange.upperBound...])
        callBody = ""
        inCall = false
        pending = remainder
        guard let data = json.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let name = object["name"] as? String,
            let value = JSONValue.fromAny(object["arguments"] ?? [String: Any]())
        else {
            let raw = ToolGrammar.callOpen + json + ToolGrammar.callClose
            return (raw, nil)
        }
        return ("", ToolCall(name: name, arguments: value))
    }

    private mutating func emittablePrefix() -> String {
        let marker = ToolGrammar.callOpen
        for overlap in stride(from: min(marker.count - 1, pending.count), through: 1, by: -1) {
            let tail = String(pending.suffix(overlap))
            if marker.hasPrefix(tail) {
                let text = String(pending.dropLast(overlap))
                pending = tail
                return text
            }
        }
        let text = pending
        pending = ""
        return text
    }
}
