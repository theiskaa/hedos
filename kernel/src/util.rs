//! Small internal helpers shared across the kernel.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Timestamps in the kernel's stores are
/// plain integers rather than formatted dates, so no date library is needed.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
