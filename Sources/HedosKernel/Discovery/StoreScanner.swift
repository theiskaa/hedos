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

    public init(
        name: String,
        source: ModelSource,
        modalityHint: Modality? = nil,
        capabilitiesHint: [Capability] = [],
        executionHint: ExecutionMode = .sync,
        footprintBytes: Int64 = 0,
        primaryWeightPath: String? = nil,
        diagnostics: [String] = []
    ) {
        self.name = name
        self.source = source
        self.modalityHint = modalityHint
        self.capabilitiesHint = capabilitiesHint
        self.executionHint = executionHint
        self.footprintBytes = footprintBytes
        self.primaryWeightPath = primaryWeightPath
        self.diagnostics = diagnostics
    }
}

public struct ScanResult: Sendable {
    public var discovered: [DiscoveredModel]
    public var issues: [String]

    public init(discovered: [DiscoveredModel] = [], issues: [String] = []) {
        self.discovered = discovered
        self.issues = issues
    }
}

public protocol StoreScanner: Sendable {
    var kinds: Set<SourceKind> { get }
    func scan() async -> ScanResult
}
