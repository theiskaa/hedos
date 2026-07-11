import Foundation

final class ChatSessionGate: @unchecked Sendable {
    private let lock = NSLock()
    private var streaming: Set<String> = []

    func begin(_ sessionID: String) throws {
        lock.lock()
        defer { lock.unlock() }
        guard !streaming.contains(sessionID) else {
            throw KernelError.sessionBusy(sessionID)
        }
        streaming.insert(sessionID)
    }

    func end(_ sessionID: String) {
        lock.lock()
        defer { lock.unlock() }
        streaming.remove(sessionID)
    }
}
