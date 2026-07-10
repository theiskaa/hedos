import Foundation

struct MfluxAdapter: RuntimeAdapter, JobRunning {
    var id: RuntimeID { .mflux }

    static let servedPipelineClasses: Set<String> = ["FluxPipeline"]

    private let governor: MemoryGovernor
    private let supervisor: SidecarSupervisor

    init(governor: MemoryGovernor = .shared, supervisor: SidecarSupervisor = .shared) {
        self.governor = governor
        self.supervisor = supervisor
    }

    static func bundleDirectory() -> URL? {
        RuntimeBundle.directory(named: "python-mflux")
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .diffusers,
            identified.modality == .image,
            identified.capabilities.contains(.image),
            let pipelineClass = identified.pipelineClass,
            Self.servedPipelineClasses.contains(pipelineClass)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.mflux, alternatives: [.diffusers])
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(
                throwing: KernelError.wrongExecutionMode(runtimeID: .mflux, expected: .job))
        }
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let runtimeID = id.rawValue
            let task = Task {
                do {
                    guard let bundle = Self.bundleDirectory(),
                        FileManager.default.fileExists(atPath: bundle.path)
                    else {
                        throw KernelError.bundleMissing(runtimeID: .mflux)
                    }
                    continuation.yield(.status("Preparing image runtime…"))
                    let envDir = try await EnvironmentManager.shared.prepare(
                        runtimeID: runtimeID,
                        lockfile: bundle.appendingPathComponent("requirements.lock"),
                        progress: { message in continuation.yield(.status(message)) })

                    let spec = try Self.spec(
                        runtimeID: runtimeID, envDir: envDir, bundle: bundle, record: record)
                    try await adapter.executeJob(
                        record, spec: spec, payload: payload, into: continuation)
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func executeJob(
        _ record: ModelRecord, spec: SidecarSpec, payload: JSONValue,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        await governor.beginGeneration(record.id)
        do {
            try await runThroughSidecar(record, spec: spec, payload: payload, into: continuation)
            await governor.endGeneration(record.id)
        } catch {
            await governor.endGeneration(record.id)
            throw error
        }
    }

    private func runThroughSidecar(
        _ record: ModelRecord, spec: SidecarSpec, payload: JSONValue,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        let producer = GPUProducer.job(modelID: record.id)
        try await SidecarWarmLoad.acquire(
            governor: governor, supervisor: supervisor, spec: spec, record: record,
            producer: producer, warmWindow: spec.idleTimeout,
            startingStatus: "Starting image runtime…"
        ) { continuation.yield(.status($0)) }

        var control: [String: JSONValue] = ["op": .string("image")]
        if case .object(let fields) = payload {
            for (key, value) in fields { control[key] = value }
        }
        do {
            let stream = await supervisor.jobRequest(spec, .object(control))
            for try await event in stream {
                continuation.yield(event)
            }
            await governor.gate.release(producer)
        } catch {
            await governor.gate.release(producer)
            throw error
        }
    }

    static func spec(
        runtimeID: String, envDir: URL, bundle: URL, record: ModelRecord, workdir: URL? = nil
    ) throws -> SidecarSpec {
        let fm = FileManager.default
        let paths = SidecarModelPaths.resolve(record)
        let workdir =
            workdir
            ?? Registry.defaultDirectory()
            .appendingPathComponent("workdirs/python-mflux", isDirectory: true)
        try fm.createDirectory(at: workdir, withIntermediateDirectories: true)

        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: SandboxArgv.build(
                envDir: envDir, bundle: bundle,
                modelSandboxRoot: URL(fileURLWithPath: paths.sandboxRoot), workdir: workdir,
                trailingArguments: [
                    "--model", paths.snapshot,
                    "--name", record.name,
                    "--workdir", workdir.path,
                ]),
            environment: ["PYTHONDONTWRITEBYTECODE": "1"],
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            idleTimeout: .seconds(60))
    }
}
