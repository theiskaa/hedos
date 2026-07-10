import Foundation

public protocol WhisperBackend: Sendable {
    func load(path: String) async throws
    func unload() async
    func transcribe(samples: [Float]) -> AsyncThrowingStream<String, Error>
}

public struct MissingWhisperBackend: WhisperBackend {
    static let hint = "Transcription needs the whisper engine, which is not bundled yet."

    public init() {}

    public func load(path: String) async throws {
        throw KernelError.runtimeUnavailable(hint: Self.hint)
    }

    public func unload() async {}

    public func transcribe(samples: [Float]) -> AsyncThrowingStream<String, Error> {
        AsyncThrowingStream { continuation in
            continuation.finish(throwing: KernelError.runtimeUnavailable(hint: Self.hint))
        }
    }
}

public actor WhisperEngine {
    public static let shared = WhisperEngine(backend: SidecarWhisperBackend())

    public static let expectedSampleRate = 16000

    private let backend: any WhisperBackend
    private var loadedPath: String?
    private var loadedModelID: String?
    private let transcriptionSlot = GenerationSlot()

    public init(backend: any WhisperBackend) {
        self.backend = backend
    }

    public func run(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        samples: [Float],
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async {
        await governor.beginGeneration(modelID)
        do {
            let producer = GPUProducer.generation(modelID: modelID)
            try await acquireGateWithModelLoaded(
                producer: producer, path: path, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, governor: governor, continuation: continuation)
            await transcriptionSlot.acquire()
            do {
                try await transcribe(samples: samples, continuation: continuation)
                await transcriptionSlot.release()
                await governor.gate.release(producer)
            } catch {
                await transcriptionSlot.release()
                await governor.gate.release(producer)
                throw error
            }
            continuation.finish()
        } catch {
            continuation.finish(throwing: error)
        }
        await governor.endGeneration(modelID)
    }

    private func acquireGateWithModelLoaded(
        producer: GPUProducer,
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        try await GovernedEngineLoad.acquireLoaded(
            governor: governor, producer: producer,
            modelID: modelID, modelName: modelName, footprintMB: footprintMB,
            tightStatus: "Memory is tight — transcription may be slow",
            status: { continuation.yield(.status($0)) },
            isLoaded: { await self.loadedPath == path },
            previousModelID: { await self.loadedModelID },
            unloadPrevious: { await self.unloadBackend() },
            load: { try await self.loadBackend(path: path, modelID: modelID) },
            evict: { [weak self] in await self?.unloadIfLoaded(path: path) },
            observedFootprintMB: { Footprint.weightsMB(path: path) })
    }

    private func loadBackend(path: String, modelID: String) async throws {
        try await backend.load(path: path)
        loadedPath = path
        loadedModelID = modelID
    }

    private func transcribe(
        samples: [Float],
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        let clock = ContinuousClock()
        let started = clock.now
        var segments = 0
        for try await delta in backend.transcribe(samples: samples) {
            if Task.isCancelled { break }
            continuation.yield(.text(delta))
            segments += 1
        }
        let elapsed = clock.now - started
        continuation.yield(
            .done(
                GenerationStats(
                    completionTokens: segments,
                    durationMs: Int(elapsed.components.seconds) * 1000
                        + Int(elapsed.components.attoseconds / 1_000_000_000_000_000))))
    }

    public func unloadIfLoaded(path: String) async {
        guard loadedPath == path else { return }
        await unloadBackend()
    }

    private func unloadBackend() async {
        await backend.unload()
        loadedPath = nil
        loadedModelID = nil
    }
}
