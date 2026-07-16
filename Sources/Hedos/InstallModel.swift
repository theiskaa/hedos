import Foundation
import HedosKernel

@Observable
@MainActor
final class InstallModel {
    let kernel: Kernel

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    var providers: [InstallProviderStatus] = []
    var searchQuery = ""
    var searchHits: [InstallSearchHit] = []
    var searching = false
    var searchError: String?
    var stagedPlan: InstallPlan?
    var stagingID: String?
    var stageError: String?
    var active: [ActiveInstall] = []
    var progressByID: [String: InstallProgress] = [:]
    var statusByID: [String: String] = [:]
    var failures: [String: String] = [:]
    var completed: Set<String> = []

    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var watchers: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var watcherTokens: [String: UUID] = [:]
    @ObservationIgnored private var referenceByInstallID: [String: String] = [:]
    @ObservationIgnored private var canonicalReferences: [String: String] = [:]
    @ObservationIgnored var recordsProvider: () -> [ModelRecord] = { [] }

    var catalog: [InstallCatalogEntry] { InstallCatalog.entries }

    func load() async {
        providers = await kernel.installs.providers()
        reconcileCompleted()
        await refreshActive()
        for install in active where watchers[install.id] == nil {
            watch(install.id, provider: install.provider, reference: install.reference)
        }
    }

    func reconcileCompleted() {
        guard !completed.isEmpty else { return }
        completed = completed.filter { key in
            guard let separator = key.firstIndex(of: "|") else { return false }
            let provider = InstallProviderID(rawValue: String(key[..<separator]))
            let reference = String(key[key.index(after: separator)...])
            return !onShelf(provider: provider, reference: reference)
        }
    }

    func installed(provider: InstallProviderID, reference: String) -> Bool {
        let resolved = resolvedReference(provider: provider, reference)
        return onShelf(provider: provider, reference: reference)
            || onShelf(provider: provider, reference: resolved)
            || completed.contains(Self.referenceKey(provider: provider, reference))
            || completed.contains(Self.referenceKey(provider: provider, resolved))
    }

    func availability(of provider: InstallProviderID) -> InstallAvailability? {
        providers.first { $0.id == provider }?.availability
    }

    func isAvailable(_ provider: InstallProviderID) -> Bool {
        availability(of: provider) == .ready
    }

    func activeInstall(provider: InstallProviderID, reference: String) -> ActiveInstall? {
        let resolved = resolvedReference(provider: provider, reference)
        let wanted = InstallReference.normalized(provider: provider, reference: resolved)
        return active.first { install in
            install.provider == provider
                && InstallReference.normalized(
                    provider: provider, reference: install.reference) == wanted
        }
    }

    func failure(provider: InstallProviderID, reference: String) -> String? {
        failures[reference] ?? failures[resolvedReference(provider: provider, reference)]
    }

    static func referenceKey(provider: InstallProviderID, _ reference: String) -> String {
        provider.rawValue + "|"
            + InstallReference.normalized(provider: provider, reference: reference)
    }

    private func resolvedReference(
        provider: InstallProviderID, _ reference: String
    ) -> String {
        canonicalReferences[Self.referenceKey(provider: provider, reference)] ?? reference
    }

    private func rememberCanonical(
        provider: InstallProviderID, typed: String, canonical: String
    ) {
        guard typed != canonical else { return }
        canonicalReferences[Self.referenceKey(provider: provider, typed)] = canonical
    }

    func progress(installID: String) -> InstallProgress? {
        progressByID[installID]
    }

    func progress(for record: ModelRecord) -> InstallProgress? {
        let install = active.first { install in
            switch install.provider {
            case .huggingface:
                record.source.kind == .huggingfaceCache
                    && (record.source.repo ?? "").lowercased()
                        == install.reference.lowercased()
            case .ollama:
                record.source.kind == .ollama
                    && InstallReference.normalizedTag(record.name)
                        == InstallReference.normalizedTag(install.reference)
            default:
                false
            }
        }
        guard let install else { return nil }
        return progressByID[install.id] ?? install.progress
    }

