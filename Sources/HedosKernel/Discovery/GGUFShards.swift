import Foundation

enum GGUFShards {
    struct Member: Hashable {
        let index: Int
        let url: URL
        let bytes: Int64
    }

    struct ShardGroup {
        let base: String
        let total: Int
        let members: [Member]

        var firstShard: URL? { members.first(where: { $0.index == 1 })?.url }
        var footprintBytes: Int64 { members.reduce(0) { $0 + $1.bytes } }
        var complete: Bool { members.count == total }
    }

    static func parse(_ filename: String) -> (base: String, index: Int, total: Int)? {
        guard filename.lowercased().hasSuffix(".gguf") else { return nil }
        let stem = String(filename.dropLast(".gguf".count))
        guard let ofRange = stem.range(of: "-of-", options: .backwards) else { return nil }
        let totalField = String(stem[ofRange.upperBound...])
        guard totalField.count == 5, let total = Int(totalField), total > 0 else { return nil }
        let head = stem[..<ofRange.lowerBound]
        guard let dashRange = head.range(of: "-", options: .backwards) else { return nil }
        let indexField = String(head[dashRange.upperBound...])
        guard indexField.count == 5, let index = Int(indexField), index > 0, index <= total
        else { return nil }
        let base = String(head[..<dashRange.lowerBound])
        guard !base.isEmpty else { return nil }
        return (base, index, total)
    }

    static func name(base: String, index: Int, total: Int) -> String {
        String(format: "%@-%05d-of-%05d.gguf", base, index, total)
    }

    static func group(_ files: [(url: URL, bytes: Int64)]) -> (
        groups: [ShardGroup], loose: [URL]
    ) {
        struct Key: Hashable {
            let directory: String
            let base: String
            let total: Int
        }

        var buckets: [Key: [Member]] = [:]
        var loose: [URL] = []
        for file in files {
            guard let shard = parse(file.url.lastPathComponent) else {
                loose.append(file.url)
                continue
            }
            let key = Key(
                directory: file.url.deletingLastPathComponent().path,
                base: shard.base, total: shard.total)
            buckets[key, default: []].append(
                Member(index: shard.index, url: file.url, bytes: file.bytes))
        }

        let groups = buckets.map { key, members in
            ShardGroup(
                base: key.base, total: key.total,
                members: members.sorted { $0.index < $1.index })
        }
        return (groups, loose)
    }

    static func discoveredModels(
        from files: [(url: URL, bytes: Int64)], kind: SourceKind, repo: (URL) -> String?
    ) -> (discovered: [DiscoveredModel], issues: [String]) {
        let (groups, loose) = group(files)
        let bytesByURL = Dictionary(files.map { ($0.url, $0.bytes) }, uniquingKeysWith: { a, _ in a })
        let hint = ModalityHints.gguf
        var discovered: [DiscoveredModel] = []
        var issues: [String] = []

        for url in loose {
            discovered.append(
                DiscoveredModel(
                    name: url.deletingPathExtension().lastPathComponent,
                    source: ModelSource(kind: kind, path: url.path, repo: repo(url)),
                    modalityHint: hint.modality,
                    capabilitiesHint: hint.capabilities,
                    executionHint: hint.execution,
                    footprintBytes: bytesByURL[url] ?? 0,
                    primaryWeightPath: url.path))
        }
        for shardGroup in groups {
            guard let first = shardGroup.firstShard else {
                issues.append(
                    "sharded model \(shardGroup.base) is missing its first part — skipped")
                continue
            }
            discovered.append(
                DiscoveredModel(
                    name: shardGroup.base,
                    source: ModelSource(kind: kind, path: first.path, repo: repo(first)),
                    modalityHint: hint.modality,
                    capabilitiesHint: hint.capabilities,
                    executionHint: hint.execution,
                    footprintBytes: shardGroup.footprintBytes,
                    primaryWeightPath: first.path,
                    downloading: !shardGroup.complete))
        }
        return (discovered, issues)
    }
}
