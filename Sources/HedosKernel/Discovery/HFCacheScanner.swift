import Foundation

public struct HFCacheScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.huggingfaceCache] }
    public let roots: [URL]
    public let userRoots: [URL]

    public init(roots: [URL], userRoots: [URL] = []) {
        self.roots = roots
        self.userRoots = userRoots
    }

    public init(root: URL) {
        self.init(roots: [root])
    }

    public static func defaultRoots(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        user: [String] = [],
        home: URL = FileManager.default.homeDirectoryForCurrentUser
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
        candidates.append(home.appendingPathComponent(".cache/huggingface/hub"))
        candidates.append(contentsOf: userRoots(user))

        var seen = Set<String>()
        return candidates.filter { seen.insert($0.standardizedFileURL.path).inserted }
    }

    public static func userRoots(_ paths: [String]) -> [URL] {
        var roots: [URL] = []
        for path in paths {
            let base = URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
            let candidates = [
                base.appendingPathComponent("hub"),
                base.appendingPathComponent("huggingface/hub"),
                base,
            ]
            let existing = candidates.filter(isHubDirectory)
            roots.append(contentsOf: existing.isEmpty ? [base] : existing)
        }
        var seen = Set<String>()
        return roots.filter { seen.insert($0.standardizedFileURL.path).inserted }
    }

    private static func isHubDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
            && isDirectory.boolValue
    }

    public func scan() async -> ScanResult {
        var result = ScanResult()
        for root in roots {
            scanRoot(root, required: false, into: &result)
        }
        for root in userRoots {
            scanRoot(root, required: true, into: &result)
        }
        return result
    }

    private func scanRoot(_ root: URL, required: Bool, into result: inout ScanResult) {
        let fm = FileManager.default
        guard fm.fileExists(atPath: root.path) else {
            if required { result.failedKinds.insert(.huggingfaceCache) }
            return
        }
        guard fm.isReadableFile(atPath: root.path),
            let entries = try? fm.contentsOfDirectory(
                at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else {
            result.failedKinds.insert(.huggingfaceCache)
            return
        }

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
            } else if fileNames.contains(where: { $0.lowercased().hasSuffix(".gguf") }) {
                hint = ModalityHints.gguf
            } else {
                diagnostics.append("no config.json or model_index.json in snapshot")
            }
            if hint.modality == nil || hint.modality == .text,
                fileNames.contains(where: {
                    Identification.sentenceTransformersMarkers.contains($0)
                })
            {
                var embedding = ModalityHints.embeddingHint
                embedding.contextLength = hint.contextLength
                hint = embedding
            }
            if hint.modality == .text,
                !fileNames.contains(where: { $0.hasPrefix("tokenizer") || $0 == "vocab.json" })
            {
                diagnostics.append("no tokenizer found")
            }

            let downloading =
                hasIncompleteBlobs(in: repoDir.appendingPathComponent("blobs"))
                || indexReferencesMissingShard(snapshot: snapshot.url, fileNames: fileNames)
                || ggufShardsIncomplete(snapshot: snapshot.url, fileNames: fileNames)

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
                    diagnostics: diagnostics,
                    contextLengthHint: hint.contextLength,
                    downloading: downloading))
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

    private func hasIncompleteBlobs(in blobs: URL) -> Bool {
        let fm = FileManager.default
        guard
            let entries = try? fm.contentsOfDirectory(
                at: blobs, includingPropertiesForKeys: [.isRegularFileKey])
        else { return false }
        return entries.contains { url in
            url.lastPathComponent.hasSuffix(".incomplete")
                && (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
        }
    }

    private func ggufShardsIncomplete(snapshot: URL, fileNames: Set<String>) -> Bool {
        let ggufs = fileNames.filter { $0.lowercased().hasSuffix(".gguf") }
            .map { (url: snapshot.appendingPathComponent($0), bytes: Int64(0)) }
        return GGUFShards.group(ggufs).groups.contains { !$0.complete }
    }

    private func indexReferencesMissingShard(snapshot: URL, fileNames: Set<String>) -> Bool {
        let fm = FileManager.default
        for indexName in fileNames where indexName.hasSuffix(".safetensors.index.json") {
            guard
                let data = try? Data(
                    contentsOf: snapshot.appendingPathComponent(indexName)),
                let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                let weightMap = json["weight_map"] as? [String: Any]
            else { continue }
            for shard in Set(weightMap.values.compactMap({ $0 as? String })) {
                let resolved = snapshot.appendingPathComponent(shard).resolvingSymlinksInPath()
                if !fm.fileExists(atPath: resolved.path) { return true }
            }
        }
        return false
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

    private func largestWeight(in snapshot: URL) -> String? {
        let fm = FileManager.default
        let files =
            (try? fm.contentsOfDirectory(at: snapshot, includingPropertiesForKeys: nil)) ?? []
        var best: (url: URL, size: Int64)?
        for file in files where isWeightFile(file) {
            let size =
                Int64(
                    (try? file.resolvingSymlinksInPath().resourceValues(forKeys: [.fileSizeKey]))?
                        .fileSize ?? 0)
            if size > (best?.size ?? -1) { best = (file, size) }
        }
        guard let best else { return nil }
        if let shard = GGUFShards.parse(best.url.lastPathComponent) {
            let firstName = GGUFShards.name(base: shard.base, index: 1, total: shard.total)
            if files.contains(where: { $0.lastPathComponent == firstName }) {
                return snapshot.appendingPathComponent(firstName).resolvingSymlinksInPath().path
            }
        }
        return best.url.resolvingSymlinksInPath().path
    }

    private func isWeightFile(_ url: URL) -> Bool {
        guard !Identification.isMmprojName(url.lastPathComponent) else { return false }
        switch url.pathExtension.lowercased() {
        case "safetensors", "gguf": return true
        case "bin": return Identification.hasGGMLMagic(at: url)
        default: return false
        }
    }
}
