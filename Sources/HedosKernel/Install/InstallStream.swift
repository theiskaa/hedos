import Foundation
import Synchronization

final class InstallInterruptionCleanup: Sendable {
    private let action = Mutex<(@Sendable () -> Void)?>(nil)

    func register(_ cleanup: @escaping @Sendable () -> Void) {
        action.withLock { $0 = cleanup }
    }

    func run() {
        action.withLock { $0 }?()
    }
}

enum InstallStream {
    static func make(
        mapError: @escaping @Sendable (Error) -> InstallError? = { _ in nil },
        run body: @escaping @Sendable (
            AsyncThrowingStream<InstallStreamEvent, Error>.Continuation,
            InstallInterruptionCleanup
        ) async throws -> Void
    ) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { continuation in
            let interruption = InstallInterruptionCleanup()
            let task = Task {
                do {
                    try await body(continuation, interruption)
                    continuation.finish()
                } catch is CancellationError {
                    interruption.run()
                    continuation.finish()
                } catch {
                    interruption.run()
                    continuation.finish(throwing: mapError(error) ?? error)
                }
            }
            continuation.onTermination = { termination in
                task.cancel()
                if case .cancelled = termination {
                    interruption.run()
                }
            }
        }
    }
}
