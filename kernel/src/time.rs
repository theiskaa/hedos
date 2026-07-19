//! Shared time helpers: the epoch-millisecond clock and the hand-rolled
//! Gregorian calendar conversions used for ISO 8601 wire timestamps. Kept in the
//! kernel (the dependency floor) so every crate shares one implementation
//! instead of a date-library dependency or a per-crate copy.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current wall-clock time in milliseconds since the Unix epoch, or `0` if
/// the clock is before the epoch (unreachable on a sane system).
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Format `millis` since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ` (UTC, no
/// fractional seconds) — RFC 3339 / ISO 8601.
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

/// Parse an ISO 8601 UTC timestamp back to epoch milliseconds. Accepts both the
/// fixed-width `YYYY-MM-DDTHH:MM:SSZ` form [`iso8601`] emits and a fractional
/// `…:SS.fffZ` form (only the leading three fraction digits are kept). Returns
/// `None` unless the shape is a `date T time` split with in-range fields; the
/// fraction is scanned with `chars()` so untrusted multibyte input never panics.
pub fn millis_from_iso8601(text: &str) -> Option<i64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (hms, fraction) = match time.split_once('.') {
        Some((hms, fraction)) => (hms, Some(fraction)),
        None => (time, None),
    };
    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    let sub_millis: i64 = fraction
        .map(|fraction| {
            let digits: String = fraction
                .chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect();
            format!("{digits:0<3}").parse().unwrap_or(0)
        })
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000 + sub_millis)
}

/// Convert a `(year, month, day)` civil date to days since the Unix epoch, via
/// Howard Hinnant's `days_from_civil` algorithm (the inverse of
/// [`civil_from_days`]).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146096]
    era * 146_097 + day_of_era - 719_468
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)` civil
/// date, via Howard Hinnant's `civil_from_days` algorithm.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
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
        assert_eq!(iso8601(-1_000), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn iso8601_round_trips_through_millis() {
        for millis in [0, 1_600_000_000_000, 1_582_934_400_000] {
            let text = iso8601(millis);
            assert_eq!(millis_from_iso8601(&text), Some(millis));
        }
    }

    #[test]
    fn civil_dates_anchor_at_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(2024, 1, 15), 19737);
        // Leap day and the day-after-Feb boundary.
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2024, 2, 29), 19782);
    }

    #[test]
    fn the_parser_handles_fractions_and_never_panics() {
        assert_eq!(millis_from_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00.000Z"),
            Some(1_705_314_600_000)
        );
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00Z"),
            Some(1_705_314_600_000)
        );
        // Fractions of varying length pad/truncate to milliseconds.
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00.5Z"),
            Some(1_705_314_600_500)
        );
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00.12Z"),
            Some(1_705_314_600_120)
        );
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00.123456Z"),
            Some(1_705_314_600_123)
        );
        // A multibyte char in the fraction must NOT panic (untrusted input).
        assert_eq!(
            millis_from_iso8601("2024-01-15T10:30:00.12éZ"),
            Some(1_705_314_600_120)
        );
    }

    #[test]
    fn a_malformed_timestamp_does_not_parse() {
        assert_eq!(millis_from_iso8601("not-a-time"), None);
        assert_eq!(millis_from_iso8601("2020/09/13 12:26:40"), None);
        assert_eq!(millis_from_iso8601("2024-01-15T10:30:00+02:00"), None);
        assert_eq!(millis_from_iso8601("2024-13-01T00:00:00Z"), None);
        assert_eq!(millis_from_iso8601(""), None);
    }
}
