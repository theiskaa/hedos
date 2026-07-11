import Foundation

struct OpenAIModelsHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let shelf = try await port.shelf()
        let visible = identity.scopes.filter(shelf.filter { $0.state == .ready })
        try await responder.respond(
            status: 200, body: WireJSON.serialize(OpenAIWire.modelsList(visible)))
        return .ok
    }
}
