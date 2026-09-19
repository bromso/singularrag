use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Never panics; pre-1970 clocks yield 0.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
