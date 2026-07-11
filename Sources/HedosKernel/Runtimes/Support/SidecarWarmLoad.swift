import Foundation

enum SidecarWarmLoad {
    static func acquire(
        governor: MemoryGovernor,
        supervisor: SidecarSupervisor,
        spec: SidecarSpec,
        record: ModelRecord,
        producer: GPUProducer,
        warmWindow: Duration? = nil,
        startingStatus: String,
        status: @escaping @Sendable (String) -> Void
    ) async throws {
        while true {
            if await !supervisor.isRunning(spec.runtimeID) {
                let verdict = try await governor.admit(
                    modelID: record.id, name: record.name,
                    footprintMB: record.footprintMB
                ) { reason in
                    status(reason)
                }
                if verdict == .tight {
                    status("Memory is tight — loading anyway")
                }
                status(startingStatus)
                let loadProducer = GPUProducer.load(modelID: record.id)
                do {
                    try await governor.gate.acquire(loadProducer)
                } catch {
                    await governor.markUnloaded(record.id)
                    throw error
                }
                do {
                    try await supervisor.ensureRunning(spec)
                    await governor.gate.release(loadProducer)
                } catch {
                    await governor.gate.release(loadProducer)
                    await governor.markUnloaded(record.id)
                    throw error
                }
                await governor.markLoaded(
                    modelID: record.id, name: record.name,
                    footprintMB: record.footprintMB,
                    warmWindow: warmWindow
                ) {
                    await supervisor.shutdown(spec.runtimeID)
                }
            }
            try await governor.gate.acquire(producer)
            if await supervisor.isRunning(spec.runtimeID) { return }
            await governor.gate.release(producer)
        }
    }
}
