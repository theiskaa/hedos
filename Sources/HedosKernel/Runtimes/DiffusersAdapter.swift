import Foundation

public struct DiffusersAdapter: RuntimeAdapter, JobRunning {
    public var id: String { "python:diffusers" }

    private let governor: MemoryGovernor
    private let supervisor: SidecarSupervisor

    public init(governor: MemoryGovernor = .shared, supervisor: SidecarSupervisor = .shared) {
        self.governor = governor
        self.supervisor = supervisor
    }

    public static func bundleDirectory() -> URL? {
        RuntimeBundle.directory(named: "python-diffusers")
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .diffusers,
            identified.modality == .image,
            identified.capabilities.contains(.image)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: 26, alternatives: [])
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(
                throwing: KernelError.runtimeFailed("python:diffusers runs image generation as jobs"))
        }
    }

    public func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let runtimeID = id
            let task = Task {
                do {
                    guard let bundle = Self.bundleDirectory(),
                        FileManager.default.fileExists(atPath: bundle.path)
                    else {
                        throw KernelError.runtimeFailed("diffusers runtime bundle missing")
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
        while true {
            if await !supervisor.isRunning(spec.runtimeID) {
                let verdict = try await governor.admit(
                    modelID: record.id, name: record.name,
                    footprintMB: record.footprintMB
                ) { reason in
                    continuation.yield(.status(reason))
                }
                if verdict == .tight {
                    continuation.yield(.status("Memory is tight — loading anyway"))
                }
                continuation.yield(.status("Starting image runtime…"))
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
                let supervisor = supervisor
                await governor.markLoaded(
                    modelID: record.id, name: record.name,
                    footprintMB: record.footprintMB,
                    warmWindow: spec.idleTimeout
                ) {
                    await supervisor.shutdown(spec.runtimeID)
                }
            }
            await governor.gate.acquire(producer)
            if await supervisor.isRunning(spec.runtimeID) { break }
            await governor.gate.release(producer)
        }

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
            .appendingPathComponent("workdirs/python-diffusers", isDirectory: true)
        try fm.createDirectory(at: workdir, withIntermediateDirectories: true)

        let python = envDir.appendingPathComponent("bin/python")
        let realPython = URL(fileURLWithPath: canonicalPath(python))
        let uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent()
        let tmp = URL(fileURLWithPath: canonicalPath(fm.temporaryDirectory))
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")

        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: [
                "-f", bundle.appendingPathComponent("sandbox.sb").path,
                "-D", "VENV=\(canonicalPath(envDir))",
                "-D", "UVPY=\(uvPythonRoot.path)",
                "-D", "MODEL=\(canonicalPath(URL(fileURLWithPath: paths.sandboxRoot)))",
                "-D", "WORKDIR=\(canonicalPath(workdir))",
                "-D", "RESOURCES=\(bundle.path)",
                "-D", "TMP=\(tmp.path)",
                "-D", "CACHE=\(darwinCache.path)",
                python.path,
                bundle.appendingPathComponent("main.py").path,
                "--model", paths.snapshot,
                "--name", record.name,
                "--workdir", workdir.path,
            ],
            environment: ["PYTHONDONTWRITEBYTECODE": "1"],
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            idleTimeout: .seconds(60))
    }

    private static func canonicalPath(_ url: URL) -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard realpath(url.path, &buffer) != nil else {
            return url.resolvingSymlinksInPath().path
        }
        return String(cString: buffer)
    }
}
