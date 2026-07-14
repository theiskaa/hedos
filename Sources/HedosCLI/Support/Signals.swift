import Dispatch
import Foundation

enum Signals {
    nonisolated(unsafe) private static var source: DispatchSourceSignal?

    static func waitForInterrupt() async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            signal(SIGINT, SIG_IGN)
            let interrupt = DispatchSource.makeSignalSource(
                signal: SIGINT, queue: DispatchQueue.global())
            interrupt.setEventHandler {
                signal(SIGINT, SIG_DFL)
                interrupt.cancel()
                continuation.resume()
            }
            interrupt.resume()
            source = interrupt
        }
    }
}
