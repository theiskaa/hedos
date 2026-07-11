import Foundation

extension SidecarSupervisor {
    func pumpJob(
        _ spec: SidecarSpec,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        while let frame = await nextFrame(id, timeout: spec.frameTimeout) {
            guard case .control(let value) = frame else { continue }
            switch value.objectValue?["event"]?.stringValue {
            case "begin":
                continuation.yield(.started)
            case "step":
                let step = value.objectValue?["n"]?.intValue ?? 0
                let total = value.objectValue?["total"]?.intValue ?? 0
                continuation.yield(.progress(step: step, totalSteps: total))
            case "preview":
                if let data = await nextBinaryFrame(id, timeout: spec.frameTimeout) {
                    continuation.yield(.preview(data))
                }
            case "image":
                let format = value.objectValue?["format"]?.stringValue ?? "png"
                if let data = await nextBinaryFrame(id, timeout: spec.frameTimeout) {
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
        throw KernelError.sidecarDied(
            runtimeID: id,
            detail: "stopped unexpectedly: \(ManifestSupport.errorSummary(stderrTail(id)))")
    }

    private func nextBinaryFrame(_ id: String, timeout: Duration) async -> Data? {
        guard let frame = await nextFrame(id, timeout: timeout),
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

        while let frame = await nextFrame(id, timeout: spec.frameTimeout) {
            switch frame {
            case .audio(let data):
                continuation.yield(.audio(AudioFrame(data: data, sampleRate: sampleRate)))
            case .control(let value):
                switch value.objectValue?["event"]?.stringValue {
                case "begin":
                    continuation.yield(.status("generating"))
                case "text":
                    continuation.yield(.text(value.objectValue?["text"]?.stringValue ?? ""))
                case "thinking":
                    continuation.yield(.thinking(value.objectValue?["text"]?.stringValue ?? ""))
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
        throw KernelError.sidecarDied(
            runtimeID: id,
            detail: "stopped unexpectedly: \(ManifestSupport.errorSummary(stderrTail(id)))")
    }
}

extension Frame {
    func controlField(_ key: String) -> JSONValue? {
        guard case .control(let value) = self else { return nil }
        return value.objectValue?[key]
    }
}
