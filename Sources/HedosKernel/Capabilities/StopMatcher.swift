import Foundation

struct StopMatcher: Sendable {
    private let stops: [String]
    private var buffer: String = ""
    private(set) var stopped = false

    init(_ stops: [String]) {
        self.stops = stops.filter { !$0.isEmpty }
    }

    var isActive: Bool { !stops.isEmpty }

    mutating func feed(_ chunk: String) -> String {
        guard isActive, !stopped else { return stopped ? "" : chunk }
        buffer += chunk
        if let match = earliestStop() {
            let emit = String(buffer[buffer.startIndex..<match.lowerBound])
            buffer = ""
            stopped = true
            return emit
        }
        let held = heldSuffixLength()
        let emit = String(buffer.dropLast(held))
        buffer = String(buffer.suffix(held))
        return emit
    }

    mutating func flush() -> String {
        guard !stopped else { return "" }
        let emit = buffer
        buffer = ""
        return emit
    }

    private func earliestStop() -> Range<String.Index>? {
        var earliest: Range<String.Index>?
        for stop in stops {
            guard let range = buffer.range(of: stop) else { continue }
            if earliest == nil || range.lowerBound < earliest!.lowerBound {
                earliest = range
            }
        }
        return earliest
    }

    private func heldSuffixLength() -> Int {
        let longest = (stops.map(\.count).max() ?? 1) - 1
        let cap = Swift.min(longest, buffer.count)
        guard cap > 0 else { return 0 }
        for length in stride(from: cap, through: 1, by: -1) {
            let suffix = String(buffer.suffix(length))
            if stops.contains(where: { $0.hasPrefix(suffix) }) { return length }
        }
        return 0
    }

    static func strings(from value: JSONValue?) -> [String] {
        switch value {
        case .string(let single):
            return [single]
        case .array(let items):
            return items.compactMap {
                if case .string(let text) = $0 { return text }
                return nil
            }
        default:
            return []
        }
    }
}
