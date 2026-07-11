import Foundation

struct VMCommandAdapter: RuntimeAdapter, JobRunning, ManifestBacked {
    let manifest: RuntimeManifest
    let host: any VMHost
    let workdirRoot: URL

    private let governor: MemoryGovernor

    var id: RuntimeID { RuntimeID(rawValue: manifest.id) }

    init(
        manifest: RuntimeManifest, host: any VMHost,
        governor: MemoryGovernor = .shared,
        workdirRoot: URL = ManifestSupport.defaultWorkdirRoot()
    ) {
        self.manifest = manifest
        self.host = host
        self.governor = governor
        self.workdirRoot = workdirRoot
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && manifest.capabilities.contains(capability)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard let detect = manifest.detect, detect.matches(record) else {
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
                    continuation.yield(.status("Running \(adapter.id) in its own machine…"))
                    let output = try await GovernedOneShot.run(
                        governor: adapter.governor, record: record,
                        producer: GPUProducer.generation(modelID: record.id),
                        status: { continuation.yield(.status($0)) }
                    ) {
                        try await adapter.execute(record, payload: payload) {
                            continuation.yield(.status($0))
                        }
                    }
                    let files = try FileManager.default.contentsOfDirectory(
                        at: output.outputs, includingPropertiesForKeys: nil,
                        options: [.skipsHiddenFiles])
                    var spokeAudio = false
                    for file in files.sorted(by: { $0.lastPathComponent < $1.lastPathComponent })
                    where file.pathExtension.lowercased() == "wav" {
                        guard let wav = try? ManifestSupport.boundedOutputData(at: file),
                            let audio = SpeechAudio.float32PCM(fromWAV: wav)
                        else { continue }
                        spokeAudio = true
                        continuation.yield(
                            .audio(AudioFrame(data: audio.pcm, sampleRate: audio.sampleRate)))
                    }
                    if !spokeAudio {
                        for line in output.stdout.split(
                            separator: "\n", omittingEmptySubsequences: false)
                        {
                            continuation.yield(.text(String(line) + "\n"))
                        }
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
                    continuation.yield(.status("Running \(adapter.id) in its own machine…"))
                    let result = try await GovernedOneShot.run(
                        governor: adapter.governor, record: record,
                        producer: GPUProducer.job(modelID: record.id),
                        status: { continuation.yield(.status($0)) }
                    ) {
                        try await adapter.execute(record, payload: payload) {
                            continuation.yield(.status($0))
                        }
                    }
                    continuation.yield(.started)
                    let files = try FileManager.default.contentsOfDirectory(
                        at: result.outputs, includingPropertiesForKeys: nil,
                        options: [.skipsHiddenFiles])
                    for file in files.sorted(by: { $0.lastPathComponent < $1.lastPathComponent })
                    {
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

    private func execute(
        _ record: ModelRecord, payload: JSONValue,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> (stdout: String, outputs: URL) {
        guard let vm = manifest.vm else {
            throw KernelError.runtimeFailed("\(id) declares no [vm] section")
        }
        guard let invoke = manifest.invoke else {
            throw KernelError.runtimeFailed("\(id) declares no [invoke] command")
        }
        let paths = SidecarModelPaths.resolve(record)
        let workdir = try ManifestSupport.workdir(for: manifest, root: workdirRoot)
        let outputs = workdir.appendingPathComponent("outputs", isDirectory: true)
        try? FileManager.default.removeItem(at: outputs)
        try FileManager.default.createDirectory(at: outputs, withIntermediateDirectories: true)

        let tokens = try ManifestSupport.substitutedForVM(
            command: invoke.command, payload: payload)
        let request = VMRunRequest(
            runtimeID: manifest.id,
            image: vm.image,
            setup: vm.setup,
            arguments: tokens,
            modelPath: paths.snapshot,
            resourcesPath: manifest.directory.map { ManifestSupport.canonicalPath($0) },
            workdir: ManifestSupport.canonicalPath(workdir),
            outputs: ManifestSupport.canonicalPath(outputs))

        let host = host
        if await !host.environmentReady(request) {
            progress("Preparing the contained runtime — first use only")
            try await host.provisionEnvironment(request) { progress($0) }
        }
        let runtimeID = manifest.id
        let result = try await withTaskCancellationHandler {
            try await host.run(request)
        } onCancel: {
            Task { await host.cancel(runtimeID: runtimeID) }
        }
        guard result.exitCode == 0 else {
            let tail = ManifestSupport.errorSummary(result.stderr + result.stdout)
            throw KernelError.runtimeFailed(
                "\(id) stopped with status \(result.exitCode): \(tail)")
        }
        return (result.stdout, outputs)
    }
}
