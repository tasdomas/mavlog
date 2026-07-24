//! Time formatting and the jump-to-time parser.

use chrono::{DateTime, NaiveDateTime, NaiveTime};

/// How timestamps are shown in the message list and jump prompt.
#[derive(Debug, Default, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum TimeFormat {
    #[default]
    DateTime,
    OffsetSecs,
}

/// Absolute wall-clock time, e.g. "2024-07-17 07:06:40.000" (UTC).
pub fn format_datetime(timestamp_us: u64) -> String {
    match DateTime::from_timestamp_micros(timestamp_us as i64) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("({timestamp_us} us)"),
    }
}

/// Seconds relative to the start of the log, e.g. "T+4.600s".
pub fn format_offset(timestamp_us: u64, start_us: u64) -> String {
    let secs = (timestamp_us as i64 - start_us as i64) as f64 / 1e6;
    format!("T{secs:+.3}s")
}

/// Parse jump input into an absolute timestamp in microseconds. The accepted
/// syntax follows the current time column format: seconds relative to the log
/// start, or a wall-clock time (date defaults to the log start's date, UTC).
pub fn parse_jump(input: &str, format: TimeFormat, start_us: u64) -> Result<u64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty input".to_string());
    }
    match format {
        TimeFormat::OffsetSecs => {
            let s = s.strip_prefix('T').unwrap_or(s);
            let s = s.strip_suffix('s').unwrap_or(s);
            let secs: f64 = s
                .parse()
                .map_err(|_| "expected seconds, e.g. 42.5".to_string())?;
            let target = start_us as i64 + (secs * 1e6).round() as i64;
            Ok(target.max(0) as u64)
        }
        TimeFormat::DateTime => {
            if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
                return Ok(dt.and_utc().timestamp_micros().max(0) as u64);
            }
            let start = DateTime::from_timestamp_micros(start_us as i64)
                .ok_or_else(|| "log start time invalid".to_string())?;
            for fmt in ["%H:%M:%S%.f", "%H:%M"] {
                if let Ok(t) = NaiveTime::parse_from_str(s, fmt) {
                    let dt = start.date_naive().and_time(t);
                    return Ok(dt.and_utc().timestamp_micros().max(0) as u64);
                }
            }
            Err("expected HH:MM:SS[.mmm] or YYYY-MM-DD HH:MM:SS".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2024-07-17 07:06:40 UTC in microseconds.
    const START_US: u64 = 1_721_200_000_000_000;

    #[test]
    fn parses_offset_input() {
        assert_eq!(
            parse_jump("42.5", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US + 42_500_000
        );
        assert_eq!(
            parse_jump("T+1s", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert_eq!(
            parse_jump("-5", TimeFormat::OffsetSecs, START_US).unwrap(),
            START_US - 5_000_000
        );
        assert!(parse_jump("abc", TimeFormat::OffsetSecs, START_US).is_err());
    }

    #[test]
    fn parses_datetime_input() {
        // Time-only inherits the log start's date.
        assert_eq!(
            parse_jump("07:06:41", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert_eq!(
            parse_jump("07:06:41.250", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_250_000
        );
        assert_eq!(
            parse_jump("2024-07-17 07:06:41", TimeFormat::DateTime, START_US).unwrap(),
            START_US + 1_000_000
        );
        assert!(parse_jump("07:06:41", TimeFormat::OffsetSecs, START_US).is_err());
        assert!(parse_jump("nonsense", TimeFormat::DateTime, START_US).is_err());
    }
}
