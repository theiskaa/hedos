import Foundation

struct PipelineListHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let shelf = try await port.shelf()
        let pipelines = await port.pipelines()
        var entries: [[String: Any]] = []
        for pipeline in pipelines {
            let permitted = pipeline.stages.allSatisfy {
                identity.scopes.permits(modelID: $0.modelID, capability: $0.capability)
            }
            guard permitted,
                let signature = try? PipelineValidator.validate(pipeline.stages, shelf: shelf)
            else { continue }
            let stages = pipeline.stages.map { stage -> [String: Any] in
                let name = shelf.first { $0.id == stage.modelID }?.name ?? stage.modelID
                return ["model": name, "capability": stage.capability.rawValue]
            }
            entries.append([
                "id": pipeline.id,
                "name": pipeline.name,
                "input": signature.input.rawValue,
                "output": signature.output.rawValue,
                "stages": stages,
            ])
        }
        try await responder.respond(
            status: 200, body: OpenAIWire.serialize(["pipelines": entries]))
        return .ok
    }
}
