import Foundation

public enum PipelineRunnerFactory {
    public typealias ChatOverride = @Sendable (String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    >

    static func payload(_ base: [String: JSONValue], merging params: [String: JSONValue])
        -> JSONValue
    {
        var object = base
        for (key, value) in params where object[key] == nil {
            object[key] = value
        }
        return .object(object)
    }

    static func aggregatedAudioPCM(_ upstream: AsyncStream<PipelineToken>) async -> [Float] {
        var samples: [Float] = []
        for await token in upstream {
            if case .audioPCM(let pcm) = token {
                samples.append(contentsOf: pcm)
            }
        }
        return samples
    }

    static func aggregatedText(_ upstream: AsyncStream<PipelineToken>) async -> String {
        var text = ""
        for await token in upstream {
            if case .text(let delta) = token {
                text += delta
            }
        }
        return text
    }

    public static func transcribe(
        index: Int, modelID: String, params: [String: JSONValue], sampleRate: Int,
        backend: any PipelineBackend
    ) -> PipelineStageRunner {
        PipelineStageRunner(
            index: index, capability: .transcribe, input: .audio, output: .text
        ) { upstream, downstream, sink in
            sink(.status(index: index, "transcribing"))
            let samples = await aggregatedAudioPCM(upstream)
            let base64 = samples.withUnsafeBytes { Data($0) }.base64EncodedString()
            let payload = payload(
                ["pcm": .string(base64), "sampleRate": .int(sampleRate)], merging: params)
            var transcript = ""
            for try await chunk in try await backend.invoke(modelID, .transcribe, payload: payload)
            {
                switch chunk {
                case .text(let delta), .segment(let delta, _, _):
                    transcript += delta
                    downstream(.text(delta))
                default:
                    break
                }
            }
            sink(.transcript(index: index, transcript))
        }
    }

    public static func textToText(
        index: Int, modelID: String, capability: Capability, params: [String: JSONValue],
        backend: any PipelineBackend, chat: ChatOverride? = nil
    ) -> PipelineStageRunner {
        PipelineStageRunner(
            index: index, capability: capability, input: .text, output: .text
        ) { upstream, downstream, sink in
            let prompt = await aggregatedText(upstream)
            let stream: AsyncThrowingStream<CapabilityChunk, Error>
            if let chat {
                stream = try await chat(prompt)
            } else {
                let payload = payload(
                    [
                        "messages": .array([
                            .object(["role": .string("user"), "content": .string(prompt)])
                        ])
                    ], merging: params)
                stream = try await backend.invoke(modelID, capability, payload: payload)
            }
            for try await chunk in stream {
                if case .text(let delta) = chunk {
                    sink(.delta(index: index, delta))
                    downstream(.text(delta))
                }
            }
        }
    }

    public static func speak(
        index: Int, modelID: String, params: [String: JSONValue], voice: String?,
        backend: any PipelineBackend
    ) -> PipelineStageRunner {
        PipelineStageRunner(
            index: index, capability: .speak, input: .text, output: .audio
        ) { upstream, downstream, sink in
            sink(.status(index: index, "speaking"))
            var chunker = SentenceChunker()

            @Sendable func utter(_ sentence: String) async throws {
                var base: [String: JSONValue] = ["text": .string(sentence)]
                if let voice { base["voice"] = .string(voice) }
                let payload = payload(base, merging: params)
                for try await chunk in try await backend.invoke(modelID, .speak, payload: payload) {
                    if case .audio(let frame) = chunk {
                        downstream(.audioFrame(frame))
                    }
                }
            }

            for await token in upstream {
                guard case .text(let delta) = token else { continue }
                for sentence in chunker.consume(delta) {
                    try Task.checkCancellation()
                    try await utter(sentence)
                }
            }
            if let remainder = chunker.flush() {
                try await utter(remainder)
            }
        }
    }

    public static func image(
        index: Int, modelID: String, params: [String: JSONValue], backend: any PipelineBackend
    ) -> PipelineStageRunner {
        PipelineStageRunner(
            index: index, capability: .image, input: .text, output: .image
        ) { upstream, downstream, sink in
            let prompt = await aggregatedText(upstream)
            sink(.status(index: index, "generating"))
            let payload = payload(["prompt": .string(prompt)], merging: params)
            let jobID = try await backend.submit(modelID, .image, payload: payload)
            let events = await backend.jobEvents(id: jobID)
            let artifacts = try await withTaskCancellationHandler {
                var result: [String] = []
                for await event in events {
                    switch event {
                    case .done(let ids):
                        result = ids
                    case .failed(let message):
                        throw KernelError.runtimeFailed(message)
                    case .cancelled:
                        throw CancellationError()
                    default:
                        continue
                    }
                }
                return result
            } onCancel: {
                Task { await backend.cancel(jobID: jobID) }
            }
            for id in artifacts {
                downstream(.artifact(id))
            }
        }
    }
}
