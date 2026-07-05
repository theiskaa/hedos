import Foundation

/// Reads the Hugging Face Hub cache in place. Layout (verified):
/// `<hub>/models--<org>--<repo>/` with `refs/main` naming the current
/// revision, `snapshots/<revision>/` holding symlinked files, and a
/// per-repo `blobs/` directory holding the real bytes.
public struct HFCacheScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.huggingfaceCache] }
    public let roots: [URL]

    public init(roots: [URL]) {
        self.roots = roots
    }

    public init(root: URL) {
        self.roots = [root]
    }

    /// ALL candidate cache locations, not first-match: the `HF_HUB_CACHE` /
    /// `HF_HOME` overrides AND the standard `~/.cache/huggingface/hub`.
    /// A stale env override must never hide the real cache — completeness
    /// is the product promise.
    public static func defaultRoots(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> [URL] {
        var candidates: [URL] = []
        if let cache = environment["HF_HUB_CACHE"], !cache.isEmpty {
            candidates.append(URL(fileURLWithPath: (cache as NSString).expandingTildeInPath))
        }
        if let home = environment["HF_HOME"], !home.isEmpty {
            candidates.append(
                URL(fileURLWithPath: (home as NSString).expandingTildeInPath)
                    .appendingPathComponent("hub"))
        }
        candidates.append(
            FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".cache/huggingface/hub"))

        var seen = Set<String>()
        return candidates.filter { seen.insert($0.standardizedFileURL.path).inserted }
    }

    public func scan() async -> ScanResult {
        var result = ScanResult()
        for root in roots {
            scanRoot(root, into: &result)
        }
        return result
    }

    private func scanRoot(_ root: URL, into result: inout ScanResult) {
        let fm = FileManager.default
        guard
            let entries = try? fm.contentsOfDirectory(
                at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { return }

        for repoDir in entries where repoDir.lastPathComponent.hasPrefix("models--") {
            let repo = repoDir.lastPathComponent
                .dropFirst("models--".count)
                .replacingOccurrences(of: "--", with: "/")

            guard let snapshot = currentSnapshot(in: repoDir) else {
                result.issues.append("hf-cache: \(repo) has no usable snapshot")
                continue
            }

            var diagnostics: [String] = []
            let files =
                (try? fm.contentsOfDirectory(
                    at: snapshot.url, includingPropertiesForKeys: nil,
                    options: [.skipsHiddenFiles])) ?? []
            let fileNames = Set(files.map(\.lastPathComponent))

            var hint = ModalityHints.Hint(modality: nil, capabilities: [], execution: .sync)
            if fileNames.contains("model_index.json") {
                hint = ModalityHints.fromModelIndex(
                    at: snapshot.url.appendingPathComponent("model_index.json"))
            } else if fileNames.contains("config.json") {
                if let configHint = ModalityHints.fromConfigJSON(
                    at: snapshot.url.appendingPathComponent("config.json"))
                {
                    hint = configHint
                }
            } else {
                diagnostics.append("no config.json or model_index.json in snapshot")
            }
            if hint.modality == .text,
                !fileNames.contains(where: { $0.hasPrefix("tokenizer") || $0 == "vocab.json" })
            {
                diagnostics.append("no tokenizer found")
            }

            result.discovered.append(
                DiscoveredModel(
                    name: String(repo.split(separator: "/").last ?? Substring(repo)),
                    source: ModelSource(
                        kind: .huggingfaceCache, path: repoDir.path, repo: repo,
                        ref: snapshot.revision),
                    modalityHint: hint.modality,
                    capabilitiesHint: hint.capabilities,
                    executionHint: hint.execution,
                    footprintBytes: directoryBytes(repoDir.appendingPathComponent("blobs")),
                    primaryWeightPath: largestWeight(in: snapshot.url),
                    diagnostics: diagnostics))
        }
    }

    private func currentSnapshot(in repoDir: URL) -> (url: URL, revision: String)? {
        let fm = FileManager.default
        let snapshots = repoDir.appendingPathComponent("snapshots")

        if let revision = try? String(
            contentsOf: repoDir.appendingPathComponent("refs/main"), encoding: .utf8)
        {
            let trimmed = revision.trimmingCharacters(in: .whitespacesAndNewlines)
            let url = snapshots.appendingPathComponent(trimmed)
            if fm.fileExists(atPath: url.path) { return (url, trimmed) }
        }
        // Fallback: newest snapshot directory.
        let candidates =
            (try? fm.contentsOfDirectory(
                at: snapshots, includingPropertiesForKeys: [.contentModificationDateKey],
                options: [.skipsHiddenFiles])) ?? []
        let newest = candidates.max { a, b in
            let da = (try? a.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            let db = (try? b.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            return da < db
        }
        return newest.map { ($0, $0.lastPathComponent) }
    }

    private func directoryBytes(_ dir: URL) -> Int64 {
        let fm = FileManager.default
        guard
            let enumerator = fm.enumerator(
                at: dir, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey])
        else { return 0 }
        var total: Int64 = 0
        for case let url as URL in enumerator {
            let values = try? url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
            if values?.isRegularFile == true { total += Int64(values?.fileSize ?? 0) }
        }
        return total
    }

    /// Largest safetensors in the snapshot, symlinks resolved so the path
    /// points at real bytes (what dedup compares).
    private func largestWeight(in snapshot: URL) -> String? {
        let fm = FileManager.default
        let files =
            (try? fm.contentsOfDirectory(at: snapshot, includingPropertiesForKeys: nil)) ?? []
        var best: (path: String, size: Int64)?
        for file in files where file.pathExtension == "safetensors" {
            let resolved = file.resolvingSymlinksInPath()
            let size =
                Int64((try? resolved.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0)
            if size > (best?.size ?? -1) { best = (resolved.path, size) }
        }
        return best?.path
    }
}
