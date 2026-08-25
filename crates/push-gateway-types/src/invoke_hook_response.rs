use serde::Serialize;

/// Stable response returned by the public hook invocation endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvokeHookResponse {
    /// Whether the invocation was accepted for fake/no-op delivery.
    pub accepted: bool,
    /// Sanitized reason code for clients and tests.
    pub reason: &'static str,
    /// Number of push registrations targeted.
    pub delivery_attempts: usize,
}

impl InvokeHookResponse {
    /// Creates an accepted invocation response.
    #[must_use]
    pub fn accepted(delivery_attempts: usize) -> Self {
        Self {
            accepted: true,
            reason: "accepted",
            delivery_attempts,
        }
    }
}
