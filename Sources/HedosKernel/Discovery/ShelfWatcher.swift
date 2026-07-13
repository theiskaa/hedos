import CoreServices
import Foundation

struct HabitatMap: Sendable {
    let roots: [(kind: SourceKind, path: String)]

    init(roots: [(kind: SourceKind, url: URL)]) {
        self.roots = roots.map { ($0.kind, Self.canonicalRootPath($0.url)) }
    }

    static func canonicalRootPath(_ url: URL) -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        if let real = realpath(url.path, &buffer) {
            return String(cString: real)
        }
        var missing: [String] = []
        var current = url
        while current.pathComponents.count > 1 {
            missing.insert(current.lastPathComponent, at: 0)
            current = current.deletingLastPathComponent()
            if let real = realpath(current.path, &buffer) {
                var resolved = URL(fileURLWithPath: String(cString: real))
                for component in missing {
                    resolved = resolved.appendingPathComponent(component)
                }
                return resolved.path
            }
        }
        return url.path
    }

    func kinds(
        forEventPath eventPath: String,
        rootExists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }
    ) -> Set<SourceKind> {
        var path = eventPath
        while path.count > 1 && path.hasSuffix("/") {
            path = String(path.dropLast())
        }
        var kinds: Set<SourceKind> = []
        for root in roots {
            if path == root.path || path.hasPrefix(root.path + "/") {
                kinds.insert(root.kind)
                continue
            }
            if root.path.hasPrefix(path + "/") && rootExists(root.path) {
                kinds.insert(root.kind)
            }
        }
        return kinds
    }
}

final class ShelfWatcher: @unchecked Sendable {
    let events: AsyncStream<Set<SourceKind>>

    private let map: HabitatMap
    private let debounce: Duration
    private let queue = DispatchQueue(label: "hedos.shelf-watcher")
    private let continuation: AsyncStream<Set<SourceKind>>.Continuation
    private var stream: FSEventStreamRef?
    private var pending: [SourceKind: DispatchWorkItem] = [:]

    init(roots: [(kind: SourceKind, url: URL)], debounce: Duration = .seconds(2)) {
        self.map = HabitatMap(roots: roots)
        self.debounce = debounce
        (self.events, self.continuation) = AsyncStream.makeStream()
    }

    func start() {
        queue.sync {
            guard stream == nil else { return }
            let watchPaths = Self.watchPaths(for: map.roots.map(\.path))
            guard !watchPaths.isEmpty else { return }

            var context = FSEventStreamContext(
                version: 0,
                info: Unmanaged.passUnretained(self).toOpaque(),
                retain: nil, release: nil, copyDescription: nil)
            let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
                guard let info else { return }
                let watcher = Unmanaged<ShelfWatcher>.fromOpaque(info).takeUnretainedValue()
                guard
                    let eventPaths = Unmanaged<CFArray>.fromOpaque(paths)
                        .takeUnretainedValue() as? [String]
                else { return }
                watcher.handle(paths: eventPaths)
            }
            guard
                let created = FSEventStreamCreate(
                    kCFAllocatorDefault,
                    callback,
                    &context,
                    watchPaths as CFArray,
                    FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
                    0.5,
                    FSEventStreamCreateFlags(
                        kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagNoDefer))
            else { return }
            stream = created
            FSEventStreamSetDispatchQueue(created, queue)
            FSEventStreamStart(created)
        }
    }

    deinit {
        stop()
    }

    func stop() {
        queue.sync {
            for item in pending.values {
                item.cancel()
            }
            pending = [:]
            if let stream {
                FSEventStreamStop(stream)
                FSEventStreamInvalidate(stream)
                FSEventStreamRelease(stream)
                self.stream = nil
            }
            continuation.finish()
        }
    }

    static func watchPaths(for rootPaths: [String]) -> [String] {
        var ancestors: [String] = []
        for path in rootPaths {
            var current = path
            while !FileManager.default.fileExists(atPath: current) {
                let parent = (current as NSString).deletingLastPathComponent
                guard parent.count > 1, parent != current else { break }
                current = parent
            }
            ancestors.append(current)
        }
        let sorted = ancestors.sorted { $0.count < $1.count }
        var kept: [String] = []
        for path in sorted {
            if !kept.contains(where: { path == $0 || path.hasPrefix($0 + "/") }) {
                kept.append(path)
            }
        }
        return kept
    }

    private func handle(paths: [String]) {
        var touched: Set<SourceKind> = []
        for path in paths {
            touched.formUnion(map.kinds(forEventPath: path))
        }
        for kind in touched {
            pending[kind]?.cancel()
            let item = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.pending[kind] = nil
                self.continuation.yield([kind])
            }
            pending[kind] = item
            queue.asyncAfter(
                deadline: .now() + .milliseconds(Int(debounce / .milliseconds(1))),
                execute: item)
        }
    }
}
