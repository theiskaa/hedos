import Foundation

struct OpenAITranscriptionsHandler: GatewayHandling {
    static let unsupportedFields = ["language", "prompt", "temperature", "timestamp_granularities"]

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        guard let boundary = GatewayMultipart.boundary(from: request.header("Content-Type"))
        else {
            throw GatewayError(.badRequest, "transcriptions require multipart/form-data")
        }
        let parts = GatewayMultipart.parse(request.body, boundary: boundary)
        func field(_ name: String) -> GatewayMultipart.Part? { parts.first { $0.name == name } }
        func text(_ name: String) -> String? {
            field(name).flatMap { String(data: $0.data, encoding: .utf8) }
        }

        guard let model = text("model"), !model.isEmpty else {
            throw GatewayError(.badRequest, "model is required")
        }
        guard let filePart = field("file") else {
            throw GatewayError(.badRequest, "file is required")
        }
        let responseFormat = text("response_format") ?? "json"
        guard responseFormat == "json" || responseFormat == "text" else {
            throw GatewayError(
                .badRequest, "response_format '\(responseFormat)' is not supported",
                code: "unsupported_parameter")
        }
        for unsupported in Self.unsupportedFields where field(unsupported) != nil {
            throw GatewayError(
                .badRequest, "the parameter '\(unsupported)' is not supported yet",
                code: "unsupported_parameter")
        }

        let record = try await GatewayModelResolver.resolveAuthorized(
            model, capability: .transcribe, kind: .stream, port: port, identity: identity)
        let audio = try TranscriptionAudio.fromWAVData(filePart.data)
        let pcm = audio.samples.withUnsafeBytes { Data($0) }.base64EncodedString()
        let payload: JSONValue = .object([
            "pcm": .string(pcm), "sampleRate": .int(audio.sampleRate),
        ])

        let stream = try await port.invoke(record.id, .transcribe, payload: payload)
        var transcript = ""
        for try await chunk in stream {
            if case .text(let value) = chunk { transcript += value }
        }

        if responseFormat == "text" {
            try await responder.respond(
                status: 200, contentType: "text/plain; charset=utf-8",
                body: Data(transcript.utf8))
        } else {
            try await responder.respond(status: 200, body: WireJSON.serialize(["text": transcript]))
        }
        return .ok(model: record.id, capability: .transcribe)
    }
}
