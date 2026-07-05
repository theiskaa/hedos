import Foundation

public enum KernelError: Error, Sendable {
    /// The signature is pinned but the path lands in a later milestone.
    case notImplemented(String)
    case modelNotFound(String)
    case capabilityUnsupported(model: String, capability: Capability)
}

/// The Hedos kernel — the stateful brain between the models on this machine
/// and everything that wants to use them. Surfaces (the app UI, later the
/// local gateway) talk only to this API; no surface has a privileged path.
public actor Kernel {
    public static let version = "0.1.0"

    public let registry: Registry

    public init(directory: URL) {
        self.registry = Registry(directory: directory)
    }

    public init() {
        self.registry = Registry(directory: Registry.defaultDirectory())
    }

    /// Interactive capabilities (`chat`, `speak`, …): one call, a typed
    /// stream of chunks back. Real path lands in M3 (Ollama chat).
    public func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<JSONValue, Error> {
        throw KernelError.notImplemented("invoke")
    }

    /// Generative capabilities (`image`, `video`, …): submit returns a job
    /// ID; progress, previews, and artifacts follow. Lands in v0.2.
    public func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String {
        throw KernelError.notImplemented("submit")
    }
}
