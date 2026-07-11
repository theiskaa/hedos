import Foundation

struct ManifestSidecarAdapter: RuntimeAdapter, JobRunning, ManifestBacked {
    let manifest: RuntimeManifest
    let approvedHostExecution: Bool
    let approvedNetwork: Bool
    let workdirRoot: URL

    private let governor: MemoryGovernor
    private let supervisor: SidecarSupervisor

    var id: RuntimeID { RuntimeID(rawValue: manifest.id) }

    init(
        manifest: RuntimeManifest, approvedHostExecution: Bool, approvedNetwork: Bool,
        governor: MemoryGovernor = .shared, supervisor: SidecarSupervisor = .shared,
        workdirRoot: URL = ManifestSupport.defaultWorkdirRoot()
    ) {
        self.manifest = manifest
        self.approvedHostExecution = approvedHostExecution
        self.approvedNetwork = approvedNetwork
        self.governor = governor
        self.supervisor = supervisor
        self.workdirRoot = workdirRoot
    }

    private var executionBlocked: Bool {
        !approvedHostExecution
    }

    private var executionConsentError: KernelError {
        .runtimeUnavailable(
            hint: "\(id) runs code on this Mac and needs your approval. Approve it from the model's page.")
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && manifest.capabilities.contains(capability)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard let detect = manifest.detect, detect.matches(record), !executionBlocked else {
            return nil
        }
        return RuntimeBid(
            tier: .managed, preference: BidPreference.manifest,
            alternatives: manifest.alternativeIDs)
    }

    func spec(record: ModelRecord, envDir: URL?) throws -> SidecarSpec {
        guard let serve = manifest.serve, let directory = manifest.directory else {
            throw KernelError.runtimeFailed("\(id) declares no [serve] entrypoint")
        }
        guard
            let profile = ManifestSupport.profileURL(
                network: manifest.permissions.network && approvedNetwork)
        else {
            throw KernelError.runtimeFailed("generic sandbox profile missing")
        }
        let workdir = try ManifestSupport.workdir(for: manifest, root: workdirRoot)
        let paths = SidecarModelPaths.resolve(record)
        let python =
            envDir?.appendingPathComponent("bin/python").path ?? "/usr/bin/python3"
        return SidecarSpec(
            runtimeID: "\(id)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: ManifestSupport.sandboxArguments(
                profile: profile, envDir: envDir, manifest: manifest, record: record,
                workdir: workdir) + [
                    python,
                    directory.appendingPathComponent(serve.entrypoint).path,
                    "--model", paths.snapshot,
                    "--workdir", workdir.path,
                ],
            environment: ["PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": ""],
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            cooperativeCancel: manifest.execution == .stream)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard !executionBlocked else {
            let error = executionConsentError
            return AsyncThrowingStream { $0.finish(throwing: error) }
        }
        let stream = runtime.stream(record, op: capability, payload: payload)
        guard capability == .chat || capability == .complete else { return stream }
        return ThinkSplitter.separating(stream)
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        guard !executionBlocked else {
            let error = executionConsentError
            return AsyncThrowingStream { $0.finish(throwing: error) }
        }
        return runtime.job(record, op: capability.rawValue, payload: payload)
    }

    var runtime: PythonSidecarRuntime {
        let adapter = self
        return PythonSidecarRuntime(
            descriptor: PythonSidecarRuntime.Descriptor(
                runtimeID: manifest.id,
                preparingStatus: "Preparing \(id)…",
                startingStatus: "Starting \(id)…",
                warmWindow: nil,
                prepareEnvironment: { progress in
                    try await ManifestSupport.prepareEnvironmentIfNeeded(
                        manifest: adapter.manifest, progress: progress)
                },
                makeSpec: { record, envDir in
                    try adapter.spec(record: record, envDir: envDir)
                }),
            governor: governor, supervisor: supervisor)
    }
}
