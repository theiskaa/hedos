//! Human-readable byte-size formatting.

const GB: i64 = 1 << 30;
const MB: i64 = 1 << 20;
const KB: i64 = 1 << 10;

/// Bytes in one mebibyte, for footprints recorded in MiB.
pub const BYTES_PER_MIB: i64 = MB;
/// Bytes in one gibibyte, for memory figures.
pub const BYTES_PER_GIB: i64 = GB;

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
