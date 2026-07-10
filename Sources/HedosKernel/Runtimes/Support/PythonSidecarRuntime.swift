import Foundation

struct PythonSidecarRuntime: Sendable {
    struct Descriptor: Sendable {
        var runtimeID: String
        var preparingStatus: String
        var startingStatus: String
        var warmWindow: Duration?
        var prepareEnvironment:
            @Sendable (@escaping @Sendable (String) -> Void) async throws -> URL?
        var makeSpec: @Sendable (ModelRecord, URL?) throws -> SidecarSpec
    }

    let descriptor: Descriptor
    let governor: MemoryGovernor
    let supervisor: SidecarSupervisor

    init(
        descriptor: Descriptor,
        governor: MemoryGovernor = .shared,
        supervisor: SidecarSupervisor = .shared
    ) {
        self.descriptor = descriptor
        self.governor = governor
        self.supervisor = supervisor
    }

    func stream(
        _ record: ModelRecord, op: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        run(
            record, op: op.rawValue, payload: payload,
            producer: GPUProducer.generation(modelID: record.id),
            status: { .status($0) },
            open: { supervisor, spec, control in await supervisor.request(spec, control) })
    }

    func job(
        _ record: ModelRecord, op: String, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        run(
            record, op: op, payload: payload,
            producer: GPUProducer.job(modelID: record.id),
            status: { .status($0) },
            open: { supervisor, spec, control in await supervisor.jobRequest(spec, control) })
    }

    static func control(op: String, payload: JSONValue) -> JSONValue {
        var control: [String: JSONValue] = ["op": .string(op)]
        if case .object(let fields) = payload {
            for (key, value) in fields { control[key] = value }
        }
        return .object(control)
    }

    private func run<Element: Sendable>(
        _ record: ModelRecord, op: String, payload: JSONValue,
        producer: GPUProducer,
        status: @escaping @Sendable (String) -> Element,
        open: @escaping @Sendable (SidecarSupervisor, SidecarSpec, JSONValue) async ->
            AsyncThrowingStream<Element, Error>
    ) -> AsyncThrowingStream<Element, Error> {
        AsyncThrowingStream { continuation in
            let runtime = self
            let task = Task {
                do {
                    continuation.yield(status(runtime.descriptor.preparingStatus))
                    let envDir = try await runtime.descriptor.prepareEnvironment { message in
                        continuation.yield(status(message))
                    }
                    let spec = try runtime.descriptor.makeSpec(record, envDir)
                    await runtime.governor.beginGeneration(record.id)
                    do {
                        try await SidecarWarmLoad.acquire(
                            governor: runtime.governor, supervisor: runtime.supervisor,
                            spec: spec, record: record, producer: producer,
                            warmWindow: runtime.descriptor.warmWindow,
                            startingStatus: runtime.descriptor.startingStatus
                        ) { continuation.yield(status($0)) }
                        do {
                            let events = await open(
                                runtime.supervisor, spec,
                                Self.control(op: op, payload: payload))
                            for try await event in events {
                                continuation.yield(event)
                            }
                            await runtime.governor.gate.release(producer)
                        } catch {
                            await runtime.governor.gate.release(producer)
                            throw error
                        }
                        await runtime.governor.endGeneration(record.id)
                    } catch {
                        await runtime.governor.endGeneration(record.id)
                        throw error
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
