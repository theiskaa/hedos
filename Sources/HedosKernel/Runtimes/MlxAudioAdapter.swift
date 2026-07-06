import Foundation

public struct MlxAudioAdapter: RuntimeAdapter {
    public var id: String { "python:mlx-audio" }

    private let governor: MemoryGovernor

    public init(governor: MemoryGovernor = .shared) {
        self.governor = governor
    }

    public static func bundleDirectory() -> URL? {
        RuntimeBundle.directory(named: "python-mlx-audio")
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .speak
    }

    public static func availableVoices(_ record: ModelRecord) -> [String] {
        let paths = SidecarModelPaths.resolve(record)
        let voicesDir = URL(fileURLWithPath: paths.snapshot).appendingPathComponent("voices")
        let files = (try? FileManager.default.contentsOfDirectory(atPath: voicesDir.path)) ?? []
        return files.filter { $0.hasSuffix(".safetensors") }
            .map { String($0.dropLast(".safetensors".count)) }
            .sorted()
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.modality == .speech,
            identified.capabilities.contains(.speak),
            identified.format == .safetensors || identified.format == .mlxSafetensors
        else { return nil }
        return RuntimeBid(tier: .managed, preference: 30)
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let governor = governor
            let runtimeID = id
            let task = Task {
                await governor.beginGeneration(record.id)
                do {
                    guard let bundle = Self.bundleDirectory(),
                        FileManager.default.fileExists(atPath: bundle.path)
                    else {
                        throw KernelError.runtimeFailed("mlx-audio runtime bundle missing")
                    }
                    continuation.yield(.status("Preparing speech runtime…"))
                    let envDir = try await EnvironmentManager.shared.prepare(
                        runtimeID: runtimeID,
                        lockfile: bundle.appendingPathComponent("requirements.lock"),
                        progress: { message in continuation.yield(.status(message)) })

                    let spec = try Self.spec(
                        runtimeID: runtimeID, envDir: envDir, bundle: bundle, record: record)
                    let producer = GPUProducer.generation(modelID: record.id)
                    while true {
                        if await !SidecarSupervisor.shared.isRunning(spec.runtimeID) {
                            let verdict = try await governor.admit(
                                modelID: record.id, name: record.name,
                                footprintMB: record.footprintMB
                            ) { reason in
                                continuation.yield(.status(reason))
                            }
                            if verdict == .tight {
                                continuation.yield(.status("Memory is tight — loading anyway"))
                            }
                            continuation.yield(.status("Starting speech runtime…"))
                            let loadProducer = GPUProducer.load(modelID: record.id)
                            await governor.gate.acquire(loadProducer)
                            do {
                                try await SidecarSupervisor.shared.ensureRunning(spec)
                                await governor.gate.release(loadProducer)
                            } catch {
                                await governor.gate.release(loadProducer)
                                await governor.markUnloaded(record.id)
                                throw error
                            }
                            await governor.markLoaded(
                                modelID: record.id, name: record.name,
                                footprintMB: record.footprintMB,
                                warmWindow: spec.idleTimeout
                            ) {
                                await SidecarSupervisor.shared.shutdown(spec.runtimeID)
                            }
                        }
                        await governor.gate.acquire(producer)
                        if await SidecarSupervisor.shared.isRunning(spec.runtimeID) { break }
                        await governor.gate.release(producer)
                    }

                    var control: [String: JSONValue] = ["op": .string("speak")]
                    if case .object(let fields) = payload {
                        for (key, value) in fields { control[key] = value }
                    }
                    do {
                        let stream = await SidecarSupervisor.shared.request(
                            spec, .object(control))
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

    static func spec(
        runtimeID: String, envDir: URL, bundle: URL, record: ModelRecord
    ) throws -> SidecarSpec {
        let fm = FileManager.default
        let paths = SidecarModelPaths.resolve(record)
        let workdir = Registry.defaultDirectory()
            .appendingPathComponent("workdirs/python-mlx-audio", isDirectory: true)
        try fm.createDirectory(at: workdir, withIntermediateDirectories: true)

        let python = envDir.appendingPathComponent("bin/python")
        let realPython = python.resolvingSymlinksInPath()
        let uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent()
        let tmp = fm.temporaryDirectory.resolvingSymlinksInPath()
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")

        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: [
                "-f", bundle.appendingPathComponent("sandbox.sb").path,
                "-D", "VENV=\(envDir.resolvingSymlinksInPath().path)",
                "-D", "UVPY=\(uvPythonRoot.path)",
                "-D", "MODEL=\(paths.sandboxRoot)",
                "-D", "WORKDIR=\(workdir.resolvingSymlinksInPath().path)",
                "-D", "RESOURCES=\(bundle.path)",
                "-D", "TMP=\(tmp.path)",
                "-D", "CACHE=\(darwinCache.path)",
                python.path,
                bundle.appendingPathComponent("main.py").path,
                "--model", paths.snapshot,
                "--workdir", workdir.path,
            ],
            workingDirectory: workdir)
    }
}
