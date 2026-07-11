import Foundation

public enum GPUProducer: Hashable, Sendable {
    case generation(modelID: String)
    case load(modelID: String)
    case unload(modelID: String)
    case job(modelID: String)

    var shares: Bool {
        if case .generation = self { return true }
        return false
    }
}

public actor GPUGate {
    private struct Waiter {
        let producer: GPUProducer
        let continuation: CheckedContinuation<Void, Never>
    }

    private var holder: GPUProducer?
    private var holders = 0
    private var waiters: [Waiter] = []

    public init() {}

    public func acquire(_ producer: GPUProducer) async {
        if waiters.isEmpty, admits(producer) {
            holder = producer
            holders += 1
            return
        }
        await withCheckedContinuation { continuation in
            waiters.append(Waiter(producer: producer, continuation: continuation))
        }
    }

    public func release(_ producer: GPUProducer) {
        guard holder == producer, holders > 0 else {
            assertionFailure("unbalanced GPUGate release by \(producer)")
            return
        }
        holders -= 1
        if holders == 0 {
            holder = nil
            admitWaiters()
        }
    }

    public func withAccess<T: Sendable>(
        _ producer: GPUProducer, _ body: @Sendable () async throws -> T
    ) async rethrows -> T {
        await acquire(producer)
        defer { release(producer) }
        return try await body()
    }

    private func admits(_ producer: GPUProducer) -> Bool {
        guard let holder else { return true }
        return holder == producer && producer.shares
    }

    private func admitWaiters() {
        while let next = waiters.first, admits(next.producer) {
            waiters.removeFirst()
            holder = next.producer
            holders += 1
            next.continuation.resume()
        }
    }
}
