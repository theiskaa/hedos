import CryptoKit
import Foundation

public actor ArtifactStore {
    public let root: URL

    private var artifacts: [String: Artifact] = [:]
    private var loaded = false

    public init(root: URL) {
        self.root = root
    }

    public func store(_ draft: ArtifactDraft) throws -> Artifact {
        try loadIfNeeded()
        let createdAt = Date(
            timeIntervalSince1970: (Date().timeIntervalSince1970 * 1000).rounded() / 1000)
        let hash = Self.hex(SHA256.hash(data: draft.data))
        let slug = Self.slug(draft.model)
        let year = Self.year(of: createdAt)
        let path = "\(year)/\(slug)_\(hash.prefix(12)).\(draft.fileExtension)"
        try writeIfAbsent(draft.data, at: path)
        let previewPath = try draft.preview.map { try spill($0) }
        let artifact = Artifact(
            id: uniqueID(slug: slug, hash: hash, jobID: draft.jobID),
            path: path,
            contentHash: hash,
            previewPath: previewPath,
            model: draft.model,
            modelID: draft.modelID,
            runtime: draft.runtime,
            capability: draft.capability,
            params: draft.params,
            createdAt: createdAt,
            durationMs: draft.durationMs,
            jobID: draft.jobID)
        try writeSidecar(artifact)
        artifacts[artifact.id] = artifact
        return artifact
    }

    public func list() throws -> [Artifact] {
        try loadIfNeeded()
        return artifacts.values.sorted {
            ($0.createdAt, $0.id) > ($1.createdAt, $1.id)
        }
    }

    public func get(id: String) throws -> Artifact? {
        try loadIfNeeded()
        return artifacts[id]
    }

    public func url(id: String) throws -> URL? {
        try loadIfNeeded()
        return artifacts[id].map { root.appendingPathComponent($0.path) }
    }

    public func previewData(id: String) throws -> Data? {
        try loadIfNeeded()
        guard let previewPath = artifacts[id]?.previewPath else { return nil }
        return try Data(contentsOf: root.appendingPathComponent(previewPath))
    }

    public func delete(id: String) throws {
        try loadIfNeeded()
        guard let artifact = artifacts[id] else {
            throw KernelError.artifactNotFound(id)
        }
        try trash(sidecarURL(for: artifact))
        artifacts[id] = nil
        if !artifacts.values.contains(where: { $0.path == artifact.path }) {
            try trash(root.appendingPathComponent(artifact.path))
        }
        if let previewPath = artifact.previewPath,
            !artifacts.values.contains(where: { $0.previewPath == previewPath })
        {
            try trash(root.appendingPathComponent(previewPath))
        }
    }

    private func writeIfAbsent(_ data: Data, at path: String) throws {
        let url = root.appendingPathComponent(path)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard !FileManager.default.fileExists(atPath: url.path) else { return }
        try data.write(to: url, options: .atomic)
    }

    private func spill(_ preview: Data) throws -> String {
        let hash = Self.hex(SHA256.hash(data: preview))
        let path = "blobs/\(hash)"
        try writeIfAbsent(preview, at: path)
        return path
    }

    private func uniqueID(slug: String, hash: String, jobID: String) -> String {
        let base = "\(slug)_\(hash.prefix(12))_\(jobID.prefix(8).lowercased())"
        guard artifacts[base] != nil else { return base }
        var counter = 2
        while artifacts["\(base)-\(counter)"] != nil {
            counter += 1
        }
        return "\(base)-\(counter)"
    }

    private func sidecarURL(for artifact: Artifact) -> URL {
        root
            .appendingPathComponent(Self.year(of: artifact.createdAt), isDirectory: true)
            .appendingPathComponent("\(artifact.id).json")
    }

    private func writeSidecar(_ artifact: Artifact) throws {
        let url = sidecarURL(for: artifact)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .custom { date, encoder in
            var container = encoder.singleValueContainer()
            try container.encode(date.formatted(Self.dateFormat))
        }
        try encoder.encode(artifact).write(to: url, options: .atomic)
    }

    private func trash(_ url: URL) throws {
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        try FileManager.default.trashItem(at: url, resultingItemURL: nil)
    }

    private func loadIfNeeded() throws {
        guard !loaded else { return }
        let fileManager = FileManager.default
        guard fileManager.fileExists(atPath: root.path) else {
            loaded = true
            return
        }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let raw = try container.decode(String.self)
            guard
                let date = (try? Date(raw, strategy: Self.dateFormat))
                    ?? (try? Date(raw, strategy: .iso8601))
            else {
                throw DecodingError.dataCorruptedError(
                    in: container, debugDescription: "Unparseable date \(raw)")
            }
            return date
        }
        var scanned: [String: Artifact] = [:]
        for entry in try fileManager.contentsOfDirectory(
            at: root, includingPropertiesForKeys: nil)
        {
            guard Int(entry.lastPathComponent) != nil else { continue }
            var isDirectory: ObjCBool = false
            guard fileManager.fileExists(atPath: entry.path, isDirectory: &isDirectory),
                isDirectory.boolValue
            else { continue }
            for sidecar in try fileManager.contentsOfDirectory(
                at: entry, includingPropertiesForKeys: nil)
            where sidecar.pathExtension == "json" {
                guard let data = try? Data(contentsOf: sidecar),
                    let artifact = try? decoder.decode(Artifact.self, from: data)
                else { continue }
                scanned[artifact.id] = artifact
            }
        }
        artifacts = scanned
        loaded = true
    }

    private static func year(of date: Date) -> String {
        String(Calendar(identifier: .gregorian).component(.year, from: date))
    }

    private static func slug(_ name: String) -> String {
        let kept = name.lowercased().unicodeScalars.filter {
            CharacterSet.alphanumerics.contains($0)
        }
        let slug = String(String.UnicodeScalarView(kept))
        return slug.isEmpty ? "artifact" : String(slug.prefix(24))
    }

    private static func hex(_ digest: SHA256.Digest) -> String {
        digest.map { String(format: "%02x", $0) }.joined()
    }

    private static let dateFormat = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
}
