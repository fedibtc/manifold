use serde::Serialize;

/// Response returned when revoking a notification hook.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RevokeHookResponse {
    /// Whether the hook is revoked after the request.
    pub revoked: bool,
}

impl RevokeHookResponse {
    /// Creates a revoked response.
    #[must_use]
    pub fn revoked() -> Self {
        Self { revoked: true }
    }
}
