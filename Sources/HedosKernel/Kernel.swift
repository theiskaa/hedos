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

    public init(directory: URL, adapters: [any RuntimeAdapter] = [OllamaAdapter()]) {
        self.registry = Registry(directory: directory)
        self.adapters = adapters
    }

    public init() {
        self.registry = Registry(directory: Registry.defaultDirectory())
        self.adapters = [OllamaAdapter()]
    }

    public func discover() async throws -> DiscoverySummary {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let scanners: [any StoreScanner] = [
            OllamaStoreScanner(root: home.appendingPathComponent(".ollama/models")),
            HFCacheScanner(roots: HFCacheScanner.defaultRoots()),
            LMStudioScanner(roots: LMStudioScanner.defaultRoots()),
            LooseFileScanner(directories: LooseFileScanner.defaultDirectories()),
        ]
        return try await DiscoveryService(scanners: scanners).discover(into: registry)
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
