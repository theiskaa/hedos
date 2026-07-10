public struct ModelProfile: Sendable {
    public let id: String
    public let schema: [ParamSpec]
    public let matches: @Sendable (ModelRecord) -> Bool

    public init(
        id: String,
        schema: [ParamSpec],
        matches: @escaping @Sendable (ModelRecord) -> Bool
    ) {
        self.id = id
        self.schema = schema
        self.matches = matches
    }
}

public struct ProfileRegistry: Sendable {
    public let profiles: [ModelProfile]

    public init(profiles: [ModelProfile]) {
        self.profiles = profiles
    }

    public func schema(for record: ModelRecord) -> [ParamSpec] {
        var specs: [ParamSpec] = []
        for profile in profiles where profile.matches(record) {
            for spec in profile.schema where !specs.contains(where: { $0.key == spec.key }) {
                specs.append(spec)
            }
        }
        if !specs.contains(where: { $0.key == "context_length" }),
            let contextSpec = Self.contextLengthSpec(for: record)
        {
            specs.append(contextSpec)
        }
        return specs
    }

    static let contextHonoringRuntimes: Set<String> = ["ollama", "llama-cpp"]

    public static func contextLengthSpec(for record: ModelRecord) -> ParamSpec? {
        guard record.capabilities.contains(.chat) || record.capabilities.contains(.complete)
        else { return nil }
        guard let runtime = record.runtime.id, contextHonoringRuntimes.contains(runtime)
        else { return nil }
        guard let window = record.contextLength else {
            return ParamSpec(
                key: "context_length", type: .int, range: [.int(512), .int(131072)])
        }
        return ParamSpec(
            key: "context_length", type: .int,
            defaultValue: .int(min(window, 32768)),
            range: [.int(min(512, window)), .int(window)])
    }

    public func refreshed(_ record: ModelRecord) -> ModelRecord {
        let schema = schema(for: record)
        guard !schema.isEmpty else { return record }
        var updated = record
        updated.params = schema
        return updated
    }

    static let thinkingRuntimes: Set<String> = ["ollama"]

    public static let builtin = ProfileRegistry(profiles: [
        ModelProfile(
            id: "text-generation",
            schema: [
                ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)]),
                ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)]),
                ParamSpec(key: "max_tokens", type: .int, range: [.int(1), .int(32768)]),
            ],
            matches: { record in
                record.capabilities.contains(.chat) || record.capabilities.contains(.complete)
            }),
        ModelProfile(
            id: "speech-synthesis",
            schema: [
                ParamSpec(key: "voice", type: .string),
                ParamSpec(
                    key: "speed", type: .float, defaultValue: .double(1.0),
                    range: [.double(0.5), .double(2.0)]),
            ],
            matches: { record in record.capabilities.contains(.speak) }),
        ModelProfile(
            id: "togglable-thinking",
            schema: [ParamSpec(key: "thinking", type: .bool)],
            matches: { record in
                record.capabilities.contains(.chat)
                    && thinkingRuntimes.contains(record.runtime.id ?? "")
            }),
    ])
}
