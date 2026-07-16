import Foundation
import Synchronization

final class InstallProgressMeter: Sendable {
    static let emitBytes: Int64 = 8 << 20
    static let emitInterval: Duration = .milliseconds(300)

    private struct State {
        var downloaded: Int64 = 0
        var currentFile: String?
        var lastEmittedBytes: Int64 = 0
        var lastEmittedAt: ContinuousClock.Instant = .now
    }

    private let totalBytes: Int64?
    private let state: Mutex<State>

    init(totalBytes: Int64?) {
        self.totalBytes = totalBytes
        self.state = Mutex(State())
    }

    func begin(file: String) -> InstallProgress {
        state.withLock { state in
            state.currentFile = file
            state.lastEmittedBytes = state.downloaded
            state.lastEmittedAt = .now
            return snapshot(state)
        }
    }

    func add(_ delta: Int64) -> InstallProgress? {
        state.withLock { state in
            state.downloaded += delta
            let now = ContinuousClock.Instant.now
            guard
                state.downloaded - state.lastEmittedBytes >= Self.emitBytes
                    || state.lastEmittedAt.duration(to: now) >= Self.emitInterval
            else { return nil }
            state.lastEmittedBytes = state.downloaded
            state.lastEmittedAt = now
            return snapshot(state)
        }
    }

    func finish() -> InstallProgress {
        state.withLock { state in
            state.currentFile = nil
            return snapshot(state)
        }
    }

    private func snapshot(_ state: State) -> InstallProgress {
        InstallProgress(
            bytesDownloaded: max(state.downloaded, 0),
            totalBytes: totalBytes,
            currentFile: state.currentFile)
    }
}

struct HuggingFaceInstallProvider: InstallProvider {
    static let tokenAccount = "huggingface"
    static let weightKeepThreshold: Int64 = 10 << 20

    let id = InstallProviderID.huggingface
    let displayName = "Hugging Face"
    let sourceKind = SourceKind.huggingfaceCache
    let supportsSearch = true

    private let rootProvider: @Sendable () async -> URL
    private let api: HFHubAPI
    private let transport: any InstallTransport
    private let home: URL

    init(
        root: @escaping @Sendable () async -> URL,
        transport: any InstallTransport = URLSessionInstallTransport(),
        tokenProvider: @escaping @Sendable () -> String? = { nil },
        baseURL: URL = HFHubAPI.defaultBaseURL,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        self.rootProvider = root
        self.transport = transport
        self.api = HFHubAPI(baseURL: baseURL, transport: transport, token: tokenProvider)
        self.home = home
    }

    init(
        root: URL,
        transport: any InstallTransport = URLSessionInstallTransport(),
        tokenProvider: @escaping @Sendable () -> String? = { nil },
        baseURL: URL = HFHubAPI.defaultBaseURL,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        self.init(
            root: { root }, transport: transport, tokenProvider: tokenProvider,
            baseURL: baseURL, home: home)
    }

