import Foundation

struct MlxVlmAdapter: RuntimeAdapter {
    var id: RuntimeID { .mlxVlm }

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
        record.runtime.id == id
            && (capability == .chat || capability == .complete || capability == .see)
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .mlxSafetensors,
            identified.capabilities.contains(.see)
        else { return nil }
        return RuntimeBid(
            tier: .managed, preference: BidPreference.mlxVlm, alternatives: [.mlxSwift])
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete || capability == .see else {
            return []
        }
        return ["temperature", "top_p", "max_tokens"]
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        runtime.stream(record, op: .chat, payload: payload)
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
            runtimeID: RuntimeID.mlxVlm.rawValue,
            preparingStatus: "Preparing vision runtime…",
            startingStatus: "Starting vision runtime…",
            warmWindow: nil,
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require("python-mlx-vlm", runtimeID: .mlxVlm)
                return try await environments.prepare(
                    runtimeID: RuntimeID.mlxVlm.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require("python-mlx-vlm", runtimeID: .mlxVlm)
                return try SidecarBundle.spec(
                    runtimeID: .mlxVlm, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-mlx-vlm",
                    cooperativeCancel: true)
            })
    }
}
