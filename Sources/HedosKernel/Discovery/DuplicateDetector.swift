import CryptoKit
import Foundation

public struct DuplicateGroup: Sendable, Hashable {
    public var names: [String]
    public var paths: [String]
    public var wastedBytes: Int64
}

/// Finds the same weights living in more than one place. Candidates are
/// grouped by exact byte size of their primary weight file (only files at
/// or above the threshold — small files collide by size too easily), then
/// confirmed by hashing the first 1 MiB of each. Whole weights are never
/// hashed. Report-only: nothing is ever deleted.
public enum DuplicateDetector {
    public static let defaultThreshold: Int64 = 256 << 20  // 256 MB

    public static func detect(
        in models: [DiscoveredModel], threshold: Int64 = defaultThreshold
    ) -> [DuplicateGroup] {
        let fm = FileManager.default

        var bySize: [Int64: [(model: DiscoveredModel, path: String)]] = [:]
        for model in models {
            guard let path = model.primaryWeightPath,
                let size = (try? fm.attributesOfItem(atPath: path))?[.size] as? Int64,
                size >= threshold
            else { continue }
            bySize[size, default: []].append((model, path))
        }

        var groups: [DuplicateGroup] = []
        for (size, candidates) in bySize where candidates.count > 1 {
            var byPrefix: [String: [(model: DiscoveredModel, path: String)]] = [:]
            for candidate in candidates {
                guard let prefix = prefixHash(of: candidate.path) else { continue }
                byPrefix[prefix, default: []].append(candidate)
            }
            for (_, matches) in byPrefix where matches.count > 1 {
                groups.append(
                    DuplicateGroup(
                        names: matches.map(\.model.name).sorted(),
                        paths: matches.map(\.path).sorted(),
                        wastedBytes: Int64(matches.count - 1) * size))
            }
        }
        return groups.sorted { $0.wastedBytes > $1.wastedBytes }
    }

    private static func prefixHash(of path: String) -> String? {
        guard let handle = FileHandle(forReadingAtPath: path),
            let data = try? handle.read(upToCount: 1 << 20)
        else { return nil }
        try? handle.close()
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
