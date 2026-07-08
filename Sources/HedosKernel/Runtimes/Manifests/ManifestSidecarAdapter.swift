import Foundation

public struct ManifestSidecarAdapter: RuntimeAdapter, JobRunning, ManifestBacked {
    public let manifest: RuntimeManifest
    public let approvedNetwork: Bool

    private let governor: MemoryGovernor
    private let supervisor: SidecarSupervisor

    public var id: String { manifest.id }

    public init(
        manifest: RuntimeManifest, approvedNetwork: Bool,
        governor: MemoryGovernor = .shared, supervisor: SidecarSupervisor = .shared
    ) {
        self.manifest = manifest
        self.approvedNetwork = approvedNetwork
        self.governor = governor
        self.supervisor = supervisor
    }

    private var networkBlocked: Bool {
        manifest.permissions.network && !approvedNetwork
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && manifest.capabilities.contains(capability)
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard let detect = manifest.detect, detect.matches(record), !networkBlocked else {
            return nil
        }
        return RuntimeBid(tier: .managed, preference: 100, alternatives: manifest.alternatives)
    }

    func spec(record: ModelRecord, envDir: URL?) throws -> SidecarSpec {
        guard let serve = manifest.serve, let directory = manifest.directory else {
            throw KernelError.runtimeFailed("\(id) declares no [serve] entrypoint")
        }
        guard
            let profile = ManifestSupport.profileURL(
                network: manifest.permissions.network && approvedNetwork)
        else {
            throw KernelError.runtimeFailed("generic sandbox profile missing")
        }
        let workdir = try ManifestSupport.workdir(for: manifest)
        let paths = SidecarModelPaths.resolve(record)
        let python =
            envDir?.appendingPathComponent("bin/python").path ?? "/usr/bin/python3"
        return SidecarSpec(
            runtimeID: "\(id)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: ManifestSupport.sandboxArguments(
                profile: profile, envDir: envDir, manifest: manifest, record: record,
                workdir: workdir) + [
                    python,
                    directory.appendingPathComponent(serve.entrypoint).path,
                    "--model", paths.snapshot,
                    "--workdir", workdir.path,
                ],
            environment: ["PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": ""],
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            cooperativeCancel: manifest.execution == .stream)
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let governor = governor
            let task = Task {
                await governor.beginGeneration(record.id)
                do {
                    guard !adapter.networkBlocked else {
                        throw KernelError.runtimeUnavailable(
                            hint:
                                "\(adapter.id) needs network permission. Approve it from the model's page."
                        )
                    }
                    continuation.yield(.status("Preparing \(adapter.id)…"))
                    let envDir = try await ManifestSupport.prepareEnvironmentIfNeeded(
                        manifest: adapter.manifest
                    ) { continuation.yield(.status($0)) }
                    let spec = try adapter.spec(record: record, envDir: envDir)
                    var control: [String: JSONValue] = ["op": .string(capability.rawValue)]
                    if case .object(let fields) = payload {
                        for (key, value) in fields { control[key] = value }
                    }
                    let producer = GPUProducer.generation(modelID: record.id)
                    try await adapter.acquireRunning(record, spec: spec, producer: producer) {
                        continuation.yield(.status($0))
                    }
                    do {
                        let stream = await adapter.supervisor.request(spec, .object(control))
                        for try await chunk in stream {
                            continuation.yield(chunk)
                        }
                        await governor.gate.release(producer)
                    } catch {
                        await governor.gate.release(producer)
                        throw error
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
                await governor.endGeneration(record.id)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    public func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let governor = governor
            let task = Task {
                await governor.beginGeneration(record.id)
                do {
                    guard !adapter.networkBlocked else {
                        throw KernelError.runtimeUnavailable(
                            hint:
                                "\(adapter.id) needs network permission. Approve it from the model's page."
                        )
                    }
                    continuation.yield(.status("Preparing \(adapter.id)…"))
                    let envDir = try await ManifestSupport.prepareEnvironmentIfNeeded(
                        manifest: adapter.manifest
                    ) { continuation.yield(.status($0)) }
                    let spec = try adapter.spec(record: record, envDir: envDir)
                    var control: [String: JSONValue] = ["op": .string(capability.rawValue)]
                    if case .object(let fields) = payload {
                        for (key, value) in fields { control[key] = value }
                    }
                    let producer = GPUProducer.job(modelID: record.id)
                    try await adapter.acquireRunning(record, spec: spec, producer: producer) {
                        continuation.yield(.status($0))
                    }
                    do {
                        let stream = await adapter.supervisor.jobRequest(spec, .object(control))
                        for try await event in stream {
                            continuation.yield(event)
                        }
                        await governor.gate.release(producer)
                    } catch {
                        await governor.gate.release(producer)
                        throw error
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
                await governor.endGeneration(record.id)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func acquireRunning(
        _ record: ModelRecord, spec: SidecarSpec, producer: GPUProducer,
        status: @escaping @Sendable (String) -> Void
    ) async throws {
        let supervisor = supervisor
        let governor = governor
        while true {
            if await !supervisor.isRunning(spec.runtimeID) {
                let verdict = try await governor.admit(
                    modelID: record.id, name: record.name,
                    footprintMB: record.footprintMB
                ) { reason in status(reason) }
                if verdict == .tight {
                    status("Memory is tight — loading anyway")
                }
                status("Starting \(id)…")
                let loadProducer = GPUProducer.load(modelID: record.id)
                await governor.gate.acquire(loadProducer)
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
                    footprintMB: record.footprintMB
                ) {
                    await supervisor.shutdown(spec.runtimeID)
                }
            }
            await governor.gate.acquire(producer)
            if await supervisor.isRunning(spec.runtimeID) { return }
            await governor.gate.release(producer)
        }
    }
}
