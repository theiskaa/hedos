import Foundation

public actor ModelLease {
    private var counts: [String: Int] = [:]
    private var drains: [String: [UUID: CheckedContinuation<Void, any Error>]] = [:]

    public init() {}

    public func acquire(_ modelID: String) {
        counts[modelID, default: 0] += 1
    }

    public func release(_ modelID: String) {
        let remaining = max((counts[modelID] ?? 0) - 1, 0)
        counts[modelID] = remaining == 0 ? nil : remaining
        if remaining == 0 {
            resumeDrains(modelID)
        }
    }

    public func count(_ modelID: String) -> Int {
        counts[modelID] ?? 0
    }

    public func drain(_ modelID: String) async throws {
        while counts[modelID, default: 0] > 0 {
            let token = UUID()
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    if counts[modelID, default: 0] == 0 {
                        continuation.resume()
                    } else if Task.isCancelled {
                        continuation.resume(throwing: CancellationError())
                    } else {
                        drains[modelID, default: [:]][token] = continuation
                    }
                }
            } onCancel: {
                Task { await self.abandonDrain(modelID, token: token) }
            }
        }
    }

    private func resumeDrains(_ modelID: String) {
        guard let waiting = drains.removeValue(forKey: modelID) else { return }
        for continuation in waiting.values {
            continuation.resume()
        }
    }

    private func abandonDrain(_ modelID: String, token: UUID) {
        guard let continuation = drains[modelID]?.removeValue(forKey: token) else { return }
        continuation.resume(throwing: CancellationError())
    }
}
