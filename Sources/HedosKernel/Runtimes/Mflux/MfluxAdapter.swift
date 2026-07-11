import Foundation

struct MfluxAdapter: RuntimeAdapter, JobRunning {
    var id: RuntimeID { .mflux }

    static let servedPipelineClasses: Set<String> = ["FluxPipeline"]

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
            identified.capabilities.contains(.image),
            let pipelineClass = identified.pipelineClass,
            Self.servedPipelineClasses.contains(pipelineClass)
        else { return nil }
        return RuntimeBid(tier: .managed, preference: BidPreference.mflux, alternatives: [.diffusers])
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(
                throwing: KernelError.wrongExecutionMode(runtimeID: .mflux, expected: .job))
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
            runtimeID: RuntimeID.mflux.rawValue,
            preparingStatus: "Preparing image runtime…",
            startingStatus: "Starting image runtime…",
            warmWindow: .seconds(60),
            prepareEnvironment: { progress in
                let bundle = try SidecarBundle.require("python-mflux", runtimeID: .mflux)
                return try await environments.prepare(
                    runtimeID: RuntimeID.mflux.rawValue,
                    lockfile: bundle.appendingPathComponent("requirements.lock"),
                    progress: progress)
            },
            makeSpec: { record, envDir in
                let bundle = try SidecarBundle.require("python-mflux", runtimeID: .mflux)
                return try SidecarBundle.spec(
                    runtimeID: .mflux, record: record, bundle: bundle, envDir: envDir,
                    workdirRoot: workdirRoot, workdirName: "python-mflux",
                    extraArguments: ["--name", record.name],
                    cancelGraceTimeout: .seconds(60))
            })
    }
}
