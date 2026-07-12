import Foundation

public final class IdentificationCache: @unchecked Sendable {
    private struct Entry {
        var mtime: Date
        var size: Int64
        var identified: IdentifiedModel
    }

    private let lock = NSLock()
    private var entries: [String: Entry] = [:]
    private var hits = 0

    public init() {}

    private static let uncachedKinds: Set<SourceKind> = [.builtin, .endpoint, .ollama]

    var hitCount: Int {
        lock.withLock { hits }
    }

    public func identify(_ record: ModelRecord) -> IdentifiedModel {
        guard !Self.uncachedKinds.contains(record.source.kind) else {
            return Identification.identify(record)
        }
        let path = (record.source.path as NSString).expandingTildeInPath
        guard let (mtime, size) = Self.freshnessSignature(path) else {
            return Identification.identify(record)
        }
        let cached = lock.withLock { entries[path] }
        if let cached, cached.mtime == mtime, cached.size == size {
            lock.withLock { hits += 1 }
            return cached.identified
        }
        let identified = Identification.identify(record)
        lock.withLock {
            entries[path] = Entry(mtime: mtime, size: size, identified: identified)
        }
        return identified
    }

    static func freshnessSignature(_ path: String) -> (Date, Int64)? {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: path, isDirectory: &isDirectory),
            let attributes = try? fm.attributesOfItem(atPath: path),
            let mtime = attributes[.modificationDate] as? Date,
            let size = (attributes[.size] as? NSNumber)?.int64Value
        else { return nil }
        guard isDirectory.boolValue,
            let children = try? fm.contentsOfDirectory(atPath: path)
        else { return (mtime, size) }
        var latest = mtime
        var total = size
        for child in children {
            let childPath = (path as NSString).appendingPathComponent(child)
            guard let attributes = try? fm.attributesOfItem(atPath: childPath) else { continue }
            if let childMtime = attributes[.modificationDate] as? Date, childMtime > latest {
                latest = childMtime
            }
            if let childSize = (attributes[.size] as? NSNumber)?.int64Value {
                total = total &+ childSize
            }
        }
        return (latest, total)
    }
}
