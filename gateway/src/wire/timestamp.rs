//! Formatting a Unix-millisecond instant as an ISO 8601 UTC string, hand-rolled
//! to avoid a date-library dependency for the one wire field that needs it (the
//! Ollama `created_at`/`modified_at` timestamps).

use std::time::{SystemTime, UNIX_EPOCH};

/// The current instant as an ISO 8601 UTC string.
pub fn now_iso8601() -> String {
    iso8601(now_millis())
}

/// The current time in whole seconds since the Unix epoch (the OpenAI `created`
/// field).
pub fn now_unix_seconds() -> i64 {
    now_millis() / 1000
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Format `millis` since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ` (UTC, no
/// fractional seconds), matching Swift's default `ISO8601DateFormatter`.
pub fn iso8601(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)` civil
/// date, via Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days fall at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_position = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_formatted() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_known_instant_is_formatted() {
        // 1_600_000_000 seconds since the epoch.
        assert_eq!(iso8601(1_600_000_000_000), "2020-09-13T12:26:40Z");
    }

    #[test]
    fn sub_second_millis_truncate_to_the_second() {
        assert_eq!(iso8601(1_600_000_000_999), "2020-09-13T12:26:40Z");
    }

    #[test]
    fn a_leap_day_is_handled() {
        // 2020-02-29T00:00:00Z = 1_582_934_400 seconds.
        assert_eq!(iso8601(1_582_934_400_000), "2020-02-29T00:00:00Z");
    }

    #[test]
    fn a_pre_epoch_instant_formats_in_the_past() {
        // -1 second before the epoch.
        assert_eq!(iso8601(-1_000), "1969-12-31T23:59:59Z");
    }
}
