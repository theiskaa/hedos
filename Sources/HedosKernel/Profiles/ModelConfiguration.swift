extension ModelRecord {
    public func normalizedParamValues() -> [String: JSONValue] {
        var kept: [String: JSONValue] = [:]
        for (key, value) in paramValues {
            guard let spec = params.first(where: { $0.key == key }),
                let normalized = spec.normalized(value)
            else { continue }
            kept[key] = normalized
        }
        return kept
    }

    public func droppingVanishedParamValues() -> ModelRecord {
        var record = self
        record.paramValues = normalizedParamValues()
        return record
    }
}

enum ModelConfiguration {
    static func merged(
        record: ModelRecord, capability: Capability, payload: JSONValue,
        fallbackPrompt: String? = nil, sessionPrompt: String? = nil,
        appendedBlock: String? = nil
    ) -> JSONValue {
        let overrides = record.normalizedParamValues()
        let prompt: String?
        let block: String?
        if capability == .chat {
            if let sessionPrompt {
                prompt = trimmed(sessionPrompt)
            } else {
                prompt = trimmedSystemPrompt(record) ?? trimmed(fallbackPrompt)
            }
            block = trimmed(appendedBlock)
        } else {
            prompt = nil
            block = nil
        }
        guard !overrides.isEmpty || prompt != nil || block != nil else { return payload }
        guard var fields = objectFields(payload) else { return payload }
        for (key, value) in overrides where fields[key] == nil {
            fields[key] = value
        }
        if prompt != nil || block != nil, case .array(let turns)? = fields["messages"] {
            fields["messages"] = seeded(prompt: prompt, block: block, turns: turns)
        }
        return .object(fields)
    }

    private static func trimmedSystemPrompt(_ record: ModelRecord) -> String? {
        trimmed(record.systemPrompt)
    }

    private static func trimmed(_ prompt: String?) -> String? {
        guard let prompt else { return nil }
        let cleaned = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? nil : cleaned
    }

    private static func objectFields(_ payload: JSONValue) -> [String: JSONValue]? {
        switch payload {
        case .object(let fields): fields
        case .null: [:]
        default: nil
        }
    }

    private static func seeded(
        prompt: String?, block: String?, turns: [JSONValue]
    ) -> JSONValue {
        let systemIndex = turns.firstIndex { turn in
            guard case .object(let fields) = turn else { return false }
            return fields["role"] == .string("system")
        }
        if let systemIndex {
            guard let block, case .object(var fields) = turns[systemIndex],
                case .string(let existing)? = fields["content"]
            else { return .array(turns) }
            fields["content"] = .string(
                [existing, block].filter { !$0.isEmpty }.joined(separator: "\n\n"))
            var updated = turns
            updated[systemIndex] = .object(fields)
            return .array(updated)
        }
        let content = [prompt, block].compactMap { $0 }.joined(separator: "\n\n")
        guard !content.isEmpty else { return .array(turns) }
        let systemTurn = JSONValue.object([
            "role": .string("system"),
            "content": .string(content),
        ])
        return .array([systemTurn] + turns)
    }
}
