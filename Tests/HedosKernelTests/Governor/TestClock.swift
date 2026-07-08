import Foundation
import Testing

@testable import HedosKernel

final class TestClock: Clock, @unchecked Sendable {
    struct Instant: InstantProtocol {
        fileprivate var offset: Duration

        static func < (lhs: Instant, rhs: Instant) -> Bool { lhs.offset < rhs.offset }

        func advanced(by duration: Duration) -> Instant {
            Instant(offset: offset + duration)
        }

        func duration(to other: Instant) -> Duration {
            other.offset - offset
        }
    }

    private struct Sleeper {
        let deadline: Instant
        let continuation: CheckedContinuation<Void, Never>
    }

    private let lock = NSLock()
    private var offset: Duration = .zero
    private var sleepers: [Sleeper] = []

    var now: Instant { lock.withLocked { Instant(offset: offset) } }
    var minimumResolution: Duration { .zero }

    func sleep(until deadline: Instant, tolerance: Duration? = nil) async throws {
        try Task.checkCancellation()
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let dueNow = lock.withLocked { () -> Bool in
                if deadline.offset <= offset { return true }
                sleepers.append(Sleeper(deadline: deadline, continuation: continuation))
                return false
            }
            if dueNow { continuation.resume() }
        }
    }

    func advance(by duration: Duration) {
        let due = lock.withLocked { () -> [Sleeper] in
            offset += duration
            let ready = sleepers.filter { $0.deadline.offset <= offset }
            sleepers.removeAll { $0.deadline.offset <= offset }
            return ready
        }
        for sleeper in due {
            sleeper.continuation.resume()
        }
    }
}

extension NSLock {
    fileprivate func withLocked<T>(_ body: () -> T) -> T {
        lock()
        defer { unlock() }
        return body()
    }
}

@Test func testClockSleeperResumesOnlyAfterAdvance() async throws {
    let clock = TestClock()
    let resumed = ResumeFlag()

    let sleeper = Task {
        try? await clock.sleep(for: .milliseconds(100))
        await resumed.mark()
    }

    try await Task.sleep(for: .milliseconds(50))
    #expect(await resumed.wasInvoked == false)

    clock.advance(by: .milliseconds(50))
    try await Task.sleep(for: .milliseconds(50))
    #expect(await resumed.wasInvoked == false)

    clock.advance(by: .milliseconds(60))
    _ = await sleeper.value
    #expect(await resumed.wasInvoked)
}

private actor ResumeFlag {
    private(set) var wasInvoked = false

    func mark() {
        wasInvoked = true
    }
}
