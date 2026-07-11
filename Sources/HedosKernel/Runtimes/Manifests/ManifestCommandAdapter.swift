import Darwin
import Foundation

struct ManifestCommandAdapter: RuntimeAdapter, JobRunning, ManifestBacked {
    static let defaultExecutionTimeout: Duration = .seconds(1800)

    let manifest: RuntimeManifest
    let approvedHostExecution: Bool
    let approvedNetwork: Bool
    let workdirRoot: URL
    let executionTimeout: Duration

    private let governor: MemoryGovernor

    var id: RuntimeID { RuntimeID(rawValue: manifest.id) }

    init(
        manifest: RuntimeManifest, approvedHostExecution: Bool, approvedNetwork: Bool,
        governor: MemoryGovernor = .shared,
        workdirRoot: URL = ManifestSupport.defaultWorkdirRoot(),
        executionTimeout: Duration = ManifestCommandAdapter.defaultExecutionTimeout
    ) {
        self.manifest = manifest
        self.approvedHostExecution = approvedHostExecution
        self.approvedNetwork = approvedNetwork
        self.governor = governor
        self.workdirRoot = workdirRoot
        self.executionTimeout = executionTimeout
    }

    private var executionBlocked: Bool {
        !approvedHostExecution
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && manifest.capabilities.contains(capability)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard let detect = manifest.detect, detect.matches(record), !executionBlocked else {
            return nil
        }
        return RuntimeBid(tier: .managed, preference: BidPreference.manifest, alternatives: manifest.alternativeIDs)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let task = Task {
                do {
                    guard adapter.manifest.execution != .job else {
                        throw KernelError.wrongExecutionMode(
                            runtimeID: adapter.id, expected: .job)
                    }
                    continuation.yield(.status("Running \(adapter.id)…"))
                    let output = try await GovernedOneShot.run(
                        governor: adapter.governor, record: record,
                        producer: GPUProducer.generation(modelID: record.id),
                        status: { continuation.yield(.status($0)) }
                    ) {
                        try await adapter.runCommand(record, payload: payload) {
                            continuation.yield(.status($0))
                        }
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

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let adapter = self
            let task = Task {
                do {
                    guard adapter.manifest.execution == .job else {
                        throw KernelError.wrongExecutionMode(
                            runtimeID: adapter.id, expected: .stream)
                    }
                    continuation.yield(.status("Running \(adapter.id)…"))
                    let outputs = try await GovernedOneShot.run(
                        governor: adapter.governor, record: record,
                        producer: GPUProducer.job(modelID: record.id),
                        status: { continuation.yield(.status($0)) }
                    ) {
                        try await adapter.runJob(record, payload: payload) {
                            continuation.yield(.status($0))
                        }
                    }
                    continuation.yield(.started)
                    for file in outputs {
                        let data = try ManifestSupport.boundedOutputData(at: file)
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
        let files = try FileManager.default.contentsOfDirectory(
            at: outputs, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        return files.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    private func execute(
        _ record: ModelRecord, payload: JSONValue,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> (stdout: String, outputs: URL) {
        guard !executionBlocked else {
            throw KernelError.runtimeUnavailable(
                hint: "\(id) runs code on this Mac and needs your approval. Approve it from the model's page.")
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
        let workdir = try ManifestSupport.workdir(for: manifest, root: workdirRoot)
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

        let drain = PipeDrain(stdout: stdout, stderr: stderr) {
            Self.terminateProcessTree(process)
        }
        try process.run()
        let executionTimeout = executionTimeout
        let timedOut = TimeoutFlag()
        let timeoutTask = Task {
            try? await Task.sleep(for: executionTimeout)
            guard !Task.isCancelled else { return }
            await timedOut.fire()
            Self.terminateProcessTree(process)
            drain.cancel()
        }
        return try await withTaskCancellationHandler {
            let (outputData, errorData) = await drain.collect(process: process)
            timeoutTask.cancel()
            if await timedOut.didFire {
                let minutes = max(1, Int(executionTimeout.components.seconds / 60))
                throw KernelError.runtimeFailed(
                    "\(id) ran for more than \(minutes) minutes and was stopped")
            }
            guard process.terminationStatus == 0 else {
                let tail = ManifestSupport.errorSummary(String(decoding: errorData, as: UTF8.self))
                throw KernelError.runtimeFailed(
                    "\(id) stopped with status \(process.terminationStatus): \(tail)")
            }
            return (String(decoding: outputData, as: UTF8.self), outputs)
        } onCancel: {
            timeoutTask.cancel()
            drain.cancel()
            Self.terminateProcessTree(process)
        }
    }

    static func descendantPIDs(of pid: pid_t) -> [pid_t] {
        let ps = Process()
        ps.executableURL = URL(fileURLWithPath: "/bin/ps")
        ps.arguments = ["-A", "-o", "pid=,ppid="]
        let stdout = Pipe()
        ps.standardOutput = stdout
        ps.standardError = Pipe()
        guard (try? ps.run()) != nil else { return [] }
        let data = (try? stdout.fileHandleForReading.readToEnd()) ?? nil
        ps.waitUntilExit()
        guard let data else { return [] }

        var parents: [pid_t: [pid_t]] = [:]
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            let fields = line.split(separator: " ").compactMap { pid_t($0) }
            guard fields.count == 2 else { continue }
            parents[fields[1], default: []].append(fields[0])
        }

        var result: [pid_t] = []
        var frontier: [pid_t] = [pid]
        while !frontier.isEmpty {
            let current = frontier.removeFirst()
            let children = parents[current] ?? []
            result.append(contentsOf: children)
            frontier.append(contentsOf: children)
        }
        return result
    }

    static func terminateProcessTree(_ process: Process, grace: Duration = .milliseconds(500)) {
        let pid = process.processIdentifier
        guard pid > 0 else {
            if process.isRunning { process.terminate() }
            return
        }
        let descendants = descendantPIDs(of: pid)
        kill(pid, SIGTERM)
        for child in descendants { kill(child, SIGTERM) }
        if process.isRunning { process.terminate() }
        Task {
            try? await Task.sleep(for: grace)
            for _ in 0..<2 {
                for child in descendantPIDs(of: pid) where kill(child, 0) == 0 {
                    kill(child, SIGKILL)
                }
                for child in descendants where kill(child, 0) == 0 {
                    kill(child, SIGKILL)
                }
                if process.isRunning {
                    kill(pid, SIGKILL)
                }
            }
        }
    }
}

actor TimeoutFlag {
    private(set) var didFire = false
    func fire() { didFire = true }
}

final class PipeDrain: @unchecked Sendable {
    static let maxBytesPerStream = 16 * 1024 * 1024

    private let stdout: Pipe
    private let stderr: Pipe
    private let maxBytes: Int
    private let lock = NSLock()
    private var out = Data()
    private var err = Data()
    private var exceeded = false
    private let onCapExceeded: @Sendable () -> Void

    init(
        stdout: Pipe, stderr: Pipe, maxBytes: Int = PipeDrain.maxBytesPerStream,
        onCapExceeded: @escaping @Sendable () -> Void = {}
    ) {
        self.stdout = stdout
        self.stderr = stderr
        self.maxBytes = maxBytes
        self.onCapExceeded = onCapExceeded
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
        var didExceed = false
        lock.withLock {
            guard !exceeded else { return }
            if isStdout {
                out.append(data)
                if out.count > maxBytes { exceeded = true }
            } else {
                err.append(data)
                if err.count > maxBytes { exceeded = true }
            }
            didExceed = exceeded
        }
        if didExceed { onCapExceeded() }
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
