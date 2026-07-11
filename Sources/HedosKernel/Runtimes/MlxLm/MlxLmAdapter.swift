import Foundation

struct MlxLmAdapter: RuntimeAdapter {
    var id: RuntimeID { .mlxLm }

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

    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        record.contextLength
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .mlxSafetensors,
            identified.modality == .text,
            identified.capabilities.contains(.chat)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.mlxLm)
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete else { return [] }
        return [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "repeat_penalty",
            "seed", "stop",
        ]
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        let stream = runtime.stream(record, op: capability, payload: payload)
        guard capability == .chat || capability == .complete else { return stream }
        return ThinkSplitter.separating(stream)
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
            runtimeID: RuntimeID.mlxLm.rawValue,
            preparingStatus: "Preparing text runtime…",
            startingStatus: "Starting text runtime…",
            warmWindow: nil,
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require("python-mlx-lm", runtimeID: .mlxLm)
                return try await environments.prepare(
                    runtimeID: RuntimeID.mlxLm.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require("python-mlx-lm", runtimeID: .mlxLm)
                return try SidecarBundle.spec(
                    runtimeID: .mlxLm, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-mlx-lm",
                    cooperativeCancel: true)
            })
    }
}
