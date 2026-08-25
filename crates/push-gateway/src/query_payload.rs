use axum::{
    extract::{FromRequestParts, Query},
    http::request::Parts,
};

use crate::ApiError;

/// Query-string extractor that maps parse failures to the stable API error envelope.
pub struct QueryPayload<T>(pub T);

impl<S, T> FromRequestParts<S> for QueryPayload<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_rejection| ApiError::bad_request("invalid_query", "query string is invalid"))
    }
}
