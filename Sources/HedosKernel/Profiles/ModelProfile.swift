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

    static let contextHonoringRuntimes: Set<RuntimeID> = [.ollama, .llamaCpp]

    public static func contextLengthSpec(for record: ModelRecord) -> ParamSpec? {
        guard record.capabilities.contains(.chat) || record.capabilities.contains(.complete)
        else { return nil }
        guard let runtime = record.runtime.id, contextHonoringRuntimes.contains(runtime)
        else { return nil }
        guard let window = record.contextLength, window > 0 else {
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

    static let thinkingRuntimes: Set<RuntimeID> = [.ollama, .mlxLm]

    static let temperatureSpec = ParamSpec(
        key: "temperature", type: .float, range: [.double(0), .double(2)])
    static let topPSpec = ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)])
    static let topKSpec = ParamSpec(key: "top_k", type: .int, range: [.int(0), .int(100)])
    static let minPSpec = ParamSpec(key: "min_p", type: .float, range: [.double(0), .double(1)])
    static let maxTokensSpec = ParamSpec(
        key: "max_tokens", type: .int, range: [.int(1), .int(32768)])
    static let repeatPenaltySpec = ParamSpec(
        key: "repeat_penalty", type: .float, range: [.double(0.5), .double(2)])
    static let frequencyPenaltySpec = ParamSpec(
        key: "frequency_penalty", type: .float, range: [.double(-2), .double(2)])
    static let presencePenaltySpec = ParamSpec(
        key: "presence_penalty", type: .float, range: [.double(-2), .double(2)])
    static let seedSpec = ParamSpec(key: "seed", type: .int)
    static let stopSpec = ParamSpec(key: "stop", type: .string)

    static func runtimeExtras(
        id: String, runtime: RuntimeID, _ schema: [ParamSpec]
    ) -> ModelProfile {
        ModelProfile(
            id: id, schema: schema,
            matches: { record in
                (record.capabilities.contains(.chat) || record.capabilities.contains(.complete))
                    && record.runtime.id == runtime
            })
    }

    public static let builtin = ProfileRegistry(profiles: [
        ModelProfile(
            id: "text-generation",
            schema: [temperatureSpec, topPSpec, maxTokensSpec],
            matches: { record in
                record.capabilities.contains(.chat) || record.capabilities.contains(.complete)
            }),
        runtimeExtras(
            id: "sampling-llama-cpp", runtime: .llamaCpp,
            [
                topKSpec, minPSpec, repeatPenaltySpec, frequencyPenaltySpec,
                presencePenaltySpec, seedSpec, stopSpec,
            ]),
        runtimeExtras(
            id: "sampling-mlx-swift", runtime: .mlxSwift, [repeatPenaltySpec, stopSpec]),
        runtimeExtras(
            id: "sampling-mlx-lm", runtime: .mlxLm,
            [topKSpec, minPSpec, repeatPenaltySpec, seedSpec, stopSpec]),
        runtimeExtras(
            id: "sampling-ollama", runtime: .ollama,
            [
                topKSpec, minPSpec, seedSpec, repeatPenaltySpec, frequencyPenaltySpec,
                presencePenaltySpec, stopSpec,
            ]),
        runtimeExtras(
            id: "sampling-endpoint", runtime: .openAIEndpoint,
            [stopSpec, seedSpec, frequencyPenaltySpec, presencePenaltySpec]),
        runtimeExtras(
            id: "sampling-apple", runtime: .appleFoundation, [topKSpec, seedSpec]),
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
                    && record.runtime.id.map(thinkingRuntimes.contains) ?? false
            }),
    ])
}
