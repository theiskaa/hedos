import Foundation

public enum KernelError: Error, Sendable {
    case notImplemented(String)
    case modelNotFound(String)
    case capabilityUnsupported(model: String, capability: Capability)
    case runtimeUnavailable(hint: String)
    case runtimeFailed(String)
}

public actor Kernel {
    public static let version = "0.1.0"

    public let registry: Registry
    private let adapters: [any RuntimeAdapter]

    public init(
        directory: URL,
        adapters: [any RuntimeAdapter] = [LlamaCppAdapter(), OllamaAdapter()]
    ) {
        self.registry = Registry(directory: directory)
        self.adapters = adapters
    }

    public init() {
        self.registry = Registry(directory: Registry.defaultDirectory())
        self.adapters = [LlamaCppAdapter(), OllamaAdapter()]
    }

    public func discover() async throws -> DiscoverySummary {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let scanners: [any StoreScanner] = [
            OllamaStoreScanner(root: home.appendingPathComponent(".ollama/models")),
            HFCacheScanner(roots: HFCacheScanner.defaultRoots()),
            LMStudioScanner(roots: LMStudioScanner.defaultRoots()),
            LooseFileScanner(directories: LooseFileScanner.defaultDirectories()),
        ]
        let summary = try await DiscoveryService(scanners: scanners).discover(into: registry)
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
        return summary
    }

    public func resolve() async throws {
        try await ResolutionEngine(adapters: adapters).resolveAll(in: registry)
    }

    public func confirmRuntime(_ modelID: String) async throws {
        guard var record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        record.runtime.confirmedAt = Date()
        try await registry.register(record)
    }

    public func overrideRuntime(_ modelID: String, to runtimeID: String) async throws {
        guard var record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        let identified = Identification.identify(record)
        let bids = adapters.compactMap { adapter -> (id: String, bid: RuntimeBid)? in
            guard let bid = adapter.bid(record, identified) else { return nil }
            return (adapter.id, bid)
        }
        guard let chosen = bids.first(where: { $0.id == runtimeID }) else {
            throw KernelError.runtimeFailed("runtime \(runtimeID) cannot serve \(record.name)")
        }
        record.runtime = RuntimeRef(
            id: chosen.id,
            resolved: .user,
            tier: chosen.bid.tier,
            alternatives: bids.map(\.id).filter { $0 != runtimeID },
            confirmedAt: Date())
        try await registry.register(record)
    }

    public func shelf() async throws -> [ModelRecord] {
        try await registry.list()
    }

    public func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        guard let adapter = adapters.first(where: { $0.canServe(record, capability) }) else {
            throw KernelError.capabilityUnsupported(model: record.name, capability: capability)
        }
        return adapter.invoke(record, capability, payload: payload)
    }

    public func chat(
        _ modelID: String, messages: [ChatMessage]
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        let payload: JSONValue = .object([
            "messages": .array(
                messages.map {
                    .object([
                        "role": .string($0.role.rawValue),
                        "content": .string($0.content),
                    ])
                })
        ])
        return try await invoke(modelID, .chat, payload: payload)
    }

    public func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String {
        throw KernelError.notImplemented("submit")
    }
}
