import Foundation

public struct GatewayAuth: Sendable {
    let clients: GatewayClientStore

    public init(clients: GatewayClientStore) {
        self.clients = clients
    }

    public func authenticate(_ request: GatewayRequest) async throws -> GatewayIdentity {
        guard let token = request.bearerToken, !token.isEmpty else {
            throw GatewayError(.unauthorized, "a client token is required")
        }
        guard let identity = await clients.verify(token: token) else {
            throw GatewayError(.unauthorized, "unknown or revoked client token")
        }
        return identity
    }
}

extension GatewayIdentity {
    public func require(modelID: String, capability: Capability) throws {
        guard scopes.permits(modelID: modelID, capability: capability) else {
            throw GatewayError(
                .forbidden, "this token is not scoped for \(capability.rawValue) on that model")
        }
    }
}
