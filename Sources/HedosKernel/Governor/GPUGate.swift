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
        let id: UUID
        let producer: GPUProducer
        let continuation: CheckedContinuation<Bool, Never>
    }

    private var holder: GPUProducer?
    private var holders = 0
    private var waiters: [Waiter] = []

    public init() {}

    public func acquire(_ producer: GPUProducer) async throws {
        try Task.checkCancellation()
        if waiters.isEmpty, admits(producer) {
            holder = producer
            holders += 1
            return
        }
        let id = UUID()
        let granted = await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                waiters.append(Waiter(id: id, producer: producer, continuation: continuation))
            }
        } onCancel: {
            Task { await self.cancelWaiter(id) }
        }
        if !granted {
            throw CancellationError()
        }
    }

    private func acquireUncancellable(_ producer: GPUProducer) async {
        if waiters.isEmpty, admits(producer) {
            holder = producer
            holders += 1
            return
        }
        let id = UUID()
        _ = await withCheckedContinuation { continuation in
            waiters.append(Waiter(id: id, producer: producer, continuation: continuation))
        }
    }

    private func cancelWaiter(_ id: UUID) {
        guard let index = waiters.firstIndex(where: { $0.id == id }) else { return }
        let waiter = waiters.remove(at: index)
        waiter.continuation.resume(returning: false)
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
        await acquireUncancellable(producer)
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
            next.continuation.resume(returning: true)
        }
    }
}
