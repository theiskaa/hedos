import Foundation

extension SidecarSupervisor {
    func frameLoopExitError(_ spec: SidecarSpec) -> Error {
        let id = spec.runtimeID
        if let sidecar = sidecars[id], !sidecar.eof {
            kill(id)
            return KernelError.runtimeFailed(
                "the runtime made no progress for \(spec.frameTimeout) and was stopped")
        }
        return KernelError.sidecarDied(
            runtimeID: id,
            detail: "stopped unexpectedly: \(ManifestSupport.errorSummary(stderrTail(id)))")
    }

    func pumpJob(
        _ spec: SidecarSpec,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        while let frame = await nextFrame(id) {
            guard case .control(let value) = frame else { continue }
            switch value.objectValue?["event"]?.stringValue {
            case "begin":
                continuation.yield(.started)
            case "step":
                let step = value.objectValue?["n"]?.intValue ?? 0
                let total = value.objectValue?["total"]?.intValue ?? 0
                continuation.yield(.progress(step: step, totalSteps: total))
            case "preview":
                if let data = await nextBinaryFrame(id) {
                    continuation.yield(.preview(data))
                }
            case "image":
                let format = value.objectValue?["format"]?.stringValue ?? "png"
                if let data = await nextBinaryFrame(id) {
                    continuation.yield(.result(data: data, fileExtension: format))
                }
            case "done":
                return
            case "cancelled":
                throw CancellationError()
            case "error":
                throw KernelError.runtimeFailed(
                    value.objectValue?["message"]?.stringValue ?? "sidecar error")
            default:
                continue
            }
        }
        throw frameLoopExitError(spec)
    }

    private func nextBinaryFrame(_ id: String) async -> Data? {
        guard let frame = await nextFrame(id),
            case .audio(let data) = frame
        else { return nil }
        return data
    }

    func pump(
        _ spec: SidecarSpec,
        into continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        let sampleRate = sidecars[id]?.sampleRate ?? SidecarSpec.defaultSampleRate

        while let frame = await nextFrame(id) {
            switch frame {
            case .audio(let data):
                continuation.yield(.audio(AudioFrame(data: data, sampleRate: sampleRate)))
            case .control(let value):
                switch value.objectValue?["event"]?.stringValue {
                case "begin":
                    continuation.yield(.status("generating"))
                case "text":
                    let text = value.objectValue?["text"]?.stringValue ?? ""
                    if let startMs = value.objectValue?["t0_ms"]?.intValue,
                        let endMs = value.objectValue?["t1_ms"]?.intValue
                    {
                        continuation.yield(.segment(text, startMs: startMs, endMs: endMs))
                    } else {
                        continuation.yield(.text(text))
                    }
                case "thinking":
                    continuation.yield(.thinking(value.objectValue?["text"]?.stringValue ?? ""))
                case "vector":
                    var values: [Double] = []
                    if case .array(let entries)? = value.objectValue?["values"] {
                        values = entries.compactMap { $0.doubleValue }
                    }
                    continuation.yield(.vector(values))
                case "status":
                    continuation.yield(.status(value.objectValue?["message"]?.stringValue ?? ""))
                case "done":
                    let seconds = value.objectValue?["seconds"]?.doubleValue
                    continuation.yield(
                        .done(
                            GenerationStats(
                                promptTokens: value.objectValue?["prompt_tokens"]?.intValue,
                                completionTokens: value.objectValue?["completion_tokens"]?
                                    .intValue,
                                durationMs: seconds.map { Int($0 * 1000) })))
                    return
                case "cancelled":
                    throw CancellationError()
                case "error":
                    throw KernelError.runtimeFailed(
                        value.objectValue?["message"]?.stringValue ?? "sidecar error")
                default:
                    continue
                }
            }
        }
        throw frameLoopExitError(spec)
    }
}

extension Frame {
    func controlField(_ key: String) -> JSONValue? {
        guard case .control(let value) = self else { return nil }
        return value.objectValue?[key]
    }
}
