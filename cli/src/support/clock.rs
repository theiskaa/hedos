//! Short human forms for spans of time, shared by the screen and the commands.

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

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

/// A span in milliseconds as [`duration`] reads it, rounding down to the second.
pub fn millis(millis: i64) -> String {
    duration(millis.max(0) / 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_pick_the_largest_whole_unit() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(26 * 60 + 30), "26m");
        assert_eq!(duration(3 * 3600), "3h");
        assert_eq!(duration(2 * 86_400 + 5), "2d");
        assert_eq!(duration(-5), "0s");
    }

    #[test]
    fn millisecond_spans_round_down_to_the_second() {
        assert_eq!(millis(45_900), "45s");
        assert_eq!(millis(900), "0s");
        assert_eq!(millis(-900), "0s");
    }
}
