import Foundation

public struct DiscoveredModel: Sendable, Hashable {
    public var name: String
    public var source: ModelSource
    public var modalityHint: Modality?
    public var capabilitiesHint: [Capability]
    public var executionHint: ExecutionMode
    public var footprintBytes: Int64
    public var primaryWeightPath: String?
    public var diagnostics: [String]
    public var contextLengthHint: Int?
    public var hasChatTemplateHint: Bool?
    public var stopTokensHint: [String]?
    public var downloading: Bool

    public init(
        name: String,
        source: ModelSource,
        modalityHint: Modality? = nil,
        capabilitiesHint: [Capability] = [],
        executionHint: ExecutionMode = .sync,
        footprintBytes: Int64 = 0,
        primaryWeightPath: String? = nil,
        diagnostics: [String] = [],
        contextLengthHint: Int? = nil,
        hasChatTemplateHint: Bool? = nil,
        stopTokensHint: [String]? = nil,
        downloading: Bool = false
    ) {
        self.name = name
        self.source = source
        self.modalityHint = modalityHint
        self.capabilitiesHint = capabilitiesHint
        self.executionHint = executionHint
        self.footprintBytes = footprintBytes
        self.primaryWeightPath = primaryWeightPath
        self.diagnostics = diagnostics
        self.contextLengthHint = contextLengthHint
        self.hasChatTemplateHint = hasChatTemplateHint
        self.stopTokensHint = stopTokensHint
        self.downloading = downloading
    }
}

public struct ScanResult: Sendable {
    public var discovered: [DiscoveredModel]
    public var issues: [String]
    public var failedKinds: Set<SourceKind>

    public init(
        discovered: [DiscoveredModel] = [], issues: [String] = [],
        failedKinds: Set<SourceKind> = []
    ) {
        self.discovered = discovered
        self.issues = issues
        self.failedKinds = failedKinds
    }
}

public protocol StoreScanner: Sendable {
    var kinds: Set<SourceKind> { get }
    func scan() async -> ScanResult
}
