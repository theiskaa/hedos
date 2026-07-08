import Foundation

struct OllamaChatHandler: GatewayHandling {
    var surface: GatewaySurface { .ollama }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let chatRequest = try OllamaWire.decodeChatRequest(request.decodedJSON())
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(chatRequest.model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .chat)
        try await GatewayBackpressure.require(port, record: record, kind: .stream)

        let stream = try await port.invoke(
            record.id, .chat, payload: OllamaWire.chatPayload(chatRequest))
        let servedModel = chatRequest.model

        if chatRequest.stream {
            let body = try await responder.beginStream(contentType: "application/x-ndjson")
            var finalStats: GenerationStats?
            for try await chunk in stream {
                switch chunk {
                case .text(let text):
                    try await body.write(
                        OllamaWire.line(OllamaWire.delta(model: servedModel, content: text)))
                case .thinking(let thought):
                    try await body.write(
                        OllamaWire.line(OllamaWire.delta(model: servedModel, thinking: thought)))
                case .done(let stats):
                    finalStats = stats
                case .audio, .status, .vector:
                    break
                }
            }
            try await body.write(
                OllamaWire.line(OllamaWire.final(model: servedModel, stats: finalStats)))
            try await body.end()
        } else {
            var content = ""
            var finalStats: GenerationStats?
            for try await chunk in stream {
                switch chunk {
                case .text(let text):
                    content += text
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector:
                    break
                }
            }
            try await responder.respond(
                status: 200,
                body: OpenAIWire.serialize(
                    OllamaWire.final(model: servedModel, content: content, stats: finalStats)))
        }
        return .ok(model: record.id, capability: .chat)
    }
}
