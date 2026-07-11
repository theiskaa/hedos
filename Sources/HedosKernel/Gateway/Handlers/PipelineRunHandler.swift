import Foundation

struct PipelineRunHandler: GatewayHandling {
    var runTimeout: Duration = GatewayDefaults.pipelineRunTimeout

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let body = try request.decodedJSON()
        guard let pipelineID = body["pipeline"] as? String, !pipelineID.isEmpty else {
            throw GatewayError(.badRequest, "pipeline id is required")
        }
        guard let pipeline = await port.pipeline(id: pipelineID) else {
            throw GatewayError(.notFound, "no pipeline with id \(pipelineID)")
        }
        for stage in pipeline.stages {
            try identity.require(modelID: stage.modelID, capability: stage.capability)
        }
        let shelf = try await port.shelf()
        let signature = try validate(pipeline, shelf: shelf)

        for stage in pipeline.stages {
            guard let record = shelf.first(where: { $0.id == stage.modelID }) else { continue }
            let kind: GatewayWorkKind =
                CapabilitySignatures.signature(stage.capability)?.mode == .job ? .job : .stream
            try await GatewayBackpressure.require(port, record: record, kind: kind)
        }

        let input = try pipelineInput(from: body, head: signature.input)
        let stream = try await port.runPipeline(id: pipelineID, input: input)

