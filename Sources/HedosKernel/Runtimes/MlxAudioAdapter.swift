import Foundation

public struct MlxAudioAdapter: RuntimeAdapter {
    public var id: String { "python:mlx-audio" }

    public init() {}

    public static func bundleDirectory() -> URL? {
        guard let root = Bundle.module.resourceURL else { return nil }
        let candidates = [
            root.appendingPathComponent("Resources/Runtimes/python-mlx-audio"),
            root.appendingPathComponent("Runtimes/python-mlx-audio"),
            root.deletingLastPathComponent()
                .appendingPathComponent("Resources/Runtimes/python-mlx-audio"),
        ]
        return candidates.first { FileManager.default.fileExists(atPath: $0.path) }
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .speak
    }

    public static func availableVoices(_ record: ModelRecord) -> [String] {
        guard let paths = try? resolvedModelPaths(record) else { return [] }
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
            let task = Task {
                do {
                    guard let bundle = Self.bundleDirectory(),
                        FileManager.default.fileExists(atPath: bundle.path)
                    else {
                        throw KernelError.runtimeFailed("mlx-audio runtime bundle missing")
                    }
                    continuation.yield(.status("Preparing speech runtime…"))
                    let envDir = try await EnvironmentManager.shared.prepare(
                        runtimeID: id,
                        lockfile: bundle.appendingPathComponent("requirements.lock"),
                        progress: { message in continuation.yield(.status(message)) })

                    let spec = try Self.spec(
                        runtimeID: id, envDir: envDir, bundle: bundle, record: record)
                    continuation.yield(.status("Starting speech runtime…"))
                    try await SidecarSupervisor.shared.ensureRunning(spec)

                    var control: [String: JSONValue] = ["op": .string("speak")]
                    if case .object(let fields) = payload {
                        for (key, value) in fields { control[key] = value }
                    }
                    let stream = await SidecarSupervisor.shared.request(spec, .object(control))
                    for try await chunk in stream {
                        continuation.yield(chunk)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func spec(
        runtimeID: String, envDir: URL, bundle: URL, record: ModelRecord
    ) throws -> SidecarSpec {
        let fm = FileManager.default
        let paths = try resolvedModelPaths(record)
        let workdir = Registry.defaultDirectory()
            .appendingPathComponent("workdirs/python-mlx-audio", isDirectory: true)
        try fm.createDirectory(at: workdir, withIntermediateDirectories: true)

        let python = envDir.appendingPathComponent("bin/python")
        let realPython = python.resolvingSymlinksInPath()
        let uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent()
        let tmp = fm.temporaryDirectory.resolvingSymlinksInPath()
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")

        return SidecarSpec(
            runtimeID: runtimeID,
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

    static func resolvedModelPaths(_ record: ModelRecord) throws -> (
        sandboxRoot: String, snapshot: String
    ) {
        let base = URL(
            fileURLWithPath: (record.source.path as NSString).expandingTildeInPath)
        let root = base.resolvingSymlinksInPath()
        if record.source.kind == .huggingfaceCache, let ref = record.source.ref {
            let snapshot = root.appendingPathComponent("snapshots/\(ref)")
            if FileManager.default.fileExists(atPath: snapshot.path) {
                return (root.path, snapshot.path)
            }
        }
        return (root.path, root.path)
    }
}
