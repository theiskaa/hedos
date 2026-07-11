import Foundation

public enum GatewayDefaults {
    public static let port = 43367
    public static let maxConnections = 128
    public static let maxBodyBytes = 2_097_152
    static let inferenceQueueDepthCap = 4
    static let saturatedRetryAfterSeconds = 1
    static let queuedRetryAfterSeconds = 5
    static let pipelineRunTimeout: Duration = .seconds(300)
}
