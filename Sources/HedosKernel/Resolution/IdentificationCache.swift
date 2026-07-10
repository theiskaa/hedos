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
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path),
            let mtime = attributes[.modificationDate] as? Date,
            let size = (attributes[.size] as? NSNumber)?.int64Value
        else {
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
}
