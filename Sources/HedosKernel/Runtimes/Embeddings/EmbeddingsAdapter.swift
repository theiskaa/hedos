import Foundation

struct EmbeddingsAdapter: RuntimeAdapter {
    var id: RuntimeID { .embeddings }

    private let governor: MemoryGovernor
    private let supervisor: SidecarSupervisor
    private let environments: EnvironmentManager
    private let workdirRoot: URL

    init(
        governor: MemoryGovernor = .shared, supervisor: SidecarSupervisor = .shared,
        environments: EnvironmentManager = .shared,
        workdirRoot: URL = SidecarWorkdir.defaultRoot()
    ) {
        self.governor = governor
        self.supervisor = supervisor
        self.environments = environments
        self.workdirRoot = workdirRoot
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .embed
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.capabilities.contains(.embed),
            identified.format == .safetensors || identified.format == .mlxSafetensors
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.embeddings)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        runtime.stream(record, op: .embed, payload: payload)
    }

    var runtime: PythonSidecarRuntime {
        PythonSidecarRuntime(
            descriptor: Self.descriptor(environments: environments, workdirRoot: workdirRoot),
            governor: governor, supervisor: supervisor)
    }

    static func descriptor(
        environments: EnvironmentManager, workdirRoot: URL
    ) -> PythonSidecarRuntime.Descriptor {
        PythonSidecarRuntime.Descriptor(
            runtimeID: RuntimeID.embeddings.rawValue,
            preparingStatus: "Preparing embedding runtime…",
            startingStatus: "Starting embedding runtime…",
            warmWindow: nil,
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require(
                    "python-embeddings", runtimeID: .embeddings)
                return try await environments.prepare(
                    runtimeID: RuntimeID.embeddings.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require(
                    "python-embeddings", runtimeID: .embeddings)
                return try SidecarBundle.spec(
                    runtimeID: .embeddings, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-embeddings",
                    cooperativeCancel: false)
            })
    }
}