    func searchDebounced() {
        searchTask?.cancel()
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty, InstallService.ollamaDirectReference(for: query) == nil else {
            searchHits = []
            searching = false
            searchError = nil
            return
        }
        searching = true
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self else { return }
            let result = await kernel.installs.browse(matching: query)
            guard !Task.isCancelled else { return }
            self.searchHits = result.hits
            self.searchError = result.failureHint
            self.searching = false
        }
    }

    func stage(provider: InstallProviderID, reference: String) async {
        let stageID = "\(provider.rawValue)|\(reference)"
        stagingID = stageID
        stageError = nil
        do {
            let plan = try await kernel.installs.plan(provider: provider, reference: reference)
            rememberCanonical(provider: provider, typed: reference, canonical: plan.reference)
            guard stagingID == stageID else { return }
            stagedPlan = plan
        } catch {
            guard stagingID == stageID else { return }
            stageError = error.localizedDescription
        }
        stagingID = nil
    }

    func stage(entry: InstallCatalogEntry) async {
        await stage(provider: entry.provider, reference: entry.reference)
    }

    func discardStagedPlan() {
        stagedPlan = nil
        stageError = nil
        stagingID = nil
    }

    func dismissFailure(reference: String) {
        failures[reference] = nil
    }

    @discardableResult
    func confirm(_ plan: InstallPlan) async -> Bool {
        guard !plan.requiresAuth else {
            let message = InstallError.authRequired(plan.reference).localizedDescription
            stageError = message
            failures[plan.reference] = message
            return false
        }
        do {
            let id = try await kernel.installs.begin(plan)
            failures[plan.reference] = nil
            completed.remove(Self.referenceKey(provider: plan.provider, plan.reference))
            referenceByInstallID[id] = plan.reference
            watch(id, provider: plan.provider, reference: plan.reference)
            await refreshActive()
            if stagedPlan == plan {
                stagedPlan = nil
            }
            return true
        } catch {
            stageError = error.localizedDescription
            failures[plan.reference] = error.localizedDescription
            return false
        }
    }

    @discardableResult
    func install(provider: InstallProviderID, reference: String) async -> Bool {
        do {
            let plan = try await kernel.installs.plan(provider: provider, reference: reference)
            rememberCanonical(provider: provider, typed: reference, canonical: plan.reference)
            guard !plan.requiresAuth else {
                failures[reference] =
                    InstallError.authRequired(plan.reference).localizedDescription
                return false
            }
            return await confirm(plan)
        } catch {
            failures[reference] = error.localizedDescription
            return false
        }
    }

    func sourceKind(of provider: InstallProviderID) -> SourceKind {
        providers.first { $0.id == provider }?.sourceKind
            ?? (provider == .ollama ? .ollama : .huggingfaceCache)
    }

    func onShelf(provider: InstallProviderID, reference: String) -> Bool {
        let records = recordsProvider()
        switch provider {
        case .ollama:
            let tag = InstallReference.normalizedTag(reference)
            return records.contains {
                $0.source.kind == .ollama && $0.state != .missing
                    && InstallReference.normalizedTag($0.name) == tag
            }
        case .huggingface:
            return records.contains {
                $0.source.kind == .huggingfaceCache && $0.state != .missing
                    && ($0.source.repo ?? "").lowercased() == reference.lowercased()
            }
        default:
            return false
        }
    }

    var aggregateProgress: InstallProgress? {
        guard !active.isEmpty else { return nil }
        var downloaded: Int64 = 0
        var total: Int64 = 0
        var totalKnown = true
        for install in active {
            let progress = progressByID[install.id] ?? install.progress
            let (downloadedSum, downloadedOverflow) = downloaded.addingReportingOverflow(
                max(0, progress.bytesDownloaded))
            downloaded = downloadedOverflow ? .max : downloadedSum
            if let totalBytes = progress.totalBytes ?? install.totalBytes,
                !progress.totalIsPartial
            {
                let (totalSum, totalOverflow) = total.addingReportingOverflow(
                    max(0, totalBytes))
                total = totalOverflow ? .max : totalSum
            } else {
                totalKnown = false
            }
        }
        return InstallProgress(
            bytesDownloaded: downloaded, totalBytes: totalKnown && total > 0 ? total : nil)
    }

    func cancel(installID: String) async {
        await kernel.installs.cancel(installID)
    }

    private func refreshActive() async {
        active = await kernel.installs.active()
    }

    private func watch(
        _ installID: String, provider: InstallProviderID, reference: String
    ) {
        watchers[installID]?.cancel()
        referenceByInstallID[installID] = reference
        let token = UUID()
        watcherTokens[installID] = token
        watchers[installID] = Task { [weak self] in
            guard let self else { return }
            let events = await self.kernel.installs.events(id: installID)
            for await event in events {
                guard self.watcherTokens[installID] == token else { return }
                switch event {
                case .queued, .preparing:
                    break
                case .status(let message):
                    self.statusByID[installID] = message
                case .progress(let progress):
                    self.progressByID[installID] = progress
                case .done:
                    self.completed.insert(Self.referenceKey(provider: provider, reference))
                case .failed(let message):
                    self.failures[reference] = message
                case .cancelled:
                    break
                }
                await self.refreshActive()
            }
            guard self.watcherTokens[installID] == token else { return }
            self.progressByID[installID] = nil
            self.statusByID[installID] = nil
            self.watchers[installID] = nil
            self.watcherTokens[installID] = nil
            await self.refreshActive()
        }
    }
}