        switch signature.output {
        case .audio:
            try await renderAudio(stream, responder: responder)
        case .image:
            try await renderImage(stream, port: port, responder: responder)
        case .vector:
            try await renderVectors(stream, responder: responder)
        default:
            try await renderText(stream, model: pipeline.id, responder: responder)
        }
        return GatewayOutcome(status: 200, outcome: "ok", model: pipeline.id)
    }

    private func validate(_ pipeline: Pipeline, shelf: [ModelRecord]) throws -> PipelineSignature {
        do {
            return try PipelineValidator.validate(pipeline.stages, shelf: shelf)
        } catch let error as PipelineValidationError {
            throw GatewayError(.badRequest, error.description)
        }
    }

    private func pipelineInput(
        from body: [String: Any], head: PipelinePort
    ) throws -> PipelineInput {
        guard let input = body["input"] as? [String: Any] else {
            throw GatewayError(.badRequest, "input is required")
        }
        switch head {
        case .text:
            guard let text = input["text"] as? String else {
                throw GatewayError(.badRequest, "this pipeline expects an input.text string")
            }
            return .text(text)
        case .audio:
            guard let base64 = input["audio"] as? String,
                let data = Data(base64Encoded: base64)
            else {
                throw GatewayError(
                    .badRequest, "this pipeline expects base64 float32 pcm in input.audio")
            }
            let samples = data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
            return .audio(samples)
        case .image, .vector:
            throw GatewayError(.badRequest, "unsupported pipeline input type")
        }
    }

    private func withRunTimeout<Result: Sendable>(
        _ body: @escaping @Sendable () async throws -> Result,
        onTimeout: @escaping @Sendable () throws -> Result
    ) async throws -> Result {
        let timeout = runTimeout
        return try await withThrowingTaskGroup(of: Result.self) { group in
            group.addTask { try await body() }
            group.addTask {
                try await Task.sleep(for: timeout)
                return try onTimeout()
            }
            let result = try await group.next()!
            group.cancelAll()
            return result
        }
    }

    private enum TextDrain { case done(failure: String?), timedOut }

    private func timeoutMessage() -> String {
        "the pipeline run timed out after \(Int(runTimeout.components.seconds))s"
    }

    private func renderText(
        _ stream: AsyncStream<PipelineEvent>, model: String, responder: GatewayResponder
    ) async throws {
        let body = try await responder.beginStream(contentType: "text/event-stream")
        let id = "pipe-\(UUID().uuidString.lowercased())"
        let created = Int(Date().timeIntervalSince1970)

        let drain: TextDrain = try await withRunTimeout {
            var first = true
            var failure: String?
            for await event in stream {
                switch event {
                case .delta(_, let text):
                    try await body.write(
                        OpenAIWire.sseFrame(
                            OpenAIWire.chunkFrame(
                                id: id, created: created, model: model, content: text,
                                role: first)))
                    first = false
                case .failed(let message):
                    failure = message
                default:
                    break
                }
            }
            return .done(failure: failure)
        } onTimeout: {
            .timedOut
        }

        switch drain {
        case .done(let failure):
            if let failure {
                try await body.write(
                    OpenAIWire.sseFrame([
                        "error": ["message": failure, "type": "api_error"]
                    ]))
            } else {
                var finalFrame = OpenAIWire.chunkFrame(
                    id: id, created: created, model: model, finishReason: "stop")
                finalFrame["usage"] = OpenAIWire.usage(nil)
                try await body.write(OpenAIWire.sseFrame(finalFrame))
            }
        case .timedOut:
            try await body.write(
                OpenAIWire.sseFrame([
                    "error": ["message": timeoutMessage(), "type": "timeout_error"]
                ]))
        }
        try await body.write(OpenAIWire.sseDone)
        try await body.end()
    }

    private func renderAudio(
        _ stream: AsyncStream<PipelineEvent>, responder: GatewayResponder
    ) async throws {
        typealias Collected = (pcm: Data, sampleRate: Int, failure: String?)
        let collected: Collected = try await withRunTimeout {
            var pcm = Data()
            var sampleRate = 24000
            var failure: String?
            for await event in stream {
                switch event {
                case .audio(let frame):
                    if pcm.isEmpty { sampleRate = frame.sampleRate }
                    pcm.append(frame.data)
                case .failed(let message):
                    failure = message
                default:
                    break
                }
            }
            return (pcm, sampleRate, failure)
        } onTimeout: {
            throw GatewayError(.timeout, timeoutMessage())
        }
        if let failure = collected.failure { throw GatewayError(.serverError, failure) }
        guard !collected.pcm.isEmpty else {
            throw GatewayError(.serverError, "the pipeline produced no audio")
        }
        try await responder.respond(
            status: 200, contentType: "audio/wav",
            body: SpeechAudio.wavData(fromFloat32: collected.pcm, sampleRate: collected.sampleRate))
    }

    private func renderImage(
        _ stream: AsyncStream<PipelineEvent>, port: any GatewayPort, responder: GatewayResponder
    ) async throws {
        typealias Collected = (artifactID: String?, failure: String?)
        let collected: Collected = try await withRunTimeout {
            var artifactID: String?
            var failure: String?
            for await event in stream {
                switch event {
                case .artifact(let id):
                    artifactID = id
                case .failed(let message):
                    failure = message
                default:
                    break
                }
            }
            return (artifactID, failure)
        } onTimeout: {
            throw GatewayError(.timeout, timeoutMessage())
        }
        if let failure = collected.failure { throw GatewayError(.serverError, failure) }
        guard let artifactID = collected.artifactID, let data = try await port.artifactData(id: artifactID)
        else {
            throw GatewayError(.serverError, "the pipeline produced no image")
        }
        try await responder.respond(
            status: 200,
            body: WireJSON.serialize([
                "created": Int(Date().timeIntervalSince1970),
                "data": [["b64_json": data.base64EncodedString()]],
            ]))
    }

    private func renderVectors(
        _ stream: AsyncStream<PipelineEvent>, responder: GatewayResponder
    ) async throws {
        typealias Collected = (vectors: [[Double]], failure: String?)
        let collected: Collected = try await withRunTimeout {
            var vectors: [[Double]] = []
            var failure: String?
            for await event in stream {
                switch event {
                case .vector(let values):
                    vectors.append(values)
                case .failed(let message):
                    failure = message
                default:
                    break
                }
            }
            return (vectors, failure)
        } onTimeout: {
            throw GatewayError(.timeout, timeoutMessage())
        }
        if let failure = collected.failure { throw GatewayError(.serverError, failure) }
        guard !collected.vectors.isEmpty else {
            throw GatewayError(.serverError, "the pipeline produced no embeddings")
        }
        let data = collected.vectors.enumerated().map { index, vector -> [String: Any] in
            ["object": "embedding", "embedding": vector, "index": index]
        }
        try await responder.respond(
            status: 200,
            body: WireJSON.serialize([
                "object": "list",
                "data": data,
            ]))
    }
}
