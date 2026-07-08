import CryptoKit
import Foundation

public struct DuplicateGroup: Sendable, Hashable {
    public var names: [String]
    public var paths: [String]
    public var wastedBytes: Int64
}

public enum DuplicateDetector {
    public static let defaultThreshold: Int64 = 256 << 20

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
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        guard let size = try? handle.seekToEnd() else { return nil }
        let sampleSize: UInt64 = 1 << 20
        guard (try? handle.seek(toOffset: 0)) != nil else { return nil }
        var data: Data
        if size <= sampleSize * 2 {
            guard let whole = try? handle.read(upToCount: Int(size)) else { return nil }
            data = whole
        } else {
            guard let head = try? handle.read(upToCount: Int(sampleSize)) else { return nil }
            guard (try? handle.seek(toOffset: size - sampleSize)) != nil else { return nil }
            guard let tail = try? handle.read(upToCount: Int(sampleSize)) else { return nil }
            data = head
            data.append(tail)
        }
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
