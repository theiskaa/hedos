import Foundation

public enum KernelError: Error, Sendable, LocalizedError {
    case notImplemented(String)
    case modelNotFound(String)
    case artifactNotFound(String)
    case pipelineNotFound(String)
    case capabilityUnsupported(model: String, capability: Capability)
    case paramUnsupported(model: String, key: String)
    case runtimeUnavailable(hint: String)
    case runtimeFailed(String)
    case contextExceeded(model: String)
    case bundleMissing(runtimeID: RuntimeID)
    case wrongExecutionMode(runtimeID: RuntimeID, expected: ExecutionMode)
    case noBoundModel
    case sidecarDied(runtimeID: String, detail: String)
    case payloadInvalid(String)

    public var errorDescription: String? {
        switch self {
        case .notImplemented(let what):
            "\(what) is not implemented yet."
        case .modelNotFound(let id):
            "No model with id \(id) is registered."
        case .artifactNotFound(let id):
            "No artifact with id \(id) is stored."
        case .pipelineNotFound(let id):
            "No pipeline with id \(id) is stored."
        case .capabilityUnsupported(let model, let capability):
            "\(model) has no runtime for \(capability.rawValue)."
        case .paramUnsupported(let model, let key):
            "\(model) has no \(key) option."
        case .runtimeUnavailable(let hint):
            hint
        case .runtimeFailed(let message):
            message
        case .contextExceeded(let model):
            "This conversation no longer fits \(model)'s context window. Start a new chat or switch to a model with a larger window."
        case .bundleMissing(let runtimeID):
            "The \(runtimeID) runtime bundle is missing."
        case .wrongExecutionMode(let runtimeID, let expected):
            expected == .job
                ? "\(runtimeID) runs as jobs, not streams."
                : "\(runtimeID) streams, it does not run jobs."
        case .noBoundModel:
            "No model is bound to this chat."
        case .sidecarDied(let runtimeID, let detail):
            "The \(runtimeID) sidecar \(detail)"
        case .payloadInvalid(let message):
            message
        }
    }
}

public struct ChatContextAssessment: Sendable, Hashable {
    public let estimatedTokens: Int
    public let window: Int
    public let fits: Bool
}

