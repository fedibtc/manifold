use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;

/// Stable JSON error response envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ErrorResponse {
    /// Sanitized error details.
    pub error: ErrorBody,
}

impl ErrorResponse {
    /// Creates an error response.
    #[must_use]
    pub fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            error: ErrorBody { code, message },
        }
    }
}

/// Sanitized error code and message returned to clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ErrorBody {
    /// Stable machine-readable error code.
    pub code: &'static str,
    /// Stable human-readable error message.
    pub message: &'static str,
}

/// Sanitized API error with an HTTP status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiError {
    /// HTTP status returned to the client.
    pub status: StatusCode,
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Sanitized client-facing message.
    pub message: &'static str,
}

impl ApiError {
    /// Creates an API error.
    #[must_use]
    pub fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    /// Creates a bad-request validation error.
    #[must_use]
    pub fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    /// Creates a sanitized internal server error.
    #[must_use]
    pub fn internal_server_error() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorResponse::new(self.code, self.message)),
        )
            .into_response()
    }
}
