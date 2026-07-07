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
    private var transcriptionSlotHeld = false
    private var transcriptionSlotWaiters: [CheckedContinuation<Void, Never>] = []

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
        await acquireTranscriptionSlot()
        do {
            let producer = GPUProducer.generation(modelID: modelID)
            try await acquireGateWithModelLoaded(
                producer: producer, path: path, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, governor: governor, continuation: continuation)
            do {
                try await transcribe(samples: samples, continuation: continuation)
                await governor.gate.release(producer)
            } catch {
                await governor.gate.release(producer)
                throw error
            }
            continuation.finish()
        } catch {
            continuation.finish(throwing: error)
        }
        releaseTranscriptionSlot()
        await governor.endGeneration(modelID)
    }

    private func acquireTranscriptionSlot() async {
        if !transcriptionSlotHeld {
            transcriptionSlotHeld = true
            return
        }
        await withCheckedContinuation { transcriptionSlotWaiters.append($0) }
    }

    private func releaseTranscriptionSlot() {
        if transcriptionSlotWaiters.isEmpty {
            transcriptionSlotHeld = false
        } else {
            transcriptionSlotWaiters.removeFirst().resume()
        }
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
        while true {
            try await ensureLoadedGoverned(
                path: path, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, governor: governor, continuation: continuation)
            await governor.gate.acquire(producer)
            if loadedPath == path { return }
            await governor.gate.release(producer)
        }
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

    private func ensureLoadedGoverned(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        if loadedPath == path { return }
        let verdict = try await governor.admit(
            modelID: modelID, name: modelName, footprintMB: footprintMB
        ) { reason in
            continuation.yield(.status(reason))
        }
        if verdict == .tight {
            continuation.yield(.status("Memory is tight — transcription may be slow"))
        }
        let producer = GPUProducer.load(modelID: modelID)
        await governor.gate.acquire(producer)
        do {
            if let previousModelID = loadedModelID {
                await unloadBackend()
                await governor.markUnloaded(previousModelID)
            }
            try await backend.load(path: path)
            loadedPath = path
            loadedModelID = modelID
            await governor.gate.release(producer)
        } catch {
            await governor.gate.release(producer)
            await governor.markUnloaded(modelID)
            throw error
        }
        await governor.markLoaded(
            modelID: modelID, name: modelName, footprintMB: footprintMB
        ) { [weak self] in
            await self?.unloadIfLoaded(path: path)
        }
        if let observed = LlamaEngine.weightsFootprintMB(path: path) {
            await governor.observeFootprint(modelID, footprintMB: observed)
        }
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
