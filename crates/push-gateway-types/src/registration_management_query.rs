use serde::Deserialize;

/// Query string for temporary registration management endpoints.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RegistrationManagementQuery {
    /// Optional sanitized reason recorded when disabling a registration.
    pub reason: Option<String>,
}
