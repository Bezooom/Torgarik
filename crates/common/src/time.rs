//! Time utilities for trading.

use chrono::{DateTime, Utc};

/// Get the current UTC time as a formatted string.
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// Format a DateTime<Utc> as an ISO 8601 string.
pub fn format_iso(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Convert an ISO 8601 string back to a DateTime<Utc>.
pub fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_roundtrip() {
        let dt = now_utc();
        let s = format_iso(&dt);
        let parsed = parse_iso(&s).unwrap();
        let s2 = format_iso(&parsed);
        assert_eq!(s, s2);
    }
}
