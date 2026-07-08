import Foundation

public struct ManifestCommandAdapter: RuntimeAdapter, JobRunning, ManifestBacked {
    public let manifest: RuntimeManifest
    public let approvedNetwork: Bool

    public var id: String { manifest.id }

    public init(manifest: RuntimeManifest, approvedNetwork: Bool) {
        self.manifest = manifest
        self.approvedNetwork = approvedNetwork
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

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let task = Task {
                do {
                    guard adapter.manifest.execution != .job else {
                        throw KernelError.runtimeFailed(
                            "\(adapter.id) runs as jobs, not streams")
                    }
                    continuation.yield(.status("Running \(adapter.id)…"))
                    let output = try await adapter.runCommand(record, payload: payload) {
                        continuation.yield(.status($0))
                    }
                    for line in output.split(separator: "\n", omittingEmptySubsequences: false) {
                        continuation.yield(.text(String(line) + "\n"))
                    }
                    continuation.yield(.done(nil))
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    public func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let task = Task {
                do {
                    guard adapter.manifest.execution == .job else {
                        throw KernelError.runtimeFailed(
                            "\(adapter.id) streams, it does not run jobs")
                    }
                    continuation.yield(.status("Running \(adapter.id)…"))
                    let outputs = try await adapter.runJob(record, payload: payload) {
                        continuation.yield(.status($0))
                    }
                    continuation.yield(.started)
                    for file in outputs {
                        let data = try Data(contentsOf: file)
                        continuation.yield(
                            .result(data: data, fileExtension: file.pathExtension))
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func runCommand(
        _ record: ModelRecord, payload: JSONValue,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> String {
        let (stdout, _) = try await execute(record, payload: payload, progress: progress)
        return stdout
    }

    private func runJob(
        _ record: ModelRecord, payload: JSONValue,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> [URL] {
        let (_, outputs) = try await execute(record, payload: payload, progress: progress)
        let files =
            (try? FileManager.default.contentsOfDirectory(
                at: outputs, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])) ?? []
        return files.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    private func execute(
        _ record: ModelRecord, payload: JSONValue,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> (stdout: String, outputs: URL) {
        guard !networkBlocked else {
            throw KernelError.runtimeUnavailable(
                hint: "\(id) needs network permission. Approve it from the model's page.")
        }
        guard let invoke = manifest.invoke else {
            throw KernelError.runtimeFailed("\(id) declares no [invoke] command")
        }
        guard
            let profile = ManifestSupport.profileURL(
                network: manifest.permissions.network && approvedNetwork)
        else {
            throw KernelError.runtimeFailed("generic sandbox profile missing")
        }

        let envDir = try await ManifestSupport.prepareEnvironmentIfNeeded(
            manifest: manifest, progress: progress)
        let workdir = try ManifestSupport.workdir(for: manifest)
        let outputs = workdir.appendingPathComponent("outputs", isDirectory: true)
        try? FileManager.default.removeItem(at: outputs)
        try FileManager.default.createDirectory(at: outputs, withIntermediateDirectories: true)

        let tokens = try ManifestSupport.substituted(
            command: invoke.command, record: record, payload: payload,
            workdir: workdir, outputs: outputs, envDir: envDir)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/sandbox-exec")
        process.arguments =
            ManifestSupport.sandboxArguments(
                profile: profile, envDir: envDir, manifest: manifest, record: record,
                workdir: workdir) + tokens
        process.currentDirectoryURL = workdir
        var environment = ProcessInfo.processInfo.environment
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        environment.removeValue(forKey: "PYTHONPATH")
        environment.removeValue(forKey: "PYTHONHOME")
        process.environment = environment
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        let drain = PipeDrain(stdout: stdout, stderr: stderr)
        try process.run()
        return try await withTaskCancellationHandler {
            let (outputData, errorData) = await drain.collect(process: process)
            guard process.terminationStatus == 0 else {
                let tail = ManifestSupport.errorSummary(String(decoding: errorData, as: UTF8.self))
                throw KernelError.runtimeFailed(
                    "\(id) stopped with status \(process.terminationStatus): \(tail)")
            }
            return (String(decoding: outputData, as: UTF8.self), outputs)
        } onCancel: {
            drain.cancel()
            if process.isRunning { process.terminate() }
        }
    }
}

private final class PipeDrain: @unchecked Sendable {
    private let stdout: Pipe
    private let stderr: Pipe
    private let lock = NSLock()
    private var out = Data()
    private var err = Data()

    init(stdout: Pipe, stderr: Pipe) {
        self.stdout = stdout
        self.stderr = stderr
    }

    func collect(process: Process) async -> (stdout: Data, stderr: Data) {
        stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.append(handle.availableData, isStdout: true)
        }
        stderr.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.append(handle.availableData, isStdout: false)
        }
        await withCheckedContinuation { continuation in
            let resumed = ResumeOnce(continuation)
            process.terminationHandler = { _ in resumed.fire() }
            if !process.isRunning { resumed.fire() }
        }
        cancel()
        return lock.withLock { (out, err) }
    }

    func cancel() {
        stdout.fileHandleForReading.readabilityHandler = nil
        stderr.fileHandleForReading.readabilityHandler = nil
        append((try? stdout.fileHandleForReading.read(upToCount: 1 << 20)) ?? Data(), isStdout: true)
        append((try? stderr.fileHandleForReading.read(upToCount: 1 << 20)) ?? Data(), isStdout: false)
    }

    private func append(_ data: Data, isStdout: Bool) {
        guard !data.isEmpty else { return }
        lock.withLock {
            if isStdout { out.append(data) } else { err.append(data) }
        }
    }
}

private final class ResumeOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Never>?

    init(_ continuation: CheckedContinuation<Void, Never>) {
        self.continuation = continuation
    }

    func fire() {
        lock.lock()
        let c = continuation
        continuation = nil
        lock.unlock()
        c?.resume()
    }
}
