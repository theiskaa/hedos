//! Small internal helpers shared across the kernel, hand-rolled to avoid pulling
//! in dependencies for trivial work (timestamps, hex encoding).

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Timestamps in the kernel's stores are
/// plain integers rather than formatted dates, so no date library is needed.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Lowercase hex encoding of a byte slice.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
