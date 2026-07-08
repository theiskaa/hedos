import Foundation

public enum GatewaySurface: String, Sendable, Hashable {
    case openAI
    case ollama
}

public struct GatewayIdentity: Sendable, Hashable {
    public var clientID: String
    public var name: String
    public var scopes: GatewayScopes

    public init(clientID: String, name: String, scopes: GatewayScopes) {
        self.clientID = clientID
        self.name = name
        self.scopes = scopes
    }
}

public struct GatewayOutcome: Sendable, Hashable {
    public var status: Int
    public var outcome: String
    public var model: String?
    public var capability: String?

    public init(status: Int, outcome: String, model: String? = nil, capability: String? = nil) {
        self.status = status
        self.outcome = outcome
        self.model = model
        self.capability = capability
    }

    public static let ok = GatewayOutcome(status: 200, outcome: "ok")

    public static func ok(model: String?, capability: Capability?) -> GatewayOutcome {
        GatewayOutcome(
            status: 200, outcome: "ok", model: model, capability: capability?.rawValue)
    }
}

public protocol GatewayHandling: Sendable {
    var surface: GatewaySurface { get }
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome
}
