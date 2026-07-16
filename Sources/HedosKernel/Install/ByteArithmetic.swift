extension Int64 {
    func addingClamped(_ other: Int64) -> Int64 {
        let (sum, overflow) = addingReportingOverflow(other)
        return overflow ? (other > 0 ? .max : .min) : sum
    }
}

extension Sequence<Int64> {
    func saturatingSum() -> Int64 {
        reduce(0) { $0.addingClamped(Swift.max(0, $1)) }
    }
}
