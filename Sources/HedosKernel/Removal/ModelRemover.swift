import Foundation

extension ModelRecord {
    public var isDeletable: Bool {
        source.kind != .builtin && source.kind != .endpoint
    }
}

public struct ModelDeletionPreview: Sendable, Hashable, Codable {
    public let modelID: String
    public let name: String
    public let kind: SourceKind
    public let paths: [String]
    public let bytesEstimate: Int64
    public let viaDaemon: Bool
    public let missing: Bool
}

public struct ModelDeletionReport: Sendable, Hashable, Codable {
    public let modelID: String
    public let name: String
    public let kind: SourceKind
    public let trashedPaths: [String]
    public let freedBytesEstimate: Int64
    public let daemonDeleted: Bool
}

struct ModelRemover: Sendable {
    typealias Trasher = @Sendable (URL) throws -> Void

    let trasher: Trasher
    let ollama: OllamaModelRemover

    static func systemTrasher() -> Trasher {
        { url in
            do {
                try FileManager.default.trashItem(at: url, resultingItemURL: nil)
            } catch {
                throw RemovalError.trashFailed(
                    path: url.path, reason: error.localizedDescription)
            }
        }
    }

    func preview(_ record: ModelRecord) -> ModelDeletionPreview {
        let missing = record.state == .missing
        let viaDaemon = !missing && record.source.kind == .ollama
        let paths =
            record.source.kind == .ollama ? [] : Self.removablePaths(for: record).map(\.path)
        return ModelDeletionPreview(
            modelID: record.id,
            name: record.displayName,
            kind: record.source.kind,
            paths: paths,
            bytesEstimate: missing
                ? Self.onDiskBytes(of: paths) : Int64(record.footprintMB ?? 0) << 20,
            viaDaemon: viaDaemon,
            missing: missing)
    }

    func remove(_ record: ModelRecord) async throws -> ModelDeletionReport {
        let details = preview(record)
        if details.viaDaemon {
            try await ollama.delete(tag: record.source.repo ?? record.name)
            return ModelDeletionReport(
                modelID: record.id, name: details.name, kind: details.kind,
                trashedPaths: [], freedBytesEstimate: details.bytesEstimate,
                daemonDeleted: true)
        }
        var trashed: [String] = []
        for path in details.paths {
            try trasher(URL(fileURLWithPath: path))
            trashed.append(path)
        }
        return ModelDeletionReport(
            modelID: record.id, name: details.name, kind: details.kind,
            trashedPaths: trashed, freedBytesEstimate: details.bytesEstimate,
            daemonDeleted: false)
    }

    static func removablePaths(for record: ModelRecord) -> [URL] {
        let files = FileManager.default
        let source = URL(fileURLWithPath: record.source.path)
        switch record.source.kind {
        case .huggingfaceCache, .folder:
            return files.fileExists(atPath: source.path) ? [source] : []
        case .lmStudio, .file:
            return shardGroup(around: source)
        default:
            return []
        }
    }

    private static func shardGroup(around url: URL) -> [URL] {
        let files = FileManager.default
        guard let shard = GGUFShards.parse(url.lastPathComponent) else {
            return files.fileExists(atPath: url.path) ? [url] : []
        }
        let directory = url.deletingLastPathComponent()
        guard
            let entries = try? files.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil)
        else {
            return []
        }
        return entries
            .filter { entry in
                guard let candidate = GGUFShards.parse(entry.lastPathComponent) else {
                    return false
                }
                return candidate.base == shard.base && candidate.total == shard.total
            }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    private static func onDiskBytes(of paths: [String]) -> Int64 {
        let files = FileManager.default
        return paths.reduce(0) { total, path in
            let size = (try? files.attributesOfItem(atPath: path)[.size] as? Int64) ?? nil
            return total.addingClamped(max(0, size ?? 0))
        }
    }
}
