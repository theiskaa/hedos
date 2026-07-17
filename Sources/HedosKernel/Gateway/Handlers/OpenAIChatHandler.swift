import Foundation

struct OpenAIChatHandler: GatewayHandling {
    static let defaultRunTimeoutSeconds = 600
    var runTimeoutSeconds = OpenAIChatHandler.defaultRunTimeoutSeconds

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        var chatRequest = try OpenAIWire.decodeChatRequest(request.decodedJSON())
        if chatRequest.toolChoice == .string("none") {
            chatRequest.tools = []
        }
        let record = try await GatewayModelResolver.resolveAuthorized(
            chatRequest.model, capability: .chat, kind: .stream, port: port, identity: identity)
        if !chatRequest.tools.isEmpty, try await !port.supportsTools(modelID: record.id) {
            throw GatewayError(
                .badRequest, "\(chatRequest.model) does not support tool calling")
        }
        try GatewayParamGuard.require(
            chatRequest.sampling,
            honoredBy: try await port.honoredParams(modelID: record.id, capability: .chat),
            runtime: record.runtime.id)

        let stream = try await port.invoke(
            record.id, .chat, payload: OpenAIWire.chatPayload(chatRequest))
        let completionID = "chatcmpl-\(UUID().uuidString.lowercased())"
        let created = Int(Date().timeIntervalSince1970)
        let servedModel = chatRequest.model

        let includeUsage = chatRequest.includeUsage
        if chatRequest.stream {
            let body = try await responder.beginStream(contentType: "text/event-stream")
            do {
                let timedOut = try await StreamTimeout.race(seconds: runTimeoutSeconds) {
                    var first = true
                    var finalStats: GenerationStats?
                    var toolCallCount = 0
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
                        case .toolCall(let call):
                            try await body.write(
                                OpenAIWire.sseFrame(
                                    OpenAIWire.chunkFrame(
                                        id: completionID, created: created, model: servedModel,
                                        toolCall: call, toolCallIndex: toolCallCount, role: first)))
                            first = false
                            toolCallCount += 1
                        case .done(let stats):
                            finalStats = stats
                        case .audio, .status, .vector, .segment:
                            break
                        }
                    }
                    let finishReason =
                        toolCallCount > 0 ? "tool_calls" : finalStats?.finishReason ?? "stop"
                    let finalFrame = OpenAIWire.chunkFrame(
                        id: completionID, created: created, model: servedModel,
                        finishReason: finishReason)
                    try await body.write(OpenAIWire.sseFrame(finalFrame))
                    if includeUsage {
                        try await body.write(
                            OpenAIWire.sseFrame([
                                "id": completionID,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": servedModel,
                                "choices": [],
                                "usage": OpenAIWire.usage(finalStats),
                            ]))
                    }
                    try await body.write(OpenAIWire.sseDone)
                    try await body.end()
                }
                if timedOut {
                    try await StreamFailure.timeout(
                        surface: .openAI, body: body, seconds: runTimeoutSeconds)
                }
            } catch {
                try await StreamFailure.write(error, surface: .openAI, body: body)
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
                case .thinking, .audio, .status, .vector, .segment:
                    break
                }
            }
            try await responder.respond(
                status: 200,
                body: WireJSON.serialize(
                    OpenAIWire.completion(
                        id: completionID, created: created, model: servedModel,
                        content: content, stats: finalStats, toolCalls: toolCalls)))
        }
        return .ok(model: record.id, capability: .chat)
    }
}
