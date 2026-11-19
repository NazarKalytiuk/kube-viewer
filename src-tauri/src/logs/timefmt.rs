//! Log timestamps: parsing the kubelet prefix and formatting for display/export.
//!
//! Logs are always requested with `timestamps=true`; the RFC 3339 prefix is
//! parsed once and stored as nanoseconds, so switching between "no time",
//! local time and UTC is instant and never re-fetches anything.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Deserialize;

/// No timestamp available for the line.
pub const NO_TS: i64 = i64::MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TimeMode {
    #[default]
    None,
    Local,
    Utc,
}

/// Splits `2026-11-19T15:20:25.865123456Z message` into (nanos, message).
/// Lines without a parsable prefix keep their full text.
pub fn split_timestamp(line: &str) -> (i64, &str) {
    let Some((prefix, rest)) = line.split_once(' ') else {
        // A line consisting only of a timestamp is an empty log line.
        return match line.parse::<Timestamp>() {
            Ok(ts) => (to_nanos(ts), ""),
            Err(_) => (NO_TS, line),
        };
    };
    match prefix.parse::<Timestamp>() {
        Ok(ts) => (to_nanos(ts), rest),
        Err(_) => (NO_TS, line),
    }
}

fn to_nanos(ts: Timestamp) -> i64 {
    i64::try_from(ts.as_nanosecond()).unwrap_or(NO_TS)
}

pub fn from_nanos(nanos: i64) -> Option<Timestamp> {
    (nanos != NO_TS)
        .then(|| Timestamp::from_nanosecond(nanos as i128).ok())
        .flatten()
}

/// Formats timestamps for one mode; the system time zone is resolved once.
pub struct TimeFormatter {
    mode: TimeMode,
    tz: TimeZone,
}

impl TimeFormatter {
    pub fn new(mode: TimeMode) -> Self {
        let tz = match mode {
            TimeMode::Local => TimeZone::system(),
            _ => TimeZone::UTC,
        };
        TimeFormatter { mode, tz }
    }

    #[cfg(test)]
    fn with_zone(mode: TimeMode, tz: TimeZone) -> Self {
        TimeFormatter { mode, tz }
    }

    pub fn mode(&self) -> TimeMode {
        self.mode
    }

    /// `None` when the mode hides time or the line has no timestamp.
    pub fn format(&self, nanos: i64) -> Option<String> {
        if self.mode == TimeMode::None {
            return None;
        }
        let ts = from_nanos(nanos)?;
        Some(match self.mode {
            TimeMode::Utc => ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            TimeMode::Local => ts
                .to_zoned(self.tz.clone())
                .strftime("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            TimeMode::None => unreachable!(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_kubelet_prefix() {
        let (ts, text) = split_timestamp("2026-11-19T15:20:25.865123456Z GET /health 200");
        assert_eq!(text, "GET /health 200");
        assert_eq!(
            ts,
            "2026-11-19T15:20:25.865123456Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_nanosecond() as i64
        );
    }

    #[test]
    fn keeps_lines_without_timestamp_and_empty_lines() {
        assert_eq!(split_timestamp("plain line"), (NO_TS, "plain line"));
        assert_eq!(
            split_timestamp("not-a-time rest"),
            (NO_TS, "not-a-time rest")
        );
        let (ts, text) = split_timestamp("2026-11-19T15:20:25Z");
        assert_ne!(ts, NO_TS);
        assert_eq!(text, "");
    }

    #[test]
    fn preserves_leading_whitespace_of_message() {
        let (_, text) = split_timestamp("2026-11-19T15:20:25Z   at Foo.bar(Foo.java:1)");
        assert_eq!(text, "  at Foo.bar(Foo.java:1)");
    }

    #[test]
    fn formats_utc_local_and_none() {
        let (ts, _) = split_timestamp("2026-11-19T15:20:25.865123456Z x");
        assert_eq!(TimeFormatter::new(TimeMode::None).format(ts), None);
        assert_eq!(
            TimeFormatter::new(TimeMode::Utc).format(ts).as_deref(),
            Some("2026-11-19T15:20:25.865Z")
        );
        let kyiv = TimeZone::get("Europe/Kyiv").unwrap();
        assert_eq!(
            TimeFormatter::with_zone(TimeMode::Local, kyiv)
                .format(ts)
                .as_deref(),
            Some("2026-11-19 17:20:25.865")
        );
        assert_eq!(TimeFormatter::new(TimeMode::Utc).format(NO_TS), None);
    }
}
