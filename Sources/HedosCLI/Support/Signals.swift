import Dispatch
import Foundation
import Synchronization

enum Signals {
    private enum Wait {
        case pending
        case waiting(CheckedContinuation<Void, Never>)
        case finished
    }

    nonisolated(unsafe) private static var source: DispatchSourceSignal?

    static func waitForInterrupt() async {
        let gate = Mutex<Wait>(.pending)
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                let alreadyFinished = gate.withLock { state in
                    if case .finished = state { return true }
                    state = .waiting(continuation)
                    return false
                }
                if alreadyFinished {
                    continuation.resume()
                    return
                }
                signal(SIGINT, SIG_IGN)
                let interrupt = DispatchSource.makeSignalSource(
                    signal: SIGINT, queue: DispatchQueue.global())
                interrupt.setEventHandler {
                    signal(SIGINT, SIG_DFL)
                    interrupt.cancel()
                    fire(gate)
                }
                interrupt.resume()
                source = interrupt
            }
        } onCancel: {
            fire(gate)
        }
    }

    static func restoreDefault() {
        source?.cancel()
        source = nil
        signal(SIGINT, SIG_DFL)
    }

    private static func fire(_ gate: borrowing Mutex<Wait>) {
        let continuation: CheckedContinuation<Void, Never>? = gate.withLock { state in
            if case .waiting(let waiting) = state {
                state = .finished
                return waiting
            }
            state = .finished
            return nil
        }
        continuation?.resume()
    }
}
