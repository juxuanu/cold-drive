//! Sizes and dates as GNOME shows them.

use chrono::{DateTime, Datelike, Local};

/// A size in SI units, as `g_format_size` gives it: "42 bytes", "1.2 MB".
pub fn size(bytes: u64) -> String {
    const UNITS: &[&str] = &["kB", "MB", "GB", "TB", "PB"];

    if bytes < 1000 {
        return if bytes == 1 {
            "1 byte".into()
        } else {
            format!("{bytes} bytes")
        };
    }

    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }

    format!("{value:.1} {}", UNITS[unit])
}

/// An RFC 3339 time as Files shows a modification date: the time for today,
/// day and month this year, the full date before.
pub fn date(rfc3339: &str) -> Option<String> {
    let when = DateTime::parse_from_rfc3339(rfc3339)
        .ok()?
        .with_timezone(&Local);

    Some(relative(when, Local::now()))
}

fn relative(when: DateTime<Local>, now: DateTime<Local>) -> String {
    if when.date_naive() == now.date_naive() {
        when.format("%H:%M").to_string()
    } else if when.year() == now.year() {
        when.format("%-d %b").to_string()
    } else {
        when.format("%-d %b %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn sizes_are_si() {
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1000), "1.0 kB");
        assert_eq!(size(1_234_567), "1.2 MB");
        assert_eq!(size(999_999), "1.0 MB");
    }

    #[test]
    fn dates_are_relative_to_now() {
        let now = Local.with_ymd_and_hms(2026, 10, 1, 15, 0, 0).unwrap();

        assert_eq!(
            relative(Local.with_ymd_and_hms(2026, 10, 1, 9, 5, 0).unwrap(), now),
            "09:05"
        );
        assert_eq!(
            relative(Local.with_ymd_and_hms(2026, 3, 7, 9, 5, 0).unwrap(), now),
            "7 Mar"
        );
        assert_eq!(
            relative(Local.with_ymd_and_hms(2024, 3, 7, 9, 5, 0).unwrap(), now),
            "7 Mar 2024"
        );
        assert!(date("not a date").is_none());
    }
}
