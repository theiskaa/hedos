import Foundation

struct OpenAISpeechHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let model = body["model"] as? String, !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let input = body["input"] as? String, !input.isEmpty else {
            throw GatewayError(.badRequest, "input is required")
        }
        if let format = body["response_format"] as? String, format != "wav" {
            throw GatewayError(
                .badRequest, "only wav output is available — set response_format to wav")
        }
        let shelf = try await port.shelf()
        let record = try GatewayModelResolver.resolve(model, shelf: shelf)
        try identity.require(modelID: record.id, capability: .speak)
        try await GatewayBackpressure.require(port, record: record, kind: .stream)

        var voice = body["voice"] as? String
        if voice == nil || voice!.isEmpty {
            voice = try await port.voices(for: record.id).first
        }
        var payload: [String: JSONValue] = ["text": .string(input)]
        if let voice { payload["voice"] = .string(voice) }
        if let speed = body["speed"] as? Double { payload["speed"] = .double(speed) }

        let stream = try await port.invoke(record.id, .speak, payload: .object(payload))
        var pcm = Data()
        var sampleRate = 24000
        for try await chunk in stream {
            if case .audio(let frame) = chunk {
                if pcm.isEmpty { sampleRate = frame.sampleRate }
                pcm.append(frame.data)
            }
        }
        guard !pcm.isEmpty else {
            throw GatewayError(.serverError, "\(record.name) produced no audio")
        }
        try await responder.respond(
            status: 200, contentType: "audio/wav",
            body: SpeechAudio.wavData(fromFloat32: pcm, sampleRate: sampleRate))
        return .ok(model: record.id, capability: .speak)
    }
}
