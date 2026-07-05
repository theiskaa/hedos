import Foundation

/// Reads Ollama's on-disk store directly — no daemon required. Layout
/// (verified against a real store):
/// `<root>/manifests/<registry>/<namespace>/<model>/<tag>` is a JSON
/// manifest whose layers carry media types and sizes; weights live in the
/// shared `<root>/blobs/sha256-<digest>` pool. Weights are never touched.
public struct OllamaStoreScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.ollama] }
    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    private struct Manifest: Decodable {
        struct Layer: Decodable {
            let mediaType: String
            let size: Int64
            let digest: String
        }
        let layers: [Layer]
    }

    public func scan() async -> ScanResult {
        scanSynchronously()
    }

    // NSEnumerator iteration is unavailable in async contexts.
    private func scanSynchronously() -> ScanResult {
    let fm = FileManager.default
        let manifests = root.appendingPathComponent("manifests")
        guard fm.fileExists(atPath: manifests.path) else { return ScanResult() }

        var result = ScanResult()
        // producesRelativePathURLs sidesteps /var vs /private/var prefix
        // mismatches that break string-based relative-path math.
        guard
            let enumerator = fm.enumerator(
                at: manifests, includingPropertiesForKeys: [.isRegularFileKey],
                options: [.skipsHiddenFiles, .producesRelativePathURLs])
        else { return result }

        for case let url as URL in enumerator {
            guard (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
            else { continue }
            // Relative shape: <registry>/<namespace>/<model>/<tag>
            let rel = url.relativePath.split(separator: "/")
            guard rel.count == 4 else { continue }
            let namespace = String(rel[1])
            let model = String(rel[2])
            let tag = String(rel[3])

            do {
                let manifest = try JSONDecoder().decode(Manifest.self, from: Data(contentsOf: url))
                let name =
                    namespace == "library"
                    ? "\(model):\(tag)" : "\(namespace)/\(model):\(tag)"
                let footprint = manifest.layers.reduce(Int64(0)) { $0 + $1.size }
                let weightBlob = manifest.layers
                    .first { $0.mediaType.hasSuffix(".model") }
                    .map {
                        root.appendingPathComponent("blobs")
                            .appendingPathComponent($0.digest.replacingOccurrences(of: ":", with: "-"))
                            .path
                    }
                result.discovered.append(
                    DiscoveredModel(
                        name: name,
                        source: ModelSource(kind: .ollama, path: url.path, repo: name),
                        modalityHint: .text,
                        capabilitiesHint: [.chat, .complete],
                        executionHint: .stream,
                        footprintBytes: footprint,
                        primaryWeightPath: weightBlob))
            } catch {
                result.issues.append("ollama: unreadable manifest \(url.lastPathComponent): \(error.localizedDescription)")
            }
        }
        return result
    }
}
