//! Service API traits for decentralized federation components.

pub use fedi_decentralized_domain as domain;
use serde::{Deserialize, Serialize};

/// Shared service result type.
pub type ServiceResult<T> = Result<T, ServiceError>;

/// Stable machine-readable service error code.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceErrorCode {
    /// The caller supplied an invalid argument.
    InvalidArgument,

    /// The authenticated caller is not allowed to perform the operation.
    PermissionDenied,

    /// The requested resource was not found or is intentionally hidden.
    NotFound,

    /// The service or one of its dependencies is temporarily unavailable.
    Unavailable,

    /// The request cannot be processed in the current service state.
    FailedPrecondition,

    /// The service hit an unexpected internal error.
    Internal,

    /// Legacy or uncategorized service error.
    Unknown,
}

impl ServiceErrorCode {
    /// Returns the stable lower-snake-case wire string for this code.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::FailedPrecondition => "failed_precondition",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }
}

impl core::fmt::Display for ServiceErrorCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned by service APIs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServiceError {
    code: ServiceErrorCode,
    message: String,
}

/// Lets generated RPC clients surface transport failures as the service's
/// normal error shape.
impl From<fedi_iroh_rpc::RpcError> for ServiceError {
    fn from(error: fedi_iroh_rpc::RpcError) -> Self {
        Self::with_code(
            ServiceErrorCode::Unavailable,
            format!("RPC transport error: {error}"),
        )
    }
}

impl ServiceError {
    /// Creates a new uncategorized service error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: ServiceErrorCode::Unknown,
            message: message.into(),
        }
    }

    /// Creates a new service error with a stable machine-readable code.
    #[must_use]
    pub fn with_code(code: ServiceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Returns the stable machine-readable error code.
    #[must_use]
    pub fn code(&self) -> ServiceErrorCode {
        self.code
    }

    /// Returns the error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl core::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_displays_message() {
        let err = ServiceError::new("boom");

        assert_eq!(err.code(), ServiceErrorCode::Unknown);
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn service_error_code_wire_strings_are_stable() {
        assert_eq!(
            ServiceErrorCode::InvalidArgument.to_string(),
            "invalid_argument"
        );
        assert_eq!(
            ServiceErrorCode::PermissionDenied.to_string(),
            "permission_denied"
        );
        assert_eq!(ServiceErrorCode::NotFound.to_string(), "not_found");
        assert_eq!(ServiceErrorCode::Unavailable.to_string(), "unavailable");
        assert_eq!(
            ServiceErrorCode::FailedPrecondition.to_string(),
            "failed_precondition"
        );
        assert_eq!(ServiceErrorCode::Internal.to_string(), "internal");
        assert_eq!(ServiceErrorCode::Unknown.to_string(), "unknown");
    }

    #[test]
    fn service_error_can_carry_code() {
        let err = ServiceError::with_code(ServiceErrorCode::NotFound, "missing");

        assert_eq!(err.code(), ServiceErrorCode::NotFound);
        assert_eq!(err.message(), "missing");
    }
}
