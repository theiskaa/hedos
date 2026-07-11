import Foundation
import Testing

@testable import HedosKernel

private actor ContractVMHost: VMHost {
    func assetState() async -> VMAssetState { .ready }
    func provisionAssets(onStatus: (@Sendable (String) async -> Void)?) async throws {}
    func environmentReady(_ request: VMRunRequest) async -> Bool { true }
    func provisionEnvironment(
        _ request: VMRunRequest, onStatus: (@Sendable (String) async -> Void)?
    ) async throws {}
    func run(_ request: VMRunRequest) async throws -> VMRunResult {
        VMRunResult(exitCode: 0, stdout: "", stderr: "")
    }
    func cancel(runtimeID: String) async {}
}

private func contractManifest(id: String = "contract-cli", vm: Bool = false) -> RuntimeManifest {
    RuntimeManifest(
        id: vm ? "contract-vm" : id,
        modalities: [.text],
        capabilities: [.chat],
        execution: .stream,
        alternatives: [],
        detect: ManifestDetect(fileExtension: "contractext"),
        env: nil,
        serve: vm ? nil : nil,
        invoke: ManifestInvoke(command: "echo hi"),
        permissions: ManifestPermissions(network: false, paths: ["{model}", "{workdir}"]),
        vm: vm
            ? ManifestVM(
                image: "example.com/img@sha256:0000000000000000000000000000000000000000000000000000000000000000",
                setup: [])
            : nil,
        directory: nil)
}

private func contractAdapters() -> [(String, any RuntimeAdapter)] {
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let workdirRoot = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-contract-workdirs")
    return [
        ("llama.cpp", LlamaCppAdapter(governor: governor)),
        ("mlx-swift", MlxSwiftAdapter(governor: governor)),
        ("whisper.cpp", WhisperCppAdapter(governor: governor)),
        ("ollama", OllamaAdapter()),
        ("openai-endpoint", OpenAIEndpointAdapter(secrets: InMemorySecretStore())),
        ("apple-foundation", AppleFoundationAdapter()),
        ("mflux", MfluxAdapter(governor: governor)),
        ("diffusers", DiffusersAdapter(governor: governor)),
        ("mlx-lm", MlxLmAdapter(governor: governor)),
        ("mlx-audio", MlxAudioAdapter(governor: governor)),
        (
            "manifest-command",
            ManifestCommandAdapter(
                manifest: contractManifest(), approvedNetwork: true,
                governor: governor, workdirRoot: workdirRoot)
        ),
        (
            "manifest-sidecar",
            ManifestSidecarAdapter(
                manifest: contractManifest(id: "contract-serve"), approvedNetwork: true,
                governor: governor, supervisor: SidecarSupervisor(),
                workdirRoot: workdirRoot)
        ),
        (
            "vm-command",
            VMCommandAdapter(
                manifest: contractManifest(vm: true), host: ContractVMHost(),
                governor: governor, workdirRoot: workdirRoot)
        ),
    ]
}

private func foreignRecord() -> ModelRecord {
    ModelRecord(
        name: "contract-foreign",
        modality: .unknown,
        capabilities: [],
        source: ModelSource(kind: .file, path: "/tmp/hedos-contract/model.zzz"),
        execution: .stream,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func everyAdapterReportsAStableID() {
    for (name, adapter) in contractAdapters() {
        #expect(adapter.id == adapter.id, "\(name)")
        #expect(!adapter.id.rawValue.isEmpty, "\(name)")
    }
}

@Test func noAdapterBidsOnAnUnknownFormatRecord() {
    let identified = IdentifiedModel(
        format: .unknown, modality: nil, capabilities: [], execution: .stream)
    let record = foreignRecord()
    for (name, adapter) in contractAdapters() {
        #expect(adapter.bid(record, identified) == nil, "\(name)")
    }
}

@Test func noAdapterServesAnUnknownCapability() {
    let record = foreignRecord()
    let bogus = Capability(rawValue: "no-such-capability")
    for (name, adapter) in contractAdapters() {
        #expect(!adapter.canServe(record, bogus), "\(name)")
    }
}

@Test func adapterIDsAreUniqueAcrossTheRoster() {
    let ids = contractAdapters().map(\.1.id)
    #expect(Set(ids).count == ids.count)
}
