import Foundation

public actor InstallService {
    static let diskHeadroom = 1.05
    static let terminalHistoryLimit = 64

    private let providersByID: [InstallProviderID: any InstallProvider]
    private let orderedProviders: [any InstallProvider]
    private let diskProbeRoot: URL
    private let freeDiskBytes: @Sendable (URL) -> Int64?

    private var installs: [String: ActiveInstall] = [:]
    private var phases: [String: InstallEvent] = [:]
    private var terminal: [String: InstallEvent] = [:]
    private var terminalOrder: [String] = []
    private var tasks: [String: Task<Void, Never>] = [:]
    private var pumps: [String: Task<Void, Never>] = [:]
    private var cancelRequests: Set<String> = []
    private var inFlight: [String: String] = [:]
    private var subscribers: [String: [UUID: AsyncStream<InstallEvent>.Continuation]] = [:]
    private var completionSubscribers: [UUID: AsyncStream<Set<SourceKind>>.Continuation] = [:]

    public init(
        providers: [any InstallProvider],
        diskProbeRoot: URL = FileManager.default.homeDirectoryForCurrentUser,
        freeDiskBytes: @escaping @Sendable (URL) -> Int64? = DiskSpace.availableBytes(at:)
    ) {
        self.orderedProviders = providers
        self.providersByID = Dictionary(
            providers.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        self.diskProbeRoot = diskProbeRoot
        self.freeDiskBytes = freeDiskBytes
    }

    public func providers() async -> [InstallProviderStatus] {
        var statuses: [InstallProviderStatus] = []
        for provider in orderedProviders {
            statuses.append(
                InstallProviderStatus(
                    id: provider.id,
                    displayName: provider.displayName,
                    sourceKind: provider.sourceKind,
                    supportsSearch: provider.supportsSearch,
                    availability: await provider.availability()))
        }
        return statuses
    }

    public func search(
        provider id: InstallProviderID, matching query: String, limit: Int = 30
    ) async throws -> [InstallSearchHit] {
        try await requireAvailable(id).search(matching: query, limit: limit)
    }

    public static func ollamaDirectReference(for query: String) -> String? {
        guard InstallReference.huggingFaceRepo(from: query) == nil,
            let tag = InstallReference.ollamaTag(from: query),
            query.contains(":") || InstallReference.isOllamaLink(query)
        else { return nil }
        return tag
    }

    public func browse(matching rawQuery: String, limit: Int = 30) async -> InstallBrowseResult {
        let query = rawQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty, Self.ollamaDirectReference(for: query) == nil else {
            return InstallBrowseResult()
        }
        let repo = InstallReference.huggingFaceRepo(from: query)
        do {
            var hits = try await search(
                provider: .huggingface, matching: repo ?? query, limit: limit)
            if let repo,
                InstallReference.isHuggingFaceLink(query) || hits.isEmpty,
                !hits.contains(where: { $0.reference.lowercased() == repo.lowercased() })
            {
                hits.insert(Self.exactHit(repo), at: 0)
            }
            return InstallBrowseResult(hits: hits)
        } catch is CancellationError {
            return InstallBrowseResult()
        } catch {
            if let repo {
                return InstallBrowseResult(hits: [Self.exactHit(repo)])
            }
            return InstallBrowseResult(failureHint: error.localizedDescription)
        }
    }

    private static func exactHit(_ repo: String) -> InstallSearchHit {
        InstallSearchHit(
            provider: .huggingface,
            reference: repo,
            name: repo.split(separator: "/").last.map(String.init) ?? repo)
    }

    public func plan(
        provider id: InstallProviderID, reference: String
    ) async throws -> InstallPlan {
        try await requireAvailable(id).plan(reference: reference)
    }

    public func begin(_ plan: InstallPlan) throws -> String {
        guard let provider = providersByID[plan.provider] else {
            throw InstallError.providerUnknown(plan.provider)
        }
        let flightKey = "\(plan.provider.rawValue)|\(plan.reference)"
        if let existing = inFlight[flightKey] {
            return existing
        }
        if let pendingBytes = plan.remainingBytes ?? plan.totalBytes {
            let scaled = Double(max(0, pendingBytes)) * Self.diskHeadroom
            let required = scaled >= Double(Int64.max) ? Int64.max : Int64(scaled)
            let available = freeDiskBytes(diskProbeRoot) ?? .max
            guard available >= required else {
                throw InstallError.insufficientDisk(
                    requiredBytes: required, availableBytes: available)
            }
        }
        let id = "in-" + UUID().uuidString.lowercased()
        installs[id] = ActiveInstall(
            id: id, provider: plan.provider, reference: plan.reference,
            displayName: plan.displayName, totalBytes: plan.totalBytes)
        phases[id] = .queued
        inFlight[flightKey] = id
        emit(id, .queued)
        tasks[id] = Task { await self.run(id: id, plan: plan, provider: provider) }
        return id
    }

    public func events(id: String) -> AsyncStream<InstallEvent> {
        AsyncStream { continuation in
            if let ended = terminal[id] {
                continuation.yield(ended)
                continuation.finish()
                return
            }
            guard let install = installs[id] else {
                continuation.finish()
                return
            }
            if let phase = phases[id] {
                continuation.yield(phase)
            }
            if install.progress.bytesDownloaded > 0 || install.progress.totalBytes != nil {
                continuation.yield(.progress(install.progress))
            }
            let token = UUID()
            subscribers[id, default: [:]][token] = continuation
            continuation.onTermination = { _ in
                Task { [weak self] in await self?.dropSubscriber(id, token: token) }
            }
        }
    }

    public func active() -> [ActiveInstall] {
        installs.values.sorted { ($0.startedAt, $0.id) < ($1.startedAt, $1.id) }
    }

    public func cancel(_ installID: String) {
        if let pump = pumps[installID] {
            pump.cancel()
        } else if installs[installID] != nil {
            cancelRequests.insert(installID)
        }
    }

    public func completions() -> AsyncStream<Set<SourceKind>> {
        let id = UUID()
        return AsyncStream { continuation in
            completionSubscribers[id] = continuation
            continuation.onTermination = { _ in
                Task { [weak self] in await self?.dropCompletionSubscriber(id) }
            }
        }
    }

    private func requireAvailable(_ id: InstallProviderID) async throws -> any InstallProvider {
        guard let provider = providersByID[id] else {
            throw InstallError.providerUnknown(id)
        }
        if case .unavailable(let hint) = await provider.availability() {
            throw InstallError.providerUnavailable(hint: hint)
        }
        return provider
    }

    private func run(id: String, plan: InstallPlan, provider: any InstallProvider) async {
        transition(id, to: .preparing)
        let (events, feed) = AsyncThrowingStream<InstallStreamEvent, Error>.makeStream()
        let inner = provider.install(plan)
        let pump = Task {
            do {
                for try await event in inner {
                    feed.yield(event)
                }
                if Task.isCancelled {
                    feed.finish(throwing: CancellationError())
                } else {
                    feed.finish()
                }
            } catch is CancellationError {
                feed.finish(throwing: CancellationError())
            } catch {
                feed.finish(throwing: Task.isCancelled ? CancellationError() : error)
            }
        }
        pumps[id] = pump
        if cancelRequests.remove(id) != nil {
            pump.cancel()
        }
        do {
            for try await event in events {
                switch event {
                case .status(let message):
                    transition(id, to: .status(message))
                case .progress(let progress):
                    applyProgress(id, progress)
                }
            }
            conclude(id, plan: plan, provider: provider, as: .done)
        } catch is CancellationError {
            conclude(id, plan: plan, provider: provider, as: .cancelled)
        } catch {
            conclude(
                id, plan: plan, provider: provider,
                as: .failed(message: error.localizedDescription))
        }
    }

    private func transition(_ id: String, to event: InstallEvent) {
        phases[id] = event
        emit(id, event)
    }

    private func applyProgress(_ id: String, _ progress: InstallProgress) {
        guard var install = installs[id] else { return }
        install.progress = progress
        installs[id] = install
        emit(id, .progress(progress))
    }

    private func conclude(
        _ id: String, plan: InstallPlan, provider: any InstallProvider, as event: InstallEvent
    ) {
        guard installs[id] != nil else { return }
        emit(id, event)
        terminal[id] = event
        terminalOrder.append(id)
        if terminalOrder.count > Self.terminalHistoryLimit {
            terminal[terminalOrder.removeFirst()] = nil
        }
        finishSubscribers(id)
        installs[id] = nil
        phases[id] = nil
        tasks[id] = nil
        pumps[id] = nil
        cancelRequests.remove(id)
        inFlight["\(plan.provider.rawValue)|\(plan.reference)"] = nil
        for continuation in completionSubscribers.values {
            continuation.yield([provider.sourceKind])
        }
    }

    private func emit(_ id: String, _ event: InstallEvent) {
        guard let continuations = subscribers[id] else { return }
        for continuation in continuations.values {
            continuation.yield(event)
        }
    }

    private func finishSubscribers(_ id: String) {
        guard let continuations = subscribers.removeValue(forKey: id) else { return }
        for continuation in continuations.values {
            continuation.finish()
        }
    }

    private func dropSubscriber(_ id: String, token: UUID) {
        subscribers[id]?[token] = nil
    }

    private func dropCompletionSubscriber(_ id: UUID) {
        completionSubscribers[id] = nil
    }
}
