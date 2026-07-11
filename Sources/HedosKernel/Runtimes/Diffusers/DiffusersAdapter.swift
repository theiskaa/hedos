import Foundation

struct DiffusersAdapter: RuntimeAdapter, JobRunning {
    var id: RuntimeID { .diffusers }

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
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .diffusers,
            identified.modality == .image,
            identified.capabilities.contains(.image)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.diffusers)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(
                throwing: KernelError.wrongExecutionMode(runtimeID: .diffusers, expected: .job))
        }
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        runtime.job(record, op: "image", payload: payload)
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
            runtimeID: RuntimeID.diffusers.rawValue,
            preparingStatus: "Preparing image runtime…",
            startingStatus: "Starting image runtime…",
            warmWindow: .seconds(60),
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require("python-diffusers", runtimeID: .diffusers)
                return try await environments.prepare(
                    runtimeID: RuntimeID.diffusers.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require("python-diffusers", runtimeID: .diffusers)
                return try SidecarBundle.spec(
                    runtimeID: .diffusers, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-diffusers",
                    extraArguments: ["--name", record.name],
                    cancelGraceTimeout: .seconds(60))
            })
    }
}
