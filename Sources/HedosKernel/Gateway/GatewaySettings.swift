import Foundation

public struct GatewaySettings: SettingsDomain {
    public static let domainName = "gateway"

    public var enabled: Bool
    public var port: Int
    public var maxConnections: Int
    public var maxConcurrentInference: Int

    public init() {
        enabled = false
        port = GatewayDefaults.port
        maxConnections = GatewayDefaults.maxConnections
        maxConcurrentInference = 4
    }

    public init(
        enabled: Bool, port: Int = GatewayDefaults.port, maxConnections: Int = GatewayDefaults.maxConnections,
        maxConcurrentInference: Int = 4
    ) {
        self.enabled = enabled
        self.port = port
        self.maxConnections = maxConnections
        self.maxConcurrentInference = maxConcurrentInference
    }

    enum CodingKeys: String, CodingKey {
        case enabled, port, maxConnections, maxConcurrentInference
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        enabled = container.lenient(Bool.self, .enabled, fallback: defaults.enabled)
        port = container.lenient(Int.self, .port, fallback: defaults.port)
        maxConnections = container.lenient(
            Int.self, .maxConnections, fallback: defaults.maxConnections)
        maxConcurrentInference = container.lenient(
            Int.self, .maxConcurrentInference, fallback: defaults.maxConcurrentInference)
    }
}
