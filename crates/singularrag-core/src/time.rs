use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Never panics; pre-1970 clocks yield 0.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// UTC to the second, e.g. `2026-09-21T09:14:02Z`.
pub fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_now_is_utc_to_the_second() {
        let s = super::rfc3339_now();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T', "{s}");
    }
}
