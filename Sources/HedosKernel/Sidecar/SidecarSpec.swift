import Foundation

public struct SidecarSpec: Sendable {
    public var runtimeID: String
    public var executable: URL
    public var arguments: [String]
    public var environment: [String: String]
    public var workingDirectory: URL?
    public var readyTimeout: Duration
    public var frameTimeout: Duration
    public var cooperativeCancel: Bool
    public var cancelGraceTimeout: Duration

    public static let defaultSampleRate = 24000

    public init(
        runtimeID: String,
        executable: URL,
        arguments: [String],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        readyTimeout: Duration = .seconds(180),
        frameTimeout: Duration = .seconds(600),
        cooperativeCancel: Bool = false,
        cancelGraceTimeout: Duration = .seconds(10)
    ) {
        self.runtimeID = runtimeID
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.readyTimeout = readyTimeout
        self.frameTimeout = frameTimeout
        self.cooperativeCancel = cooperativeCancel
        self.cancelGraceTimeout = cancelGraceTimeout
    }
}
