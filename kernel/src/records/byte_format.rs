//! Human-readable byte-size formatting.

const GB: i64 = 1 << 30;
const MB: i64 = 1 << 20;
const KB: i64 = 1 << 10;

/// Format a byte count as a short human string (`B`/`KB`/`MB`/`GB`). Gigabytes
/// get one decimal place, with a trailing `.0` trimmed.
pub fn format_bytes(bytes: i64) -> String {
    if bytes >= GB {
        let value = bytes as f64 / GB as f64;
        let formatted = format!("{value:.1}");
        let trimmed = formatted.strip_suffix(".0").unwrap_or(&formatted);
        format!("{trimmed} GB")
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}
