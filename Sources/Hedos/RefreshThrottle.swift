import Foundation

struct RefreshThrottle: Sendable {
    private let everyTicks: Int
    private var counter: Int

    init(everyTicks: Int) {
        self.everyTicks = everyTicks
        self.counter = everyTicks
    }

    mutating func shouldRefresh() -> Bool {
        counter += 1
        if counter >= everyTicks {
            counter = 0
            return true
        }
        return false
    }
}
