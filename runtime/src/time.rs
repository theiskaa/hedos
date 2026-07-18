//! Small time helpers shared across the runtime.

/// The current wall-clock time in epoch milliseconds, or `0` if the clock is
/// before the Unix epoch (unreachable on a sane system).
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
