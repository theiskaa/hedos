import Foundation

struct OpenAIChatHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let chatRequest = try OpenAIWire.decodeChatRequest(request.decodedJSON())
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(chatRequest.model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .chat)
        try await GatewayBackpressure.require(port, record: record, kind: .stream)

        let stream = try await port.invoke(
            record.id, .chat, payload: OpenAIWire.chatPayload(chatRequest))
        let completionID = "chatcmpl-\(UUID().uuidString.lowercased())"
        let created = Int(Date().timeIntervalSince1970)
        let servedModel = chatRequest.model

        if chatRequest.stream {
            let body = try await responder.beginStream(contentType: "text/event-stream")
            var first = true
            var finalStats: GenerationStats?
            for try await chunk in stream {
                switch chunk {
                case .text(let text):
                    try await body.write(
                        OpenAIWire.sseFrame(
                            OpenAIWire.chunkFrame(
                                id: completionID, created: created, model: servedModel,
                                content: text, role: first)))
                    first = false
                case .thinking(let thought):
                    try await body.write(
                        OpenAIWire.sseFrame(
                            OpenAIWire.chunkFrame(
                                id: completionID, created: created, model: servedModel,
                                reasoning: thought, role: first)))
                    first = false
                case .done(let stats):
                    finalStats = stats
                case .audio, .status, .vector:
                    break
                }
            }
            var finalFrame = OpenAIWire.chunkFrame(
                id: completionID, created: created, model: servedModel,
                finishReason: "stop")
            finalFrame["usage"] = OpenAIWire.usage(finalStats)
            try await body.write(OpenAIWire.sseFrame(finalFrame))
            try await body.write(OpenAIWire.sseDone)
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
                    OpenAIWire.completion(
                        id: completionID, created: created, model: servedModel,
                        content: content, stats: finalStats)))
        }
        return .ok(model: record.id, capability: .chat)
    }
}
