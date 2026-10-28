//! Relative display time with no I/O or formatter dependency.
//!
//! Parses the RFC3339 calendar/clock/offset representation (including fractional
//! seconds), rather than comparing timestamp strings. Invalid/missing values do
//! not become successful times. Leap-second spellings are left unavailable.
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn relative(value: Option<&str>, now: SystemTime) -> String {
    let Some(timestamp) = value.and_then(parse) else {
        return "Time unavailable".to_string();
    };
    let now = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    if timestamp > now {
        return "In the future".to_string();
    }
    let seconds = ((now - timestamp) / 1_000_000_000) as u128;
    match seconds {
        0..60 => "Just now".to_string(),
        60..3600 => quantity(seconds / 60, "minute"),
        3600..86400 => quantity(seconds / 3600, "hour"),
        86400..172800 => "Yesterday".to_string(),
        _ => quantity(seconds / 86400, "day"),
    }
}

fn quantity(value: u128, unit: &str) -> String {
    format!("{value} {unit}{} ago", if value == 1 { "" } else { "s" })
}

/// UTC nanoseconds since Unix epoch. Strict fixed-width fields, real calendar
/// validation and explicit timezone handling also work for pre-epoch times.
fn parse(value: &str) -> Option<i128> {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let digits = |start: usize, end: usize| -> Option<i64> {
        let field = bytes.get(start..end)?;
        field.iter().try_fold(0_i64, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value * 10 + i64::from(*digit - b'0'))
        })
    };
    let year = digits(0, 4)?;
    let month = digits(5, 7)?;
    let day = digits(8, 10)?;
    let hour = digits(11, 13)?;
    let minute = digits(14, 16)?;
    let second = digits(17, 19)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if year < 1 || !(1..=days).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut at = 19;
    let mut nanos = 0_i128;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let from = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            if at - from < 9 {
                nanos = nanos * 10 + i128::from(bytes[at] - b'0');
            }
            at += 1;
        }
        if at == from {
            return None;
        }
        for _ in (at - from)..9 {
            nanos *= 10;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' if at + 1 == bytes.len() => 0,
        b'+' | b'-' if at + 6 == bytes.len() && bytes[at + 3] == b':' => {
            let hours = digits(at + 1, at + 3)?;
            let minutes = digits(at + 4, at + 6)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            // RFC3339 -00:00 means unknown local offset, not verified UTC.
            if bytes[at] == b'-' && hours == 0 && minutes == 0 {
                return None;
            }
            (hours * 3600 + minutes * 60) * if bytes[at] == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    // Gregorian civil date -> days since 1970-01-01; 400-year eras account for
    // leap years without depending on the machine's local timezone.
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let day_count = era * 146097 + day_of_era - 719468;
    Some(
        i128::from(day_count * 86400 + hour * 3600 + minute * 60 + second - offset) * 1_000_000_000
            + nanos,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn instant(value: &str) -> SystemTime {
        let nanos = parse(value).unwrap();
        UNIX_EPOCH + Duration::from_nanos(nanos as u64)
    }

    #[test]
    fn boundaries_and_plural_forms_are_deterministic() {
        let at = instant("2026-10-28T12:00:00Z");
        let date = Some("2026-10-28T12:00:00Z");
        for (seconds, expected) in [
            (0, "Just now"),
            (59, "Just now"),
            (60, "1 minute ago"),
            (600, "10 minutes ago"),
            (3599, "59 minutes ago"),
            (3600, "1 hour ago"),
            (7200, "2 hours ago"),
            (86399, "23 hours ago"),
            (86400, "Yesterday"),
            (172799, "Yesterday"),
            (172800, "2 days ago"),
        ] {
            assert_eq!(relative(date, at + Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn offsets_fractions_leap_days_and_epoch_are_parsed() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse("1969-12-31T23:59:59Z"), Some(-1_000_000_000));
        assert_eq!(
            parse("2026-10-28T13:00:00+01:00"),
            parse("2026-10-28T12:00:00Z")
        );
        assert_eq!(
            parse("2026-10-28T11:00:00-01:00"),
            parse("2026-10-28t12:00:00z")
        );
        assert!(parse("2024-02-29T23:59:59.123456789123Z").is_some());
        let at = instant("2026-10-28T12:00:00.500Z");
        assert_eq!(relative(Some("2026-10-28T11:59:00.501Z"), at), "Just now");
        assert_eq!(
            relative(Some("2026-10-28T11:59:00.500Z"), at),
            "1 minute ago"
        );
    }

    #[test]
    fn missing_invalid_and_future_remain_honest() {
        let now = instant("2026-10-28T12:00:00Z");
        for date in [
            None,
            Some(""),
            Some("2026-02-29T12:00:00Z"),
            Some("2026-10-28"),
            Some("2026-10-28T24:00:00Z"),
            Some("2026-10-28T12:60:00Z"),
            Some("2026-10-28T12:00:00+24:00"),
            Some("2026-10-28T12:00:00.Z"),
            Some("2026-10-28T12:00:00Zgarbage"),
            Some("2026-10-28T12:00:00-00:00"),
            Some("2026-10-28T12:00:60Z"),
            Some("世界2026-10-28T12:00:00Z"),
        ] {
            assert_eq!(relative(date, now), "Time unavailable", "{date:?}");
        }
        assert_eq!(
            relative(Some("2026-10-28T12:00:00.001Z"), now),
            "In the future"
        );
    }
}
