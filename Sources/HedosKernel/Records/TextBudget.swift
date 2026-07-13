import Foundation

enum TextBudget {
    static func clip(_ text: String, to cap: Int) -> (kept: Substring, overflowed: Bool, total: Int) {
        let total = text.utf8.count
        guard total > cap else { return (Substring(text), false, total) }
        var kept = text.prefix(cap)
        while kept.utf8.count > cap { kept = kept.dropLast() }
        return (kept, true, total)
    }
}
