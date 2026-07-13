import Foundation

private enum SpeechDrain: Sendable {
    case audio(Data, Int)
    case timedOut
}

struct OpenAISpeechHandler: GatewayHandling {
    static let runTimeoutSeconds = 300

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
        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .speak, kind: .stream, port: port, identity: identity)

        var voice = body["voice"] as? String
        if voice == nil || voice!.isEmpty {
            voice = try await port.voices(for: record.id).first
        }
        var payload: [String: JSONValue] = ["text": .string(input)]
        if let voice { payload["voice"] = .string(voice) }
        if let speed = body["speed"] as? Double { payload["speed"] = .double(speed) }

        let stream = try await port.invoke(record.id, .speak, payload: .object(payload))
        let drained: SpeechDrain = try await withThrowingTaskGroup(of: SpeechDrain.self) {
            group in
            group.addTask {
                var pcm = Data()
                var sampleRate = SidecarSpec.defaultSampleRate
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        if pcm.isEmpty { sampleRate = frame.sampleRate }
                        pcm.append(frame.data)
                    }
                }
                return .audio(pcm, sampleRate)
            }
            group.addTask {
                try await Task.sleep(for: .seconds(Self.runTimeoutSeconds))
                return .timedOut
            }
            let first = try await group.next()!
            group.cancelAll()
            return first
        }
        guard case .audio(let pcm, let sampleRate) = drained else {
            throw GatewayError(
                .timeout, "speech timed out after \(Self.runTimeoutSeconds)s", code: "timeout")
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
