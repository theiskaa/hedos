import Foundation

public enum KernelError: Error, Sendable, LocalizedError {
    case notImplemented(String)
    case modelNotFound(String)
    case artifactNotFound(String)
    case promptNotFound(String)
    case capabilityUnsupported(model: String, capability: Capability)
    case paramUnsupported(model: String, key: String)
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
        case .promptNotFound(let id):
            "No prompt with id \(id) is stored."
        case .capabilityUnsupported(let model, let capability):
            "\(model) has no runtime for \(capability.rawValue)."
        case .paramUnsupported(let model, let key):
            "\(model) has no \(key) option."
        case .runtimeUnavailable(let hint):
            hint
        case .runtimeFailed(let message):
            message
        }
    }
}

public actor Kernel {
    public static let version = "0.1.0"

    public nonisolated let directory: URL
    public let registry: Registry
    public let settings: SettingsStore
    public let governor: MemoryGovernor
    public let artifactStore: ArtifactStore
    public let chats: ChatStore
    public let promptStore: PromptStore
    public let pipelineStore: PipelineStore
    private let baseAdapters: [any RuntimeAdapter]
    private var adapters: [any RuntimeAdapter]
    let secrets: any SecretStore
    let habitat: ModelHabitat
    let scheduler: JobScheduler
    let gatewayClientStore: GatewayClientStore
    let gatewayAuditLog: GatewayAuditLog
    var gateway: GatewayServer?
    let vmHost: any VMHost
    private var shelfWatcher: ShelfWatcher?
    private var watcherTask: Task<Void, Never>?
    private var watcherDebounce: Duration = .seconds(2)
    private var lastSummary: DiscoverySummary?
    private var shelfSubscribers: [UUID: AsyncStream<DiscoverySummary>.Continuation] = [:]
    private let duplicateThreshold: Int64
    private let identificationCache = IdentificationCache()
    private var loadedManifests: [RuntimeManifest] = []

    public init(
        directory: URL,
        adapters: [any RuntimeAdapter]? = nil,
        governor: MemoryGovernor = .shared,
        secrets: any SecretStore = KeychainStore(),
        habitat: ModelHabitat = ModelHabitat(),
        vmHost: (any VMHost)? = nil,
        duplicateThreshold: Int64 = DuplicateDetector.defaultThreshold
    ) {
        self.habitat = habitat
        self.directory = directory
        self.duplicateThreshold = duplicateThreshold
        let registry = Registry(directory: directory)
        let artifactStore = ArtifactStore(
            root: directory.appendingPathComponent("outputs", isDirectory: true))
        self.registry = registry
        self.settings = SettingsStore(directory: directory)
        self.governor = governor
        self.artifactStore = artifactStore
        self.chats = ChatStore(databaseURL: directory.appendingPathComponent("chats.sqlite"))
        self.promptStore = PromptStore(
            directory: directory.appendingPathComponent("prompts", isDirectory: true))
        self.pipelineStore = PipelineStore(
            directory: directory.appendingPathComponent("pipelines", isDirectory: true))
        self.secrets = secrets
        let base =
            adapters ?? Self.defaultAdapters(governor: governor, secrets: secrets, registry: registry)
        self.baseAdapters = base
        self.adapters = base
        self.scheduler = JobScheduler(
            history: JobHistoryStore(directory: directory),
            admission: GovernorAdmission(governor: governor, registry: registry),
            artifacts: ProvenanceArtifactWriter(store: artifactStore, registry: registry))
        let gatewayDirectory = directory.appendingPathComponent("gateway", isDirectory: true)
        self.gatewayClientStore = GatewayClientStore(directory: gatewayDirectory, secrets: secrets)
        self.gatewayAuditLog = GatewayAuditLog(directory: gatewayDirectory)
        self.vmHost = vmHost ?? ContainerizationVMHost(directory: directory)
    }

    public init() {
        self.init(directory: Registry.defaultDirectory())
    }

    private static func defaultAdapters(
        governor: MemoryGovernor, secrets: any SecretStore, registry: Registry
    ) -> [any RuntimeAdapter] {
        [
            LlamaCppAdapter(governor: governor),
            WhisperCppAdapter(governor: governor),
            OllamaAdapter(),
            MlxAudioAdapter(governor: governor),
            MfluxAdapter(governor: governor),
            DiffusersAdapter(governor: governor),
            MlxSwiftAdapter(governor: governor),
            MlxLmAdapter(governor: governor),
            AppleFoundationAdapter(registry: registry),
            OpenAIEndpointAdapter(secrets: secrets, registry: registry),
        ]
    }

    public nonisolated func userRuntimesDirectory() -> URL {
        let dir = directory.appendingPathComponent("runtimes.d", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func reloadUserRuntimes() async -> [String] {
        let models = await settings.models()
        let reserved = Set(baseAdapters.map(\.id))
        let store = UserRuntimeStore(
            directory: directory.appendingPathComponent("runtimes.d", isDirectory: true))
        let (manifests, issues) = store.load(reservedIDs: reserved)
        var manifestAdapters: [any RuntimeAdapter] = []
        for manifest in manifests {
            let approvedNetwork = Self.isNetworkApproved(manifest, in: models)
            if manifest.vm != nil {
                manifestAdapters.append(VMCommandAdapter(manifest: manifest, host: vmHost))
            } else if manifest.serve != nil {
                manifestAdapters.append(
                    ManifestSidecarAdapter(
                        manifest: manifest, approvedNetwork: approvedNetwork,
                        governor: governor))
            } else {
                manifestAdapters.append(
                    ManifestCommandAdapter(
                        manifest: manifest, approvedNetwork: approvedNetwork))
            }
        }
        adapters = baseAdapters + manifestAdapters
        loadedManifests = manifests
        return issues
    }

    public func discover() async throws -> DiscoverySummary {
        await applyStoredPolicies()
        let manifestIssues = await reloadUserRuntimes()
        let models = await settings.models()
        let scanners = habitat.scanners(kinds: nil, models: models)
        var summary = try await DiscoveryService(
            scanners: scanners, duplicateThreshold: duplicateThreshold
        ).discover(into: registry)
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
        summary.issues.append(contentsOf: manifestIssues)
        lastSummary = summary
        await rearmWatcherIfActive()
        return summary
    }

    public func shelfUpdates() -> AsyncStream<DiscoverySummary> {
        let id = UUID()
        return AsyncStream { continuation in
            shelfSubscribers[id] = continuation
            continuation.onTermination = { _ in
                Task { await self.removeShelfSubscriber(id) }
            }
        }
    }

    private func removeShelfSubscriber(_ id: UUID) {
        shelfSubscribers[id] = nil
    }

    public func startWatching(debounce: Duration = .seconds(2)) async {
        await stopWatchingInternal()
        watcherDebounce = debounce
        let models = await settings.models()
        let watcher = ShelfWatcher(roots: habitat.roots(models: models), debounce: debounce)
        watcher.start()
        shelfWatcher = watcher
        watcherTask = Task { [weak self] in
            for await kinds in watcher.events {
                await self?.scopedRescan(kinds)
            }
        }
    }

    public func stopWatching() async {
        await stopWatchingInternal()
    }

    private func stopWatchingInternal() async {
        watcherTask?.cancel()
        watcherTask = nil
        shelfWatcher?.stop()
        shelfWatcher = nil
    }

    private func rearmWatcherIfActive() async {
        guard shelfWatcher != nil else { return }
        await startWatching(debounce: watcherDebounce)
    }

    func scopedRescan(_ kinds: Set<SourceKind>) async {
        guard lastSummary != nil else {
            if let full = try? await discover() {
                emitShelfUpdate(full)
            }
            return
        }
        let manifestsBefore = loadedManifests
        let manifestIssues = await reloadUserRuntimes()
        let models = await settings.models()
        let scanners = habitat.scanners(kinds: kinds, models: models)
        guard !scanners.isEmpty else { return }
        guard
            let partial = try? await DiscoveryService(
                scanners: scanners, duplicateThreshold: duplicateThreshold
            ).discover(into: registry)
        else { return }
        let affected = Set(scanners.flatMap(\.kinds))
        let resolutionScope: Set<SourceKind>? =
            loadedManifests == manifestsBefore ? affected : nil
        try? await ResolutionEngine(
            adapters: adapters, identificationCache: identificationCache
        ).resolveAll(in: registry, kinds: resolutionScope)

        let duplicates = await recomputeDuplicates()

        guard var summary = lastSummary else { return }
        for kind in affected where !partial.failedKinds.contains(kind) {
            summary.perKind[kind] = partial.perKind[kind]
        }
        summary.totalCount = summary.perKind.values.reduce(0) { $0 + $1.count }
        summary.totalBytes = summary.perKind.values.reduce(0) { $0 + $1.bytes }
        summary.duplicates = duplicates
        summary.issues.append(contentsOf: manifestIssues)
        lastSummary = summary
        emitShelfUpdate(summary)
    }

    private func recomputeDuplicates() async -> [DuplicateGroup] {
        guard let live = try? await registry.list() else { return [] }
        let models = live.filter { $0.state != .missing }.map { record in
            DiscoveredModel(
                name: record.name,
                source: record.source,
                footprintBytes: Int64(record.footprintMB ?? 0) * (1 << 20),
                primaryWeightPath: record.primaryWeightPath)
        }
        return DuplicateDetector.detect(in: models, threshold: duplicateThreshold)
    }

    private func emitShelfUpdate(_ summary: DiscoverySummary) {
        for continuation in shelfSubscribers.values {
            continuation.yield(summary)
        }
    }

    public func explainShelf() async throws -> [ResolutionExplanation] {
        try await ResolutionEngine(adapters: adapters).explainAll(in: registry)
    }

    public func watchedFolders() async throws -> [String] {
        await settings.models().watchedFolders
    }

    public func addWatchedFolder(_ path: String) async throws {
        _ = try await settings.addWatchedFolder(path)
        await rearmWatcherIfActive()
    }

    public func removeWatchedFolder(_ path: String) async throws {
        _ = try await settings.removeWatchedFolder(path)
        await rearmWatcherIfActive()
    }

    public func manifestTemplate(for modelID: String) async throws -> String {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        return ManifestTemplate.render(record: record, identified: Identification.identify(record))
    }

    public func pendingNetworkConsent(for modelID: String) async throws -> ManifestConsentInfo? {
        guard let record = try await registry.get(id: modelID) else { return nil }
        let models = await settings.models()
        let store = UserRuntimeStore(
            directory: directory.appendingPathComponent("runtimes.d", isDirectory: true))
        let (manifests, _) = store.load(reservedIDs: Set(baseAdapters.map(\.id)))
        for manifest in manifests
        where manifest.permissions.network && !Self.isNetworkApproved(manifest, in: models) {
            if manifest.detect?.matches(record) == true {
                return ManifestConsentInfo(id: manifest.id, paths: manifest.permissions.paths)
            }
        }
        return nil
    }

    static func isNetworkApproved(_ manifest: RuntimeManifest, in models: ModelsSettings) -> Bool {
        guard models.approvedNetworkRuntimes.contains(manifest.id) else { return false }
        return models.approvedNetworkRuntimeHashes[manifest.id] == manifest.contentHash
    }

    public func approveNetworkRuntime(_ id: String) async throws {
        let store = UserRuntimeStore(
            directory: directory.appendingPathComponent("runtimes.d", isDirectory: true))
        let (manifests, _) = store.load(reservedIDs: Set(baseAdapters.map(\.id)))
        let contentHash = manifests.first(where: { $0.id == id })?.contentHash
        _ = try await settings.approveNetworkRuntime(id, contentHash: contentHash)
        _ = await reloadUserRuntimes()
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
    }

    private var manifestInstaller: ManifestInstaller {
        ManifestInstaller(
            runtimesDirectory: directory.appendingPathComponent("runtimes.d", isDirectory: true),
            reservedIDs: Set(baseAdapters.map(\.id)))
    }

    public func previewRuntimeInstall(from source: URL) async throws -> RuntimeInstallPreview {
        try manifestInstaller.preview(from: source, vmAssetState: await vmHost.assetState())
    }

    public func installRuntime(from source: URL) async throws -> String {
        let id = try manifestInstaller.install(from: source)
        _ = await reloadUserRuntimes()
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
        return id
    }

    public func uninstallRuntime(id: String) async throws {
        try manifestInstaller.uninstall(id: id)
        _ = await reloadUserRuntimes()
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
    }

    public func installedRuntimes() async -> [RuntimeManifest] {
        let store = UserRuntimeStore(
            directory: directory.appendingPathComponent("runtimes.d", isDirectory: true))
        let (manifests, _) = store.load(reservedIDs: Set(baseAdapters.map(\.id)))
        return manifests.filter { $0.provenance?.isCommunity == true }
    }

    public func addServer(baseURL: String, apiKey: String?) async throws -> [String] {
        let base = OpenAIEndpointAdapter.normalizedBase(baseURL)
        let account = OpenAIEndpointAdapter.account(for: base)
        if let apiKey, !apiKey.isEmpty {
            try secrets.set(apiKey, account: account)
        }
        return try await OpenAIEndpointAdapter.listModels(baseURL: base, key: apiKey)
    }

    public func registerEndpoint(baseURL: String, model: String) async throws -> ModelRecord {
        let base = OpenAIEndpointAdapter.normalizedBase(baseURL)
        let record = ModelRecord(
            name: model,
            modality: .text,
            capabilities: [.chat, .complete],
            source: ModelSource(kind: .endpoint, path: base, repo: model),
            runtime: RuntimeRef(
                id: "generic:openai-server", resolved: .user, tier: .remote,
                confirmedAt: Date()),
            params: Identification.endpointParams,
            execution: .stream,
            state: .ready)
        try await registry.register(record)
        return record
    }

    public func removeEndpoint(_ modelID: String) async throws {
        guard let record = try await registry.get(id: modelID),
            record.source.kind == .endpoint
        else {
            throw KernelError.modelNotFound(modelID)
        }
        _ = try await registry.unregister(id: modelID)
        let siblings = try await registry.list().filter {
            $0.source.kind == .endpoint && $0.source.path == record.source.path
        }
        if siblings.isEmpty {
            try? secrets.delete(
                account: OpenAIEndpointAdapter.account(for: record.source.path))
        }
    }

    public func hfCacheRoots() async throws -> [String] {
        await settings.models().hfCacheRoots
    }

    public func addHFCacheRoot(_ path: String) async throws {
        _ = try await settings.addHFCacheRoot(path)
        await rearmWatcherIfActive()
    }

    public func removeHFCacheRoot(_ path: String) async throws {
        _ = try await settings.removeHFCacheRoot(path)
        await rearmWatcherIfActive()
    }

    public func shellState() async throws -> ShellState {
        await settings.shellState()
    }

    public func saveShellState(_ shell: ShellState) async throws {
        try await settings.saveShellState(shell)
    }

    public func defaultChatModelID() async throws -> String? {
        await settings.defaultChatModelID()
    }

    public func setDefaultChatModel(_ modelID: String?) async throws {
        try await settings.setDefaultChatModelID(modelID)
    }

    public func sendChat(sessionID: String, text: String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        try await chatFlow().send(sessionID: sessionID, text: text)
    }

    public func continueChat(sessionID: String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        try await chatFlow().continueSession(sessionID: sessionID)
    }

    public func autoTitleIfNeeded(sessionID: String) async throws -> String? {
        try await chatFlow().autoTitleIfNeeded(sessionID: sessionID)
    }

    public func editChatTurn(
        sessionID: String, turnID: String, text: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try await chatFlow().editUserTurn(sessionID: sessionID, turnID: turnID, text: text)
    }

    public func regenerateChatTurn(
        sessionID: String, turnID: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try await chatFlow().regenerate(sessionID: sessionID, turnID: turnID)
    }

    public func replaceSpokenArtifact(
        sessionID: String, turnID: String, artifactID: String
    ) async throws {
        guard let transcript = try await chats.session(id: sessionID),
            let turn = transcript.turns.first(where: { $0.id == turnID })
        else { throw ChatStoreError.turnNotFound(turnID) }
        var kept: [String] = []
        var retired: [String] = []
        for reference in turn.artifactRefs where reference != artifactID {
            if try await artifactStore.get(id: reference)?.capability == .speak {
                retired.append(reference)
            } else {
                kept.append(reference)
            }
        }
        var updated = turn
        updated.artifactRefs = kept + [artifactID]
        _ = try await chats.updateTurn(updated, mergingCapabilityTags: [SessionTag.spoke])
        for reference in retired {
            try? await artifactStore.delete(id: reference)
        }
    }

    public func recordGeneratedTurn(
        sessionID: String, prompt: String, artifactID: String, tag: String
    ) async throws {
        guard try await chats.session(id: sessionID) != nil else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        _ = try await chats.appendTurn(TurnDraft(role: .user, content: prompt), to: sessionID)
        _ = try await chats.appendTurn(
            TurnDraft(role: .assistant, content: "", artifactRefs: [artifactID]),
            to: sessionID,
            mergingCapabilityTags: [tag])
    }

    private func chatFlow() -> ChatFlow {
        ChatFlow(
            chats: chats,
            stream: { modelID, messages in
                try await self.chat(modelID, messages: messages)
            },
            shelf: {
                try await self.shelf()
            })
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
        return SpeechVoices.available(record)
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
        let fallback = capability == .chat ? try? await chatSettings().defaultSystemPrompt : nil
        let configured = ModelConfiguration.merged(
            record: record, capability: capability, payload: payload,
            fallbackPrompt: fallback ?? nil)
        return adapter.invoke(record, capability, payload: configured)
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
        let seededPayload = Self.seeded(
            ModelConfiguration.merged(
                record: record, capability: capability, payload: payload,
                fallbackPrompt: capability == .chat
                    ? ((try? await chatSettings().defaultSystemPrompt) ?? nil) : nil))
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

    public func saveSpeech(
        modelID: String, voice: String, text: String, sampleRate: Int, pcm: Data,
        sessionID: String? = nil
    ) async throws -> Artifact {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        let wav = SpeechAudio.wavData(fromFloat32: pcm, sampleRate: sampleRate)
        let peaks = SpeechAudio.peaks(fromFloat32: pcm)
        let draft = ArtifactDraft(
            data: wav,
            fileExtension: "wav",
            model: record.name,
            modelID: modelID,
            runtime: record.runtime.id ?? "",
            capability: .speak,
            params: .object([
                "text": .string(text),
                "voice": .string(voice),
                "peaks": .array(peaks.map { .double($0) }),
            ]),
            jobID: "voice-\(UUID().uuidString.lowercased())",
            durationMs: SpeechAudio.durationMs(fromFloat32: pcm, sampleRate: sampleRate),
            sessionID: sessionID)
        return try await artifactStore.store(draft)
    }

    public struct ResidentEntry: Hashable, Sendable {
        public enum Origin: Hashable, Sendable {
            case governor
            case ollama
        }

        public let modelID: String?
        public let name: String
        public let footprintMB: Int
        public let origin: Origin
    }

    public func residentModels() async -> [ResidentEntry] {
        var entries = await governor.resident().map { resident in
            ResidentEntry(
                modelID: resident.modelID, name: resident.name,
                footprintMB: resident.footprintMB, origin: .governor)
        }
        if let ollama = adapters.compactMap({ $0 as? OllamaAdapter }).first {
            entries += await ollama.loadedModels().map { resident in
                ResidentEntry(
                    modelID: nil, name: resident.name,
                    footprintMB: resident.sizeMB, origin: .ollama)
            }
        }
        return entries
    }

    public func artifacts() async throws -> [Artifact] {
        try await artifactStore.list()
    }

    public func artifactOwners() async throws -> [String: String] {
        try await chats.artifactOwners()
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
