import Foundation

enum GovernedOneShot {
    static func run<Result: Sendable>(
        governor: MemoryGovernor,
        record: ModelRecord,
        producer: GPUProducer,
        status: @escaping @Sendable (String) -> Void,
        body: @Sendable () async throws -> Result
    ) async throws -> Result {
        await governor.beginGeneration(record.id)
        do {
            _ = try await governor.admit(
                modelID: record.id, name: record.name, footprintMB: record.footprintMB
            ) { reason in
                status(reason)
            }
        } catch {
            await governor.endGeneration(record.id)
            throw error
        }
        do {
            try await governor.gate.acquire(producer)
        } catch {
            await governor.endGeneration(record.id)
            throw error
        }
        do {
            let result = try await body()
            await governor.gate.release(producer)
            await governor.endGeneration(record.id)
            return result
        } catch {
            await governor.gate.release(producer)
            await governor.endGeneration(record.id)
            throw error
        }
    }
}