public actor Kernel {
    public nonisolated let directory: URL
    public let registry: Registry
    public let settings: SettingsStore
    public let governor: MemoryGovernor
    public let artifactStore: ArtifactStore
    public let chats: ChatStore
    public let promptStore: PromptStore
    public let pipelineStore: PipelineStore
    public nonisolated let runtimeCatalog: RuntimeCatalog
    private let baseAdapters: [any RuntimeAdapter]
    private var adapters: [any RuntimeAdapter]
    let secrets: any SecretStore
    let habitat: ModelHabitat
    public nonisolated let scheduler: JobScheduler
    public nonisolated let gatewayClientStore: GatewayClientStore
    public nonisolated let gatewayAuditLog: GatewayAuditLog
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
    private nonisolated(unsafe) var settingsReaction: Task<Void, Never>?
    private var knownWatchedFolders: [String]?
    private var knownHFCacheRoots: [String]?

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
            directory: directory.appendingPathComponent("pipelines", isDirectory: true),
            shelf: { try await registry.list() })
        self.secrets = secrets
        let base =
            adapters ?? Self.defaultAdapters(governor: governor, secrets: secrets, registry: registry)
        self.baseAdapters = base
        self.adapters = base
        self.runtimeCatalog = RuntimeCatalog(
            directory: directory.appendingPathComponent("runtimes.d", isDirectory: true),
            reservedIDs: Set(base.map(\.id.rawValue)))
        self.scheduler = JobScheduler(
            history: JobHistoryStore(directory: directory),
            admission: GovernorAdmission(governor: governor, registry: registry),
            artifacts: ProvenanceArtifactWriter(store: artifactStore, registry: registry))
        let gatewayDirectory = directory.appendingPathComponent("gateway", isDirectory: true)
        self.gatewayClientStore = GatewayClientStore(directory: gatewayDirectory, secrets: secrets)
        self.gatewayAuditLog = GatewayAuditLog(directory: gatewayDirectory)
        self.vmHost = vmHost ?? ContainerizationVMHost(directory: directory)
        let changes = self.settings.changes()
        settingsReaction = Task { [weak self] in
            for await domain in changes {
                guard let self else { return }
                await self.react(toSettingsDomain: domain)
            }
        }
    }

    deinit {
        settingsReaction?.cancel()
    }

    private func react(toSettingsDomain domain: String) async {
        switch domain {
        case ModelsSettings.domainName:
            await applyStoredPolicies()
            let models = await settings.models()
            if knownWatchedFolders != models.watchedFolders
                || knownHFCacheRoots != models.hfCacheRoots
            {
                knownWatchedFolders = models.watchedFolders
                knownHFCacheRoots = models.hfCacheRoots
                await rearmWatcherIfActive()
            }
        case AdvancedSettings.domainName:
            await applyStoredPolicies()
        default:
            break
        }
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

    private func reloadUserRuntimes() async -> [String] {
        let models = await settings.models()
        let (manifests, issues) = runtimeCatalog.load()
        let workdirRoot = directory.appendingPathComponent("workdirs", isDirectory: true)
        var manifestAdapters: [any RuntimeAdapter] = []
        for manifest in manifests {
            let approvedNetwork = Self.isNetworkApproved(manifest, in: models)
            if manifest.vm != nil {
                manifestAdapters.append(
                    VMCommandAdapter(
                        manifest: manifest, host: vmHost, governor: governor,
                        workdirRoot: workdirRoot))
            } else if manifest.serve != nil {
                manifestAdapters.append(
                    ManifestSidecarAdapter(
                        manifest: manifest, approvedNetwork: approvedNetwork,
                        governor: governor, workdirRoot: workdirRoot))
            } else {
                manifestAdapters.append(
                    ManifestCommandAdapter(
                        manifest: manifest, approvedNetwork: approvedNetwork,
                        governor: governor, workdirRoot: workdirRoot))
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

    public func suspendForQuit() async {
        await stopWatchingInternal()
        await stopGateway()
        await governor.suspendForQuit()
        await SidecarSupervisor.shared.terminateAll()
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

    public func pendingNetworkConsent(for modelID: String) async throws -> ManifestConsentInfo? {
        guard let record = try await registry.get(id: modelID) else { return nil }
        let models = await settings.models()
        let (manifests, _) = runtimeCatalog.load()
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
        let (manifests, _) = runtimeCatalog.load()
        let contentHash = manifests.first(where: { $0.id == id })?.contentHash
        _ = try await settings.approveNetworkRuntime(id, contentHash: contentHash)
        _ = await reloadUserRuntimes()
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
    }

    private var manifestInstaller: ManifestInstaller {
        ManifestInstaller(
            runtimesDirectory: runtimeCatalog.directory,
            reservedIDs: runtimeCatalog.reservedIDs)
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
                id: .openAIEndpoint, resolved: .user, tier: .remote,
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

    public func chatContextAssessment(
        sessionID: String, modelID: String
    ) async throws -> ChatContextAssessment? {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        guard
            let window = ContextBudget.effectiveWindow(
                for: record,
                requestedContextLength: ContextBudget.storedContextLength(of: record),
                adapter: adapters.first(where: { $0.id == record.runtime.id }))
        else { return nil }
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        let fallbackPrompt = await settings.chat().defaultSystemPrompt
        let systemPrompt = record.systemPrompt ?? fallbackPrompt
        let messages = ChatFlow.messages(from: transcript.turns)
        let characters =
            messages.reduce(0) { $0 + $1.content.count } + (systemPrompt?.count ?? 0)
        let fits: Bool =
            switch ContextBudget.assess(
                promptCharacters: characters, window: window, requestedMaxTokens: nil)
            {
            case .fits: true
            case .exceeds: false
            }
        return ChatContextAssessment(
            estimatedTokens: ContextBudget.estimatedTokens(characters: characters),
            window: window,
            fits: fits)
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

    public func overrideRuntime(_ modelID: String, to runtimeID: RuntimeID) async throws {
        guard var record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        let identified = Identification.identify(record)
        let bids = adapters.compactMap { adapter -> (id: RuntimeID, bid: RuntimeBid)? in
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
        if identified.params.isEmpty {
            record = ProfileRegistry.builtin.refreshed(record)
        }
        try await registry.register(record)
    }

    public func shelf() async throws -> [ModelRecord] {
        try await registry.list()
    }

    public func voices(for modelID: String) async throws -> [String] {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        return SpeechVoices.available(record)
    }

    public func startOllama() async throws {
        guard let adapter = adapters.compactMap({ $0 as? OllamaAdapter }).first else {
            throw KernelError.runtimeUnavailable(
                hint: "Ollama isn't available in this configuration.")
        }
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
        let fallback = capability == .chat ? await settings.chat().defaultSystemPrompt : nil
        var configured = ModelConfiguration.merged(
            record: record, capability: capability, payload: payload,
            fallbackPrompt: fallback)
        if capability == .chat || capability == .complete,
            case .object(var object) = configured,
            let window = ContextBudget.effectiveWindow(
                for: record, requestedContextLength: object["context_length"]?.intValue,
                adapter: adapter)
        {
            let requestedMaxTokens = object["max_tokens"]?.intValue
            let verdict = ContextBudget.assess(
                promptCharacters: ContextBudget.promptCharacters(of: configured),
                window: window,
                requestedMaxTokens: requestedMaxTokens)
            switch verdict {
            case .exceeds:
                throw KernelError.contextExceeded(model: record.displayName)
            case .fits(let clampedMaxTokens):
                if let clampedMaxTokens,
                    requestedMaxTokens != nil || clampedMaxTokens < 4096
                {
                    object["max_tokens"] = .int(clampedMaxTokens)
                    configured = .object(object)
                }
            }
        }
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
        let seededPayload = PayloadSeeding.seeded(
            ModelConfiguration.merged(
                record: record, capability: capability, payload: payload,
                fallbackPrompt: capability == .chat
                    ? await settings.chat().defaultSystemPrompt : nil))
        return await scheduler.submit(
            modelID: modelID, capability: capability, payload: seededPayload
        ) {
            runner.run(record, capability, payload: seededPayload)
        }
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
            runtime: record.runtime.id?.rawValue ?? "",
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
            artifact.modelID, artifact.capability, payload: PayloadSeeding.reseeded(artifact.params))
    }

}
