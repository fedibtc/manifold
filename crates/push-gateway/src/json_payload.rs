use axum::{
    Json,
    extract::{FromRequest, Request},
    http::StatusCode,
};

use crate::ApiError;

/// JSON extractor that maps all parse/body-limit failures to the stable API error envelope.
pub struct JsonPayload<T>(pub T);

impl<S, T> FromRequest<S> for JsonPayload<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| {
                let status = rejection.status();
                let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
                    "payload_too_large"
                } else {
                    "invalid_json"
                };
                let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                    "request body is too large"
                } else {
                    "request body must be valid JSON"
                };
                ApiError::new(status, code, message)
            })
    }
}
