import CryptoKit
import Foundation

public struct ModelSource: Codable, Hashable, Sendable {
    public var kind: SourceKind
    public var path: String
    public var repo: String?
    public var ref: String?

    public init(kind: SourceKind, path: String, repo: String? = nil, ref: String? = nil) {
        self.kind = kind
        self.path = path
        self.repo = repo
        self.ref = ref
    }

    public var identity: String {
        "\(kind.rawValue)|\(path)|\(repo ?? "")"
    }
}

public struct RuntimeRef: Codable, Hashable, Sendable {
    public enum Resolution: String, Codable, Hashable, Sendable {
        case auto
        case user
        case unresolved
    }

    public var id: RuntimeID?
    public var resolved: Resolution
    public var tier: RunTier
    public var alternatives: [RuntimeID]
    public var confirmedAt: Date?

    public init(
        id: RuntimeID? = nil,
        resolved: Resolution = .unresolved,
        tier: RunTier = .recipeNeeded,
        alternatives: [RuntimeID] = [],
        confirmedAt: Date? = nil
    ) {
        self.id = id
        self.resolved = resolved
        self.tier = tier
        self.alternatives = alternatives
        self.confirmedAt = confirmedAt
    }

    public static let unresolved = RuntimeRef()
}

public struct ParamSpec: Codable, Hashable, Sendable {
    public enum ParamType: String, Codable, Hashable, Sendable {
        case int
        case float
        case bool
        case string
        case enumeration = "enum"
    }

    public var key: String
    public var type: ParamType
    public var defaultValue: JSONValue?
    public var range: [JSONValue]?
    public var values: [String]?

    public init(
        key: String,
        type: ParamType,
        defaultValue: JSONValue? = nil,
        range: [JSONValue]? = nil,
        values: [String]? = nil
    ) {
        self.key = key
        self.type = type
        self.defaultValue = defaultValue
        self.range = range
        self.values = values
    }

    enum CodingKeys: String, CodingKey {
        case key, type, range, values
        case defaultValue = "default"
    }
}

public struct ModelRecord: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public var name: String
    public var modality: Modality
    public var capabilities: [Capability]
    public var source: ModelSource
    public var runtime: RuntimeRef
    public var params: [ParamSpec]
    public var paramValues: [String: JSONValue]
    public var systemPrompt: String?
    public var alias: String?
    public var execution: ExecutionMode
    public var footprintMB: Int?
    public var state: ModelState
    public var registeredAt: Date
    public var primaryWeightPath: String?
    public var contextLength: Int?
    public var hasChatTemplate: Bool?
    public var stopTokens: [String]?

    public init(
        name: String,
        modality: Modality,
        capabilities: [Capability],
        source: ModelSource,
        runtime: RuntimeRef = .unresolved,
        params: [ParamSpec] = [],
        paramValues: [String: JSONValue] = [:],
        systemPrompt: String? = nil,
        alias: String? = nil,
        execution: ExecutionMode = .sync,
        footprintMB: Int? = nil,
        state: ModelState = .unresolved,
        registeredAt: Date = Date(),
        contextLength: Int? = nil,
        hasChatTemplate: Bool? = nil,
        stopTokens: [String]? = nil
    ) {
        self.id = Self.stableID(for: source)
        self.name = name
        self.modality = modality
        self.capabilities = capabilities
        self.source = source
        self.runtime = runtime
        self.params = params
        self.paramValues = paramValues
        self.systemPrompt = systemPrompt
        self.alias = alias
        self.execution = execution
        self.footprintMB = footprintMB
        self.state = state
        self.registeredAt = registeredAt
        self.contextLength = contextLength
        self.hasChatTemplate = hasChatTemplate
        self.stopTokens = stopTokens
    }

    enum CodingKeys: String, CodingKey {
        case id, name, modality, capabilities, source, runtime, params, paramValues
        case systemPrompt, alias, execution, footprintMB, state, registeredAt
        case primaryWeightPath
        case contextLength, hasChatTemplate, stopTokens
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.id = try container.decode(String.self, forKey: .id)
        self.name = try container.decode(String.self, forKey: .name)
        self.modality = try container.decode(Modality.self, forKey: .modality)
        self.capabilities = try container.decode([Capability].self, forKey: .capabilities)
        self.source = try container.decode(ModelSource.self, forKey: .source)
        self.runtime = try container.decode(RuntimeRef.self, forKey: .runtime)
        self.params = try container.decode([ParamSpec].self, forKey: .params)
        self.paramValues =
            try container.decodeIfPresent([String: JSONValue].self, forKey: .paramValues) ?? [:]
        self.systemPrompt = try container.decodeIfPresent(String.self, forKey: .systemPrompt)
        self.alias = try container.decodeIfPresent(String.self, forKey: .alias)
        self.execution = try container.decode(ExecutionMode.self, forKey: .execution)
        self.footprintMB = try container.decodeIfPresent(Int.self, forKey: .footprintMB)
        self.state = try container.decode(ModelState.self, forKey: .state)
        self.registeredAt = try container.decode(Date.self, forKey: .registeredAt)
        self.primaryWeightPath = try container.decodeIfPresent(
            String.self, forKey: .primaryWeightPath)
        self.contextLength = try container.decodeIfPresent(Int.self, forKey: .contextLength)
        self.hasChatTemplate = try container.decodeIfPresent(Bool.self, forKey: .hasChatTemplate)
        self.stopTokens = try container.decodeIfPresent([String].self, forKey: .stopTokens)
    }

    public var displayName: String {
        guard let alias, !alias.isEmpty else { return name }
        return alias
    }

    public static func stableID(for source: ModelSource) -> String {
        let digest = SHA256.hash(data: Data(source.identity.utf8))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }
}
