import Foundation

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

    private func scanSynchronously() -> ScanResult {
        let fm = FileManager.default
        let manifests = root.appendingPathComponent("manifests")

        var result = ScanResult()
        guard fm.fileExists(atPath: root.path) else { return result }
        guard fm.isReadableFile(atPath: root.path) else {
            result.failedKinds = [.ollama]
            return result
        }
        guard fm.fileExists(atPath: manifests.path) else { return result }
        guard fm.isReadableFile(atPath: manifests.path),
            let enumerator = fm.enumerator(
                at: manifests, includingPropertiesForKeys: [.isRegularFileKey],
                options: [.skipsHiddenFiles, .producesRelativePathURLs])
        else {
            result.failedKinds = [.ollama]
            return result
        }

        for case let url as URL in enumerator {
            guard (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
            else { continue }
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
                func blobPath(_ layer: Manifest.Layer) -> String {
                    root.appendingPathComponent("blobs")
                        .appendingPathComponent(
                            layer.digest.replacingOccurrences(of: ":", with: "-"))
                        .path
                }
                let weightBlob = manifest.layers
                    .first { $0.mediaType.hasSuffix(".model") }
                    .map(blobPath)
                let templateLayer = manifest.layers.first { $0.mediaType.hasSuffix(".template") }
                let hasProjector = manifest.layers.contains {
                    $0.mediaType.hasSuffix(".projector")
                }
                let profile = Identification.ollamaProfile(
                    hasProjector: hasProjector, blobPath: weightBlob)

                var contextLengthHint: Int?
                var stopTokensHint: [String]?
                if let paramsLayer = manifest.layers.first(where: {
                    $0.mediaType.hasSuffix(".params")
                }) {
                    if let blob = FileManager.default.contents(atPath: blobPath(paramsLayer)),
                        let object = try? JSONSerialization.jsonObject(with: blob)
                            as? [String: Any]
                    {
                        contextLengthHint = object["num_ctx"] as? Int
                        stopTokensHint = object["stop"] as? [String]
                    } else {
                        result.issues.append("ollama: unreadable params blob for \(name)")
                    }
                }

                result.discovered.append(
                    DiscoveredModel(
                        name: name,
                        source: ModelSource(kind: .ollama, path: url.path, repo: name),
                        modalityHint: profile.modality,
                        capabilitiesHint: profile.capabilities,
                        executionHint: profile.execution,
                        footprintBytes: footprint,
                        primaryWeightPath: weightBlob,
                        contextLengthHint: contextLengthHint,
                        hasChatTemplateHint: templateLayer != nil ? true : nil,
                        stopTokensHint: stopTokensHint))
            } catch {
                result.issues.append("ollama: unreadable manifest \(url.lastPathComponent): \(error.localizedDescription)")
            }
        }
        return result
    }
}
