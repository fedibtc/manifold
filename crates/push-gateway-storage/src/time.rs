use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current Unix timestamp in seconds.
#[must_use]
pub(crate) fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
