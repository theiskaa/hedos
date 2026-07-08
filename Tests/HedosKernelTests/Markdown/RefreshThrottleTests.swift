import Foundation
import Testing

@testable import HedosKernel

@Test func refreshThrottleFiresOnceAtStartThenEverySeventhTick() {
    var throttle = RefreshThrottle(everyTicks: 7)
    let first = throttle.shouldRefresh()
    #expect(first)
    for _ in 0..<6 {
        let skipped = throttle.shouldRefresh()
        #expect(!skipped)
    }
    let seventh = throttle.shouldRefresh()
    #expect(seventh)
    for _ in 0..<6 {
        let skipped = throttle.shouldRefresh()
        #expect(!skipped)
    }
    let fourteenth = throttle.shouldRefresh()
    #expect(fourteenth)
}
