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
            ProcessContainment.terminateProcessTree(process)
        }
        try process.run()
        let executionTimeout = executionTimeout
        let timedOut = TimeoutFlag()
        let timeoutTask = Task {
            try? await Task.sleep(for: executionTimeout)
            guard !Task.isCancelled else { return }
            await timedOut.fire()
            ProcessContainment.terminateProcessTree(process)
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
            ProcessContainment.terminateProcessTree(process)
            timeoutTask.cancel()
            drain.cancel()
        }
    }

}
