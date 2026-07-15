import CryptoKit
import Foundation

struct HFCacheLayout: Sendable, Hashable {
    let root: URL
    let repo: String

    var repoDirectory: URL {
        root.appendingPathComponent(
            "models--" + repo.replacingOccurrences(of: "/", with: "--"))
    }

    var blobsDirectory: URL {
        repoDirectory.appendingPathComponent("blobs")
    }

    var refsDirectory: URL {
        repoDirectory.appendingPathComponent("refs")
    }

    func snapshotDirectory(revision: String) -> URL {
        repoDirectory.appendingPathComponent("snapshots").appendingPathComponent(revision)
    }

    func snapshotFile(revision: String, path: String) -> URL {
        var url = snapshotDirectory(revision: revision)
        for component in path.split(separator: "/") {
            url.appendPathComponent(String(component))
        }
        return url
    }

    func blobURL(named name: String) -> URL {
        blobsDirectory.appendingPathComponent(name)
    }

    func incompleteURL(named name: String) -> URL {
        blobsDirectory.appendingPathComponent(name + ".incomplete")
    }

    func relativeBlobTarget(path: String, blobName: String) -> String {
        let depth = path.split(separator: "/").count + 1
        return String(repeating: "../", count: depth) + "blobs/" + blobName
    }
}

struct HFCacheWriter: Sendable {
    static let hashChunkBytes = 1 << 20

    let layout: HFCacheLayout
    let transport: any InstallTransport

    func prepareSkeleton(revision: String, firstWeightPendingName: String?) throws {
        let files = FileManager.default
        try files.createDirectory(at: layout.blobsDirectory, withIntermediateDirectories: true)
        try files.createDirectory(
            at: layout.snapshotDirectory(revision: revision), withIntermediateDirectories: true)
        try files.createDirectory(at: layout.refsDirectory, withIntermediateDirectories: true)
        try Data(revision.utf8).write(
            to: layout.refsDirectory.appendingPathComponent("main"), options: .atomic)
        if let firstWeightPendingName {
            let pending = layout.incompleteURL(named: firstWeightPendingName)
            if !files.fileExists(atPath: pending.path) {
                files.createFile(atPath: pending.path, contents: nil)
            }
        }
    }

    static func pendingBlobName(for sibling: HFSibling) -> String {
        if let sha = sibling.sha256 { return sha }
        let digest = SHA256.hash(data: Data(sibling.rfilename.utf8))
        return "tmp-" + digest.map { String(format: "%02x", $0) }.joined()
    }

    func download(
        sibling: HFSibling, revision: String, request: URLRequest,
        onBytes: @escaping @Sendable (Int64) -> Void
    ) async throws {
        let files = FileManager.default
        let pendingName = Self.pendingBlobName(for: sibling)
        if let sha = sibling.sha256, files.fileExists(atPath: layout.blobURL(named: sha).path) {
            try link(path: sibling.rfilename, revision: revision, blobName: sha)
            onBytes(sibling.bytes ?? 0)
            return
        }
        let incomplete = layout.incompleteURL(named: pendingName)
        var hasher = SHA256()
        var written: Int64 = 0
        if files.fileExists(atPath: incomplete.path) {
            written = try hashExisting(at: incomplete, into: &hasher)
            if written > 0 {
                onBytes(written)
            }
        } else {
            files.createFile(atPath: incomplete.path, contents: nil)
        }
        var request = request
        if written > 0 {
            request.setValue("bytes=\(written)-", forHTTPHeaderField: "Range")
        }
        let (chunks, http) = try await transport.stream(request)
        switch http.statusCode {
        case 200:
            if written > 0 {
                try Data().write(to: incomplete)
                hasher = SHA256()
                onBytes(-written)
                written = 0
            }
        case 206:
            break
        case 401, 403:
            throw InstallError.authRequired(layout.repo)
        case 404:
            throw InstallError.transferFailed(
                "\(sibling.rfilename) is missing from \(layout.repo)")
        default:
            throw InstallError.transferFailed(
                "hugging face returned HTTP \(http.statusCode) for \(sibling.rfilename)")
        }
        let handle = try FileHandle(forWritingTo: incomplete)
        defer { try? handle.close() }
        try handle.seekToEnd()
        for try await chunk in chunks {
            try Task.checkCancellation()
            try handle.write(contentsOf: chunk)
            hasher.update(data: chunk)
            written += Int64(chunk.count)
            onBytes(Int64(chunk.count))
        }
        try handle.close()
        if let expected = sibling.bytes, written != expected {
            throw InstallError.transferFailed(
                "\(sibling.rfilename) ended after \(written) of \(expected) bytes")
        }
        let digest = hasher.finalize().map { String(format: "%02x", $0) }.joined()
        if let sha = sibling.sha256, digest != sha {
            try? files.removeItem(at: incomplete)
            throw InstallError.checksumMismatch(file: sibling.rfilename)
        }
        let finalName = sibling.sha256 ?? digest
        let blob = layout.blobURL(named: finalName)
        if files.fileExists(atPath: blob.path) {
            try files.removeItem(at: incomplete)
        } else {
            try files.moveItem(at: incomplete, to: blob)
        }
        try link(path: sibling.rfilename, revision: revision, blobName: finalName)
    }

    func removeStrayIncompletes() throws {
        let files = FileManager.default
        let entries = try files.contentsOfDirectory(
            at: layout.blobsDirectory, includingPropertiesForKeys: nil)
        for entry in entries where entry.lastPathComponent.hasSuffix(".incomplete") {
            try files.removeItem(at: entry)
        }
    }

    func removeRepo() {
        try? FileManager.default.removeItem(at: layout.repoDirectory)
    }

    func hasCompletedWeightBlob(minimumBytes: Int64) -> Bool {
        let files = FileManager.default
        guard
            let entries = try? files.contentsOfDirectory(
                at: layout.blobsDirectory, includingPropertiesForKeys: [.fileSizeKey])
        else { return false }
        return entries.contains { entry in
            guard !entry.lastPathComponent.hasSuffix(".incomplete") else { return false }
            let size = (try? entry.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
            return Int64(size) >= minimumBytes
        }
    }

    private func link(path: String, revision: String, blobName: String) throws {
        let files = FileManager.default
        let destination = layout.snapshotFile(revision: revision, path: path)
        try files.createDirectory(
            at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
        if files.fileExists(atPath: destination.path)
            || (try? files.destinationOfSymbolicLink(atPath: destination.path)) != nil
        {
            try? files.removeItem(at: destination)
        }
        try files.createSymbolicLink(
            atPath: destination.path,
            withDestinationPath: layout.relativeBlobTarget(path: path, blobName: blobName))
    }

    private func hashExisting(at url: URL, into hasher: inout SHA256) throws -> Int64 {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var total: Int64 = 0
        while let chunk = try handle.read(upToCount: Self.hashChunkBytes), !chunk.isEmpty {
            hasher.update(data: chunk)
            total += Int64(chunk.count)
        }
        return total
    }
}
