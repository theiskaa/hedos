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
    @ObservationIgnored private var referenceByInstallID: [String: String] = [:]
    @ObservationIgnored var recordsProvider: () -> [ModelRecord] = { [] }

    var catalog: [InstallCatalogEntry] { InstallCatalog.entries }

    func load() async {
        providers = await kernel.installs.providers()
        await refreshActive()
        for install in active where watchers[install.id] == nil {
            watch(install.id, reference: install.reference)
        }
    }

    func availability(of provider: InstallProviderID) -> InstallAvailability? {
        providers.first { $0.id == provider }?.availability
    }

    func isAvailable(_ provider: InstallProviderID) -> Bool {
        availability(of: provider) == .ready
    }

    func activeInstall(reference: String) -> ActiveInstall? {
        active.first { $0.reference == reference }
    }

    func progress(installID: String) -> InstallProgress? {
        progressByID[installID]
    }

    func progress(for record: ModelRecord) -> InstallProgress? {
        let install = active.first { install in
            switch install.provider {
            case .huggingface:
                record.source.kind == .huggingfaceCache
                    && record.source.repo == install.reference
            case .ollama:
                record.source.kind == .ollama && record.name == install.reference
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
        guard !query.isEmpty else {
            searchHits = []
            searching = false
            searchError = nil
            return
        }
        searching = true
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self else { return }
            do {
                let hits = try await kernel.installs.search(
                    provider: .huggingface, matching: query)
                guard !Task.isCancelled else { return }
                self.searchHits = hits
                self.searchError = nil
            } catch is CancellationError {
            } catch {
                guard !Task.isCancelled else { return }
                self.searchHits = []
                self.searchError = error.localizedDescription
            }
            self.searching = false
        }
    }

    func stage(provider: InstallProviderID, reference: String) async {
        let stageID = "\(provider.rawValue)|\(reference)"
        stagingID = stageID
        stageError = nil
        do {
            let plan = try await kernel.installs.plan(provider: provider, reference: reference)
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
            completed.remove(plan.reference)
            referenceByInstallID[id] = plan.reference
            watch(id, reference: plan.reference)
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
            let tag = reference.contains(":") ? reference : reference + ":latest"
            return records.contains {
                $0.source.kind == .ollama && $0.state != .missing
                    && $0.name.lowercased() == tag.lowercased()
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
            downloaded += progress.bytesDownloaded
            if let totalBytes = progress.totalBytes ?? install.totalBytes {
                total += totalBytes
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

    private func watch(_ installID: String, reference: String) {
        watchers[installID]?.cancel()
        referenceByInstallID[installID] = reference
        watchers[installID] = Task { [weak self] in
            guard let self else { return }
            let events = await self.kernel.installs.events(id: installID)
            for await event in events {
                switch event {
                case .queued, .preparing:
                    break
                case .status(let message):
                    self.statusByID[installID] = message
                case .progress(let progress):
                    self.progressByID[installID] = progress
                case .done:
                    self.completed.insert(reference)
                case .failed(let message):
                    self.failures[reference] = message
                case .cancelled:
                    break
                }
                await self.refreshActive()
            }
            self.progressByID[installID] = nil
            self.statusByID[installID] = nil
            self.watchers[installID] = nil
            await self.refreshActive()
        }
    }
}
