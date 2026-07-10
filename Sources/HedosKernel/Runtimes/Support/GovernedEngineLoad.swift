import Foundation

enum GovernedEngineLoad {
    static func acquireLoaded(
        governor: MemoryGovernor,
        producer: GPUProducer,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        tightStatus: String,
        status: @escaping @Sendable (String) -> Void,
        isLoaded: @Sendable () async -> Bool,
        previousModelID: @Sendable () async -> String?,
        unloadPrevious: @Sendable () async -> Void,
        load: @Sendable () async throws -> Void,
        evict: @escaping @Sendable () async -> Void,
        observedFootprintMB: @Sendable () -> Int?
    ) async throws {
        while true {
            try await ensureLoaded(
                governor: governor, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, tightStatus: tightStatus, status: status,
                isLoaded: isLoaded, previousModelID: previousModelID,
                unloadPrevious: unloadPrevious, load: load, evict: evict,
                observedFootprintMB: observedFootprintMB)
            await governor.gate.acquire(producer)
            if await isLoaded() { return }
            await governor.gate.release(producer)
        }
    }

    private static func ensureLoaded(
        governor: MemoryGovernor,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        tightStatus: String,
        status: @escaping @Sendable (String) -> Void,
        isLoaded: @Sendable () async -> Bool,
        previousModelID: @Sendable () async -> String?,
        unloadPrevious: @Sendable () async -> Void,
        load: @Sendable () async throws -> Void,
        evict: @escaping @Sendable () async -> Void,
        observedFootprintMB: @Sendable () -> Int?
    ) async throws {
        if await isLoaded() { return }
        let verdict = try await governor.admit(
            modelID: modelID, name: modelName, footprintMB: footprintMB
        ) { reason in
            status(reason)
        }
        if verdict == .tight {
            status(tightStatus)
        }
        let loadProducer = GPUProducer.load(modelID: modelID)
        await governor.gate.acquire(loadProducer)
        do {
            if let previous = await previousModelID() {
                await unloadPrevious()
                await governor.markUnloaded(previous)
            }
            try await load()
            await governor.gate.release(loadProducer)
        } catch {
            await governor.gate.release(loadProducer)
            await governor.markUnloaded(modelID)
            throw error
        }
        await governor.markLoaded(
            modelID: modelID, name: modelName, footprintMB: footprintMB, unloader: evict)
        if let observed = observedFootprintMB() {
            await governor.observeFootprint(modelID, footprintMB: observed)
        }
    }
}
