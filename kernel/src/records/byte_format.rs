//! Human-readable byte-size formatting.
//!
//! Sizes are decimal (`4.9 GB` is 4.9e9 bytes, not 4.9 GiB), which is how
//! the hubs and a model's own page state them; memory is binary and keeps
//! its own `GiB` figures through the constants below.

const GB: i64 = 1_000_000_000;
const MB: i64 = 1_000_000;
const KB: i64 = 1_000;

/// Bytes in one mebibyte, for footprints recorded in MiB.
pub const BYTES_PER_MIB: i64 = 1 << 20;
/// Bytes in one gibibyte, for memory figures.
pub const BYTES_PER_GIB: i64 = 1 << 30;

/// `value` with one decimal place, a trailing `.0` trimmed: `4.7`, `64`.
pub fn one_decimal(value: f64) -> String {
    let formatted = format!("{value:.1}");
    formatted
        .strip_suffix(".0")
        .unwrap_or(&formatted)
        .to_owned()
}

/// Format a byte count as a short human string (`B`/`KB`/`MB`/`GB`). Gigabytes
/// get one decimal place, with a trailing `.0` trimmed.
pub fn format_bytes(bytes: i64) -> String {
    if bytes >= GB {
        format!("{} GB", one_decimal(bytes as f64 / GB as f64))
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}
