import Foundation

struct WhisperCppAdapter: RuntimeAdapter {
    var id: RuntimeID { .whisperCpp }

    private let governor: MemoryGovernor
    private let engine: WhisperEngine

    init(governor: MemoryGovernor = .shared, engine: WhisperEngine = .shared) {
        self.governor = governor
        self.engine = engine
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        capability == .transcribe && record.runtime.id == id
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .gguf || identified.format == .ggmlBin,
            identified.capabilities.contains(.transcribe)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.whisperCpp)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let path = record.primaryWeightPath ?? record.source.path
            let expanded = (path as NSString).expandingTildeInPath
            let governor = governor
            let engine = engine
            let task = Task {
                do {
                    let audio = try TranscriptionAudio.from(payload: payload)
                    let samples = audio.monoSamples(
                        targetSampleRate: WhisperEngine.expectedSampleRate)
                    guard !samples.isEmpty else {
                        throw KernelError.runtimeFailed("transcribe payload carries no audio")
                    }
                    await engine.run(
                        path: expanded,
                        modelID: record.id,
                        modelName: record.name,
                        footprintMB: record.footprintMB,
                        governor: governor,
                        samples: samples,
                        continuation: continuation)
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