    func availability() async -> InstallAvailability {
        .ready
    }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        try await api.search(matching: query, limit: limit)
    }

    func plan(reference: String) async throws -> InstallPlan {
        let typed = try Self.repoShaped(reference)
        let info = try await api.modelInfo(repo: typed)
        let repo = info.repo
        let selection = HFFileSelection.select(siblings: info.siblings)
        guard selection.contains(where: HFFileSelection.isWeight) else {
            throw InstallError.transferFailed(
                "\(repo) has no model weights hedos knows how to download.")
        }
        let root = await rootProvider()
        let layout = HFCacheLayout(root: root, repo: repo)
        let sizes = selection.compactMap(\.bytes)
        let totalBytes = sizes.isEmpty ? nil : sizes.saturatingSum()
        return InstallPlan(
            provider: id,
            reference: repo,
            displayName: repo.split(separator: "/").last.map(String.init) ?? repo,
            revision: info.sha,
            files: selection.map { InstallPlanFile(path: $0.rfilename, bytes: $0.bytes) },
            totalBytes: totalBytes,
            remainingBytes: totalBytes.map {
                max(
                    $0
                        - Self.presentBytes(
                            selection: selection, layout: layout, revision: info.sha ?? ""),
                    0)
            },
            destination: displayPath(root),
            requiresAuth: info.gated && api.token() == nil)
    }

    static func presentBytes(
        selection: [HFSibling], layout: HFCacheLayout, revision: String
    ) -> Int64 {
        let files = FileManager.default
        return selection.reduce(0) { present, sibling in
            if let sha = sibling.sha256,
                files.fileExists(atPath: layout.blobURL(named: sha).path)
            {
                return present.addingClamped(max(0, sibling.bytes ?? 0))
            }
            let pending = layout.incompleteURL(
                named: HFCacheWriter.pendingBlobName(for: sibling, revision: revision))
            let size =
                (try? files.attributesOfItem(atPath: pending.path)[.size] as? Int64) ?? nil
            return present.addingClamped(max(0, size ?? 0))
        }
    }

    static func repoShaped(_ reference: String) throws -> String {
        guard let repo = InstallReference.huggingFaceRepo(from: reference) else {
            throw InstallError.referenceInvalid(reference)
        }
        return repo
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { continuation in
            let interruptionCleanup = Mutex<(@Sendable () -> Void)?>(nil)
            let task = Task {
                let root = await rootProvider()
                let layout = HFCacheLayout(root: root, repo: plan.reference)
                let writer = HFCacheWriter(layout: layout, transport: transport)
                let repoExistedBefore = FileManager.default.fileExists(
                    atPath: layout.repoDirectory.path)
                interruptionCleanup.withLock {
                    $0 = {
                        Self.cleanUpAfterInterruption(
                            writer: writer, repoExistedBefore: repoExistedBefore)
                    }
                }
                do {
                    continuation.yield(.status("Resolving \(plan.reference)"))
                    let info = try await api.modelInfo(repo: plan.reference)
                    guard let revision = info.sha ?? plan.revision else {
                        throw InstallError.transferFailed(
                            "\(plan.reference) has no resolvable revision.")
                    }
                    let selection = HFFileSelection.select(siblings: info.siblings)
                    guard selection.contains(where: HFFileSelection.isWeight) else {
                        throw InstallError.transferFailed(
                            "\(plan.reference) has no model weights hedos knows how to download.")
                    }
                    let ordered = Self.downloadOrder(selection)
                    let firstWeight = ordered.first(where: HFFileSelection.isWeight)
                    try writer.prepareSkeleton(
                        revision: revision,
                        firstWeightPendingName: firstWeight.map {
                            HFCacheWriter.pendingBlobName(for: $0, revision: revision)
                        })
                    try writer.removeStrayIncompletes(
                        keeping: Set(
                            ordered.map {
                                HFCacheWriter.pendingBlobName(for: $0, revision: revision)
                            }))
                    let sizes = ordered.compactMap(\.bytes)
                    let meter = InstallProgressMeter(
                        totalBytes: sizes.isEmpty ? nil : sizes.saturatingSum())
                    for sibling in ordered {
                        try Task.checkCancellation()
                        continuation.yield(.progress(meter.begin(file: sibling.rfilename)))
                        try await writer.download(
                            sibling: sibling, revision: revision,
                            request: api.resolveRequest(
                                repo: plan.reference, revision: revision,
                                path: sibling.rfilename)
                        ) { delta in
                            if let progress = meter.add(delta) {
                                continuation.yield(.progress(progress))
                            }
                        }
                    }
                    try writer.commitRef(revision: revision)
                    try writer.removeStrayIncompletes()
                    continuation.yield(.progress(meter.finish()))
                    continuation.finish()
                } catch is CancellationError {
                    Self.cleanUpAfterInterruption(
                        writer: writer, repoExistedBefore: repoExistedBefore)
                    continuation.finish()
                } catch let error as URLError {
                    Self.cleanUpAfterInterruption(
                        writer: writer, repoExistedBefore: repoExistedBefore)
                    continuation.finish(
                        throwing: InstallError.transferFailed(
                            "the download failed: \(error.localizedDescription)"))
                } catch {
                    Self.cleanUpAfterInterruption(
                        writer: writer, repoExistedBefore: repoExistedBefore)
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { termination in
                task.cancel()
                if case .cancelled = termination {
                    interruptionCleanup.withLock { $0 }?()
                }
            }
        }
    }

    static func downloadOrder(_ selection: [HFSibling]) -> [HFSibling] {
        selection.sorted { first, second in
            let firstWeight = HFFileSelection.isWeight(first)
            let secondWeight = HFFileSelection.isWeight(second)
            if firstWeight != secondWeight {
                return !firstWeight
            }
            if first.bytes != second.bytes {
                return (first.bytes ?? 0) < (second.bytes ?? 0)
            }
            return first.rfilename < second.rfilename
        }
    }

    static func cleanUpAfterInterruption(writer: HFCacheWriter, repoExistedBefore: Bool) {
        guard !repoExistedBefore else { return }
        guard writer.hasSubstantialProgress(minimumBytes: weightKeepThreshold) else {
            writer.removeRepo()
            return
        }
        if !writer.hasCompletedBlob() {
            writer.retreatToBlobsOnly()
        }
    }

    private func displayPath(_ url: URL) -> String {
        let path = url.standardizedFileURL.path
        let homePath = home.standardizedFileURL.path
        guard path == homePath || path.hasPrefix(homePath + "/") else { return path }
        return "~" + path.dropFirst(homePath.count)
    }
}
