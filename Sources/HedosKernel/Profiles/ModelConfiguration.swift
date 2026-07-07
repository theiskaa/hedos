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
        fallbackPrompt: String? = nil
    ) -> JSONValue {
        let overrides = record.normalizedParamValues()
        let prompt =
            capability == .chat
            ? (trimmedSystemPrompt(record) ?? trimmed(fallbackPrompt)) : nil
        guard !overrides.isEmpty || prompt != nil else { return payload }
        guard var fields = objectFields(payload) else { return payload }
        for (key, value) in overrides where fields[key] == nil {
            fields[key] = value
        }
        if let prompt, case .array(let turns)? = fields["messages"] {
            fields["messages"] = prepending(prompt, to: turns)
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

    private static func prepending(_ prompt: String, to turns: [JSONValue]) -> JSONValue {
        let hasSystemTurn = turns.contains { turn in
            guard case .object(let fields) = turn else { return false }
            return fields["role"] == .string("system")
        }
        guard !hasSystemTurn else { return .array(turns) }
        let systemTurn = JSONValue.object([
            "role": .string("system"),
            "content": .string(prompt),
        ])
        return .array([systemTurn] + turns)
    }
}
