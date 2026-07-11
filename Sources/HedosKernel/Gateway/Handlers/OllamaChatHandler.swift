import Foundation

struct OllamaChatHandler: GatewayHandling {
    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let chatRequest = try OllamaWire.decodeChatRequest(request.decodedJSON())
        let record = try await GatewayModelResolver.resolveAuthorized(
            chatRequest.model, capability: .chat, kind: .stream, port: port, identity: identity)
        if !chatRequest.tools.isEmpty, try await !port.supportsTools(modelID: record.id) {
            throw GatewayError(
                .badRequest, "\(chatRequest.model) does not support tool calling")
        }
        try GatewayParamGuard.require(
            chatRequest.options,
            honoredBy: try await port.honoredParams(modelID: record.id, capability: .chat),
            runtime: record.runtime.id)

        let stream = try await port.invoke(
            record.id, .chat, payload: OllamaWire.chatPayload(chatRequest))
        let servedModel = chatRequest.model

        if chatRequest.stream {
            let body = try await responder.beginStream(contentType: "application/x-ndjson")
            do {
                let timedOut = try await StreamTimeout.race(
                    seconds: OpenAIChatHandler.runTimeoutSeconds
                ) {
                    var finalStats: GenerationStats?
                    var sawToolCall = false
                    for try await chunk in stream {
                        switch chunk {
                        case .text(let text):
                            try await body.write(
                                OllamaWire.line(
                                    OllamaWire.delta(model: servedModel, content: text)))
                        case .thinking(let thought):
                            try await body.write(
                                OllamaWire.line(
                                    OllamaWire.delta(model: servedModel, thinking: thought)))
                        case .toolCall(let call):
                            sawToolCall = true
                            try await body.write(
                                OllamaWire.line(
                                    OllamaWire.delta(model: servedModel, toolCall: call)))
                        case .done(let stats):
                            finalStats = stats
                        case .audio, .status, .vector:
                            break
                        }
                    }
                    var stats = finalStats ?? GenerationStats()
                    if sawToolCall {
                        stats.finishReason = "stop"
                    }
                    try await body.write(
                        OllamaWire.line(OllamaWire.final(model: servedModel, stats: stats)))
                    try await body.end()
                }
                if timedOut {
                    try await StreamFailure.timeout(
                        surface: .ollama, body: body,
                        seconds: OpenAIChatHandler.runTimeoutSeconds)
                }
            } catch {
                try await StreamFailure.write(error, surface: .ollama, body: body)
                throw error
            }
        } else {
            var content = ""
            var finalStats: GenerationStats?
            var toolCalls: [ToolCall] = []
            for try await chunk in stream {
                switch chunk {
                case .text(let text):
                    content += text
                case .toolCall(let call):
                    toolCalls.append(call)
                case .done(let stats):
                    finalStats = stats
                case .thinking, .audio, .status, .vector:
                    break
                }
            }
            try await responder.respond(
                status: 200,
                body: WireJSON.serialize(
                    OllamaWire.final(
                        model: servedModel, content: content, stats: finalStats,
                        toolCalls: toolCalls)))
        }
        return .ok(model: record.id, capability: .chat)
    }
}
