import Foundation

public struct RefreshThrottle: Sendable {
    private let everyTicks: Int
    private var counter: Int

    public init(everyTicks: Int) {
        self.everyTicks = everyTicks
        self.counter = everyTicks
    }

    public mutating func shouldRefresh() -> Bool {
        counter += 1
        if counter >= everyTicks {
            counter = 0
            return true
        }
        return false
    }
}
