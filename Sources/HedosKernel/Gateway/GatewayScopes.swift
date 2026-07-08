import Foundation

public struct GatewayScopes: Codable, Sendable, Hashable {
    public var models: [String]?
    public var capabilities: [String]?

    public init(models: [String]? = nil, capabilities: [String]? = nil) {
        self.models = models
        self.capabilities = capabilities
    }

    public static let all = GatewayScopes()

    public func permits(modelID: String, capability: Capability) -> Bool {
        permitsModel(modelID) && permitsCapability(capability)
    }

    public func permitsModel(_ modelID: String) -> Bool {
        guard let models else { return true }
        return models.contains(modelID)
    }

    public func permitsCapability(_ capability: Capability) -> Bool {
        guard let capabilities else { return true }
        return capabilities.contains(capability.rawValue)
    }

    public func filter(_ shelf: [ModelRecord]) -> [ModelRecord] {
        shelf.filter { permitsModel($0.id) }
    }
}
