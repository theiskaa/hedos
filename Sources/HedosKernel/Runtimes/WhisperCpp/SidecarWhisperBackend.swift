import Foundation

public actor SidecarWhisperBackend: WhisperBackend {
    public typealias SpecFactory = @Sendable (String) async throws -> SidecarSpec

    private let supervisor: SidecarSupervisor
    private let specFactory: SpecFactory
    private var spec: SidecarSpec?

    public init(
        supervisor: SidecarSupervisor = .shared,
        specFactory: @escaping SpecFactory = SidecarWhisperBackend.bundleSpec
    ) {
        self.supervisor = supervisor
        self.specFactory = specFactory
    }

    public func load(path: String) async throws {
        await unload()
        let next = try await specFactory(path)
        try await supervisor.ensureRunning(next)
        spec = next
    }

    public func unload() async {
        if let spec {
            await supervisor.shutdown(spec.runtimeID)
        }
        spec = nil
    }

    public nonisolated func transcribe(samples: [Float]) -> AsyncThrowingStream<String, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    try await self.run(samples: samples, continuation: continuation)
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func run(
        samples: [Float],
        continuation: AsyncThrowingStream<String, Error>.Continuation
    ) async throws {
        guard let spec else {
            throw KernelError.runtimeFailed("the whisper sidecar is not loaded")
        }
        let workdir = spec.workingDirectory ?? FileManager.default.temporaryDirectory
        let pcmURL = workdir.appendingPathComponent("transcribe-\(UUID().uuidString).f32")
        try samples.withUnsafeBytes { Data($0) }.write(to: pcmURL)
        defer { try? FileManager.default.removeItem(at: pcmURL) }
        let stream = await supervisor.request(
            spec,
            .object([
                "op": .string("transcribe"),
                "pcm": .string(pcmURL.path),
                "sample_rate": .int(WhisperEngine.expectedSampleRate),
            ]))
        for try await chunk in stream {
            if case .text(let delta) = chunk {
                continuation.yield(delta)
            }
        }
    }

    public static func bundleSpec(path: String) async throws -> SidecarSpec {
        let runtimeID = "python:whisper-cpp"
        guard let bundle = RuntimeBundle.directory(named: "python-whisper-cpp"),
            FileManager.default.fileExists(atPath: bundle.path)
        else {
            throw KernelError.runtimeFailed("whisper runtime bundle missing")
        }
        let envDir = try await EnvironmentManager.shared.prepare(
            runtimeID: runtimeID,
            lockfile: bundle.appendingPathComponent("requirements.lock"),
            progress: { _ in })

        let fm = FileManager.default
        let workdir = Registry.defaultDirectory()
            .appendingPathComponent("workdirs/python-whisper-cpp", isDirectory: true)
        try fm.createDirectory(at: workdir, withIntermediateDirectories: true)

        let python = envDir.appendingPathComponent("bin/python")
        let realPython = python.resolvingSymlinksInPath()
        let uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent()
        let tmp = fm.temporaryDirectory.resolvingSymlinksInPath()
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")
        let modelRoot = URL(fileURLWithPath: path).deletingLastPathComponent()

        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(path)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: [
                "-f", bundle.appendingPathComponent("sandbox.sb").path,
                "-D", "VENV=\(envDir.resolvingSymlinksInPath().path)",
                "-D", "UVPY=\(uvPythonRoot.path)",
                "-D", "MODEL=\(modelRoot.path)",
                "-D", "WORKDIR=\(workdir.resolvingSymlinksInPath().path)",
                "-D", "RESOURCES=\(bundle.path)",
                "-D", "TMP=\(tmp.path)",
                "-D", "CACHE=\(darwinCache.path)",
                python.path,
                bundle.appendingPathComponent("main.py").path,
                "--model", path,
                "--workdir", workdir.path,
            ],
            workingDirectory: workdir,
            readyTimeout: .seconds(600))
    }
}
