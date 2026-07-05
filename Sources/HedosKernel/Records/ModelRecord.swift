import CryptoKit
import Foundation

/// Where a model's weights live. `path` always points at what some other
/// tool (or the user) put on disk — Hedos never moves, copies, or
/// re-downloads weights. Source and runtime are independent by design.
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

    /// Canonical identity string; the stable record ID is derived from this,
    /// so re-discovering the same model is an upsert, never a duplicate.
    public var identity: String {
        "\(kind.rawValue)|\(path)|\(repo ?? "")"
    }
}

/// What executes the model: the resolution engine's pick, how it was chosen,
/// the run-tier badge, and the alternatives the user may switch to.
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

/// One entry of a model's parameter schema. The UI renders controls from
/// these generically, so a model type the app has never seen still gets
/// correct controls.
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

/// One registered model — the single source of truth the whole platform
/// hangs off. Everything else (resolution, execution, UI) reads and writes
/// through this record.
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

    /// Content-derived ID: same source, same ID, forever — the property that
    /// makes discovery rescans idempotent.
    public static func stableID(for source: ModelSource) -> String {
        let digest = SHA256.hash(data: Data(source.identity.utf8))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }
}
