import Foundation

public final class DaemonLiveness: @unchecked Sendable {
    public static let shared = DaemonLiveness()

    public enum Daemon: Sendable, Hashable {
        case comfyUI
        case a1111
    }

    public struct State: Sendable, Hashable {
        public var alive: Bool = false
        public var models: [String] = []
    }

    public struct Snapshot: Sendable, Hashable {
        public var comfyUI = State()
        public var a1111 = State()
    }

    private let lock = NSLock()
    private var snapshot = Snapshot()
    private var epoch = 0
    private let session: URLSession
    let comfyURL: URL
    let a1111URL: URL

    public init(
        comfyURL: URL = URL(string: "http://127.0.0.1:8188")!,
        a1111URL: URL = URL(string: "http://127.0.0.1:7860")!,
        session: URLSession = .shared
    ) {
        self.comfyURL = comfyURL
        self.a1111URL = a1111URL
        self.session = session
    }

    public func current() -> Snapshot {
        lock.lock()
        defer { lock.unlock() }
        return snapshot
    }

    func store(_ updated: Snapshot) {
        lock.lock()
        defer { lock.unlock() }
        snapshot = updated
    }

    public func markDead(_ daemon: Daemon) {
        lock.lock()
        defer { lock.unlock() }
        epoch += 1
        switch daemon {
        case .comfyUI: snapshot.comfyUI = State()
        case .a1111: snapshot.a1111 = State()
        }
    }

    public func probe() async {
        let startEpoch = lock.withLock { epoch }
        async let comfy = probeComfyUI()
        async let auto = probeA1111()
        let fresh = Snapshot(comfyUI: await comfy, a1111: await auto)
        lock.withLock {
            if epoch == startEpoch { snapshot = fresh }
        }
    }

    func probeComfyUI() async -> State {
        guard await reachable(comfyURL.appendingPathComponent("system_stats")) else {
            return State()
        }
        let names = await fetchJSON(comfyURL.appendingPathComponent("object_info")).map(
            Self.comfyCheckpoints) ?? []
        return State(alive: true, models: names)
    }

    func probeA1111() async -> State {
        guard
            let json = await fetchJSON(a1111URL.appendingPathComponent("sdapi/v1/sd-models"))
        else { return State() }
        return State(alive: true, models: Self.a1111Checkpoints(json))
    }

    private func reachable(_ url: URL) async -> Bool {
        var request = URLRequest(url: url)
        request.timeoutInterval = 2
        guard let (_, response) = try? await session.data(for: request),
            let http = response as? HTTPURLResponse
        else { return false }
        return http.statusCode == 200
    }

    private func fetchJSON(_ url: URL) async -> Any? {
        var request = URLRequest(url: url)
        request.timeoutInterval = 2
        guard let (data, response) = try? await session.data(for: request),
            let http = response as? HTTPURLResponse, http.statusCode == 200
        else { return nil }
        return try? JSONSerialization.jsonObject(with: data)
    }

    static func comfyCheckpoints(_ objectInfo: Any) -> [String] {
        guard let object = objectInfo as? [String: Any],
            let loader = object["CheckpointLoaderSimple"] as? [String: Any],
            let input = loader["input"] as? [String: Any],
            let required = input["required"] as? [String: Any],
            let ckpt = required["ckpt_name"] as? [Any],
            let names = ckpt.first as? [Any]
        else { return [] }
        return names.compactMap { $0 as? String }
    }

    static func a1111Checkpoints(_ models: Any) -> [String] {
        guard let entries = models as? [[String: Any]] else { return [] }
        return entries.compactMap { ($0["model_name"] ?? $0["title"]) as? String }
    }

    static func matches(record: ModelRecord, servedModels: [String]) -> Bool {
        !matchingModels(record: record, servedModels: servedModels).isEmpty
    }

    static func matchingModels(record: ModelRecord, servedModels: [String]) -> [String] {
        let candidates = Set(
            [record.name, record.source.repo, lastComponent(record.source.path)]
                .compactMap { $0 }
                .map(normalized))
        return servedModels.filter { candidates.contains(normalized($0)) }
    }

    static func normalized(_ name: String) -> String {
        lastComponent(name).lowercased()
            .replacingOccurrences(of: ".safetensors", with: "")
            .replacingOccurrences(of: ".ckpt", with: "")
    }

    static func lastComponent(_ path: String) -> String {
        (path as NSString).lastPathComponent
    }

    static func dimensions(_ object: [String: JSONValue], fallback: Int = 512) -> (Int, Int) {
        if let width = object["width"]?.intValue, let height = object["height"]?.intValue {
            return (width, height)
        }
        if let size = object["size"]?.stringValue {
            let parts = size.lowercased().split(separator: "x")
            if parts.count == 2, let width = Int(parts[0]), let height = Int(parts[1]) {
                return (width, height)
            }
        }
        return (fallback, fallback)
    }
}
