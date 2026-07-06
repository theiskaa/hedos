import Foundation

public enum KernelError: Error, Sendable, LocalizedError {
    case notImplemented(String)
    case modelNotFound(String)
    case artifactNotFound(String)
    case capabilityUnsupported(model: String, capability: Capability)
    case runtimeUnavailable(hint: String)
    case runtimeFailed(String)

    public var errorDescription: String? {
        switch self {
        case .notImplemented(let what):
            "\(what) is not implemented yet."
        case .modelNotFound(let id):
            "No model with id \(id) is registered."
        case .artifactNotFound(let id):
            "No artifact with id \(id) is stored."
        case .capabilityUnsupported(let model, let capability):
            "\(model) has no runtime for \(capability.rawValue)."
        case .runtimeUnavailable(let hint):
            hint
        case .runtimeFailed(let message):
            message
        }
    }
}

public actor Kernel {
    public static let version = "0.1.0"

    public let registry: Registry
    public let settings: SettingsStore
    public let governor: MemoryGovernor
    public let artifactStore: ArtifactStore
    public let chats: ChatStore
    private let adapters: [any RuntimeAdapter]
    private let scheduler: JobScheduler

    public init(
        directory: URL,
        adapters: [any RuntimeAdapter]? = nil,
        governor: MemoryGovernor = .shared
    ) {
        let registry = Registry(directory: directory)
        let artifactStore = ArtifactStore(
            root: directory.appendingPathComponent("outputs", isDirectory: true))
        self.registry = registry
        self.settings = SettingsStore(directory: directory)
        self.governor = governor
        self.artifactStore = artifactStore
        self.chats = ChatStore(databaseURL: directory.appendingPathComponent("chats.sqlite"))
        self.adapters = adapters ?? Self.defaultAdapters(governor: governor)
        self.scheduler = JobScheduler(
            history: JobHistoryStore(directory: directory),
            admission: GovernorAdmission(governor: governor, registry: registry),
            artifacts: ProvenanceArtifactWriter(store: artifactStore, registry: registry))
    }

    public init() {
        self.init(directory: Registry.defaultDirectory())
    }

    private static func defaultAdapters(governor: MemoryGovernor) -> [any RuntimeAdapter] {
        [
            LlamaCppAdapter(governor: governor),
            OllamaAdapter(),
            MlxAudioAdapter(governor: governor),
            MfluxAdapter(governor: governor),
        ]
    }

    public func discover() async throws -> DiscoverySummary {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let watched = (try? await settings.load().watchedFolders) ?? []
        let looseDirectories =
            LooseFileScanner.defaultDirectories()
            + watched.map { URL(fileURLWithPath: $0, isDirectory: true) }
        let scanners: [any StoreScanner] = [
            OllamaStoreScanner(root: home.appendingPathComponent(".ollama/models")),
            HFCacheScanner(roots: HFCacheScanner.defaultRoots()),
            LMStudioScanner(roots: LMStudioScanner.defaultRoots()),
            LooseFileScanner(directories: looseDirectories),
        ]
        let summary = try await DiscoveryService(scanners: scanners).discover(into: registry)
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
        return summary
    }

    public func watchedFolders() async throws -> [String] {
        try await settings.load().watchedFolders
    }

    public func addWatchedFolder(_ path: String) async throws {
        _ = try await settings.addWatchedFolder(path)
    }

    public func removeWatchedFolder(_ path: String) async throws {
        _ = try await settings.removeWatchedFolder(path)
    }

    public func shellState() async throws -> ShellState {
        try await settings.shellState()
    }

    public func saveShellState(_ shell: ShellState) async throws {
        try await settings.saveShellState(shell)
    }

    public func resolve() async throws {
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
    }

    public func confirmRuntime(_ modelID: String) async throws {
        guard var record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        record.runtime.confirmedAt = Date()
        try await registry.register(record)
    }

    public func overrideRuntime(_ modelID: String, to runtimeID: String) async throws {
        guard var record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        let identified = Identification.identify(record)
        let bids = adapters.compactMap { adapter -> (id: String, bid: RuntimeBid)? in
            guard let bid = adapter.bid(record, identified) else { return nil }
            return (adapter.id, bid)
        }
        guard let chosen = bids.first(where: { $0.id == runtimeID }) else {
            throw KernelError.runtimeFailed("runtime \(runtimeID) cannot serve \(record.name)")
        }
        record.runtime = RuntimeRef(
            id: chosen.id,
            resolved: .user,
            tier: chosen.bid.tier,
            alternatives: bids.map(\.id).filter { $0 != runtimeID },
            confirmedAt: Date())
        try await registry.register(record)
    }

    public func shelf() async throws -> [ModelRecord] {
        try await registry.list()
    }

    public func voices(_ modelID: String) async throws -> [String] {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        return MlxAudioAdapter.availableVoices(record)
    }

    public func startOllama() async throws {
        let adapter = adapters.compactMap { $0 as? OllamaAdapter }.first ?? OllamaAdapter()
        try await adapter.startDaemon()
    }

    public func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        guard let adapter = adapters.first(where: { $0.canServe(record, capability) }) else {
            throw KernelError.capabilityUnsupported(model: record.name, capability: capability)
        }
        return adapter.invoke(record, capability, payload: payload)
    }

    public func chat(
        _ modelID: String, messages: [ChatMessage]
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        let payload: JSONValue = .object([
            "messages": .array(
                messages.map {
                    .object([
                        "role": .string($0.role.rawValue),
                        "content": .string($0.content),
                    ])
                })
        ])
        return try await invoke(modelID, .chat, payload: payload)
    }

    public func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        guard let adapter = adapters.first(where: { $0.canServe(record, capability) }) else {
            throw KernelError.capabilityUnsupported(model: record.name, capability: capability)
        }
        guard let runner = adapter as? any JobRunning else {
            throw KernelError.runtimeFailed(
                "\(adapter.id) cannot run \(capability.rawValue) as a job")
        }
        let seededPayload = Self.seeded(payload)
        return await scheduler.submit(
            modelID: modelID, capability: capability, payload: seededPayload
        ) {
            runner.run(record, capability, payload: seededPayload)
        }
    }

    public func job(id: String) async throws -> Job? {
        try await scheduler.job(id: id)
    }

    public func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        await scheduler.events(id: id)
    }

    public func cancel(jobID: String) async {
        await scheduler.cancel(jobID)
    }

    public func jobHistory() async throws -> [Job] {
        try await scheduler.history.list()
    }

    public func activeJobs() async -> [Job] {
        await scheduler.active()
    }

    public func artifacts() async throws -> [Artifact] {
        try await artifactStore.list()
    }

    public func artifact(id: String) async throws -> Artifact? {
        try await artifactStore.get(id: id)
    }

    public func artifactURL(id: String) async throws -> URL? {
        try await artifactStore.url(id: id)
    }

    public func artifactPreview(id: String) async throws -> Data? {
        try await artifactStore.previewData(id: id)
    }

    public func deleteArtifact(id: String) async throws {
        try await artifactStore.delete(id: id)
    }

    public func rerun(artifactID: String) async throws -> String {
        guard let artifact = try await artifactStore.get(id: artifactID) else {
            throw KernelError.artifactNotFound(artifactID)
        }
        return try await submit(artifact.modelID, artifact.capability, payload: artifact.params)
    }

    public func vary(artifactID: String) async throws -> String {
        guard let artifact = try await artifactStore.get(id: artifactID) else {
            throw KernelError.artifactNotFound(artifactID)
        }
        return try await submit(
            artifact.modelID, artifact.capability, payload: Self.reseeded(artifact.params))
    }

    private static func seeded(_ payload: JSONValue) -> JSONValue {
        guard var fields = seedableFields(payload) else { return payload }
        if let seed = fields["seed"], seed != .null { return .object(fields) }
        fields["seed"] = .int(randomSeed())
        return .object(fields)
    }

    private static func reseeded(_ params: JSONValue) -> JSONValue {
        guard var fields = seedableFields(params) else { return params }
        let previous = fields["seed"]
        var fresh = JSONValue.int(randomSeed())
        while fresh == previous {
            fresh = .int(randomSeed())
        }
        fields["seed"] = fresh
        return .object(fields)
    }

    private static func seedableFields(_ payload: JSONValue) -> [String: JSONValue]? {
        switch payload {
        case .object(let fields): fields
        case .null: [:]
        default: nil
        }
    }

    private static func randomSeed() -> Int {
        Int.random(in: 0..<Int(UInt32.max))
    }
}
