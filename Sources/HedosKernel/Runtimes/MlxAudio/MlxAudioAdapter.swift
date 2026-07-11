import Foundation

struct MlxAudioAdapter: RuntimeAdapter {
    var id: RuntimeID { .mlxAudio }

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
        record.runtime.id == id && capability == .speak
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.modality == .speech,
            identified.capabilities.contains(.speak),
            identified.format == .safetensors || identified.format == .mlxSafetensors
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.mlxAudio)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        runtime.stream(record, op: .speak, payload: payload)
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
            runtimeID: RuntimeID.mlxAudio.rawValue,
            preparingStatus: "Preparing speech runtime…",
            startingStatus: "Starting speech runtime…",
            warmWindow: .seconds(120),
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require("python-mlx-audio", runtimeID: .mlxAudio)
                return try await environments.prepare(
                    runtimeID: RuntimeID.mlxAudio.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require("python-mlx-audio", runtimeID: .mlxAudio)
                return try SidecarBundle.spec(
                    runtimeID: .mlxAudio, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-mlx-audio",
                    cooperativeCancel: true)
            })
    }
}
