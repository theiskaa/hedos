//! Saturating byte arithmetic (Swift `Install/ByteArithmetic.swift`).

/// Sum the non-negative values, saturating at `i64::MAX` instead of overflowing.
/// Negative inputs are treated as zero.
pub(crate) fn saturating_sum(values: impl Iterator<Item = i64>) -> i64 {
    values.fold(0i64, |accumulated, value| {
        accumulated.saturating_add(value.max(0))
    })
}
