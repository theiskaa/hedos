import Foundation

public enum GatewayWorkKind: Sendable, Hashable {
    case stream
    case job
}

public enum GatewayAdmissionState: Sendable, Hashable {
    case ready
    case saturated(retryAfterSeconds: Int)
}

final class GatewayInflight: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func enter(limit: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard count < limit else { return false }
        count += 1
        return true
    }

    func exit() {
        lock.lock()
        count -= 1
        lock.unlock()
    }
}

enum GatewayBackpressure {
    static func require(
        _ port: any GatewayPort, record: ModelRecord, kind: GatewayWorkKind
    ) async throws {
        let state = await port.admissionState(
            modelID: record.id, footprintMB: record.footprintMB, kind: kind)
        if case .saturated(let retryAfter) = state {
            throw GatewayError(
                .overloaded, "the machine is busy with another model — retry shortly",
                retryAfterSeconds: retryAfter)
        }
    }
}
