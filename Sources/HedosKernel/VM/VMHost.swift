import Foundation

public enum VMAssetState: Sendable, Hashable {
    case ready
    case absent(approxDownloadMB: Int)
}

public struct VMRunRequest: Sendable, Hashable {
    public var runtimeID: String
    public var image: String
    public var setup: [String]
    public var arguments: [String]
    public var modelPath: String?
    public var resourcesPath: String?
    public var workdir: String
    public var outputs: String

    public init(
        runtimeID: String, image: String, setup: [String] = [], arguments: [String] = [],
        modelPath: String? = nil, resourcesPath: String? = nil, workdir: String,
        outputs: String
    ) {
        self.runtimeID = runtimeID
        self.image = image
        self.setup = setup
        self.arguments = arguments
        self.modelPath = modelPath
        self.resourcesPath = resourcesPath
        self.workdir = workdir
        self.outputs = outputs
    }
}

public struct VMRunResult: Sendable, Hashable {
    public var exitCode: Int32
    public var stdout: String
    public var stderr: String

    public init(exitCode: Int32, stdout: String, stderr: String) {
        self.exitCode = exitCode
        self.stdout = stdout
        self.stderr = stderr
    }
}

public enum VMGuestPath {
    public static let model = "/model"
    public static let resources = "/resources"
    public static let workdir = "/workdir"
    public static let outputs = "/outputs"
}

public protocol VMHost: Sendable {
    func assetState() async -> VMAssetState
    func provisionAssets(onStatus: (@Sendable (String) async -> Void)?) async throws
    func environmentReady(_ request: VMRunRequest) async -> Bool
    func provisionEnvironment(
        _ request: VMRunRequest, onStatus: (@Sendable (String) async -> Void)?
    ) async throws
    func run(_ request: VMRunRequest) async throws -> VMRunResult
    func cancel(runtimeID: String) async
}
