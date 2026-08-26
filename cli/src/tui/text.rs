//! Short human forms for the numbers the screen shows.

use kernel::records::byte_format::{format_bytes, one_decimal};

const GIB: f64 = (1u64 << 30) as f64;
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Bytes as `4.7 GB` / `512 MB`, the same form `hedos ls --json` readers see.
pub fn bytes(bytes: i64) -> String {
    format_bytes(bytes)
}

/// Bytes in gibibytes with one decimal, for memory figures set against a
/// machine total: `14.2`. Negative counts read as zero.
pub fn gib(bytes: i64) -> String {
    one_decimal(bytes.max(0) as f64 / GIB)
}

/// A duration in seconds as its largest whole unit: `45s`, `26m`, `3h`, `2d`.
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= DAY {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR {
        format!("{}h", seconds / HOUR)
    } else if seconds >= MINUTE {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{seconds}s")
    }
}

/// A count with a noun that takes a plain `s` plural: `1 model`, `12 models`.
pub fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_keeps_one_decimal_and_trims_zero() {
        assert_eq!(gib(64 * (1 << 30)), "64");
        assert_eq!(gib(14_200_000_000), "13.2");
        assert_eq!(gib(0), "0");
        assert_eq!(gib(-1), "0");
    }

    #[test]
    fn durations_pick_the_largest_whole_unit() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(26 * 60 + 30), "26m");
        assert_eq!(duration(3 * 3600), "3h");
        assert_eq!(duration(2 * 86_400 + 5), "2d");
        assert_eq!(duration(-5), "0s");
    }

    #[test]
    fn counts_pluralize() {
        assert_eq!(count(1, "model"), "1 model");
        assert_eq!(count(0, "model"), "0 models");
    }
}
