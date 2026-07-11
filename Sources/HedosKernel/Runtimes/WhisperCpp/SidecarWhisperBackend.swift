import Foundation

actor SidecarWhisperBackend: WhisperBackend {
    typealias SpecFactory = @Sendable (String) async throws -> SidecarSpec

    private let supervisor: SidecarSupervisor
    private let specFactory: SpecFactory
    private var spec: SidecarSpec?

    init(
        supervisor: SidecarSupervisor = .shared,
        specFactory: @escaping SpecFactory = SidecarWhisperBackend.bundleSpec
    ) {
        self.supervisor = supervisor
        self.specFactory = specFactory
    }

    func load(path: String) async throws {
        await unload()
        let next = try await specFactory(path)
        try await supervisor.ensureRunning(next)
        spec = next
    }

    func unload() async {
        if let spec {
            await supervisor.shutdown(spec.runtimeID)
        }
        spec = nil
    }

    nonisolated func transcribe(samples: [Float], options: TranscriptionOptions)
        -> AsyncThrowingStream<TranscriptionSegment, Error>
    {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    try await self.run(
                        samples: samples, options: options, continuation: continuation)
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
        options: TranscriptionOptions,
        continuation: AsyncThrowingStream<TranscriptionSegment, Error>.Continuation
    ) async throws {
        guard let spec else {
            throw KernelError.runtimeFailed("the whisper sidecar is not loaded")
        }
        let workdir = spec.workingDirectory ?? FileManager.default.temporaryDirectory
        let pcmURL = workdir.appendingPathComponent("transcribe-\(UUID().uuidString).f32")
        try samples.withUnsafeBytes { Data($0) }.write(to: pcmURL)
        defer { try? FileManager.default.removeItem(at: pcmURL) }
        var control: [String: JSONValue] = [
            "op": .string("transcribe"),
            "pcm": .string(pcmURL.path),
            "sample_rate": .int(WhisperEngine.expectedSampleRate),
        ]
        if let language = options.language, !language.isEmpty {
            control["language"] = .string(language)
        }
        if options.translate {
            control["translate"] = .bool(true)
        }
        let stream = await supervisor.request(spec, .object(control))
        for try await chunk in stream {
            switch chunk {
            case .segment(let text, let startMs, let endMs):
                continuation.yield(
                    TranscriptionSegment(text: text, startMs: startMs, endMs: endMs))
            case .text(let delta):
                continuation.yield(TranscriptionSegment(text: delta))
            default:
                break
            }
        }
    }

    static func bundleSpec(path: String) async throws -> SidecarSpec {
        let runtimeID = "python:whisper-cpp"
        guard let bundle = RuntimeBundle.directory(named: "python-whisper-cpp"),
            FileManager.default.fileExists(atPath: bundle.path)
        else {
            throw KernelError.bundleMissing(runtimeID: .whisperCpp)
        }
        let envDir = try await EnvironmentManager.shared.prepare(
            runtimeID: runtimeID,
            lockfile: bundle.appendingPathComponent("requirements.lock"),
            progress: { _ in })
        return try makeSpec(
            path: path, bundle: bundle, envDir: envDir, runtimeID: runtimeID,
            workdirRoot: Registry.defaultDirectory())
    }

    static func makeSpec(
        path: String, bundle: URL, envDir: URL, runtimeID: String, workdirRoot: URL
    ) throws -> SidecarSpec {
        let workdir = workdirRoot.appendingPathComponent(
            "workdirs/python-whisper-cpp", isDirectory: true)
        try FileManager.default.createDirectory(at: workdir, withIntermediateDirectories: true)
        let modelRoot = URL(fileURLWithPath: path).deletingLastPathComponent()

        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(path)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: SandboxArgv.build(
                envDir: envDir, bundle: bundle, modelSandboxRoot: modelRoot, workdir: workdir,
                trailingArguments: [
                    "--model", path,
                    "--workdir", workdir.path,
                ]),
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            cooperativeCancel: true,
            cancelGraceTimeout: .seconds(30))
    }
}
