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

    public var id: String?
    public var resolved: Resolution
    public var tier: RunTier
    public var alternatives: [String]

    public init(
        id: String? = nil,
        resolved: Resolution = .unresolved,
        tier: RunTier = .recipeNeeded,
        alternatives: [String] = []
    ) {
        self.id = id
        self.resolved = resolved
        self.tier = tier
        self.alternatives = alternatives
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
    public var execution: ExecutionMode
    public var footprintMB: Int?
    public var state: ModelState
    public var registeredAt: Date
    public var primaryWeightPath: String?

    public init(
        name: String,
        modality: Modality,
        capabilities: [Capability],
        source: ModelSource,
        runtime: RuntimeRef = .unresolved,
        params: [ParamSpec] = [],
        execution: ExecutionMode = .sync,
        footprintMB: Int? = nil,
        state: ModelState = .unresolved,
        registeredAt: Date = Date()
    ) {
        self.id = Self.stableID(for: source)
        self.name = name
        self.modality = modality
        self.capabilities = capabilities
        self.source = source
        self.runtime = runtime
        self.params = params
        self.execution = execution
        self.footprintMB = footprintMB
        self.state = state
        self.registeredAt = registeredAt
    }

    public static func stableID(for source: ModelSource) -> String {
        let digest = SHA256.hash(data: Data(source.identity.utf8))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }
}
