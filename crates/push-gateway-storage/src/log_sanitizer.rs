/// Returns a single-line sanitized value suitable for operational logs.
#[must_use]
pub(crate) fn sanitize_log_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}
