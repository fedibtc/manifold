//! NIP-98-style HTTP authentication for push-gateway management endpoints.
//!
//! The authenticated recipient is always the signer public key from a kind
//! `27235` Nostr HTTP auth event, rendered as canonical lowercase hex. Events
//! must bind the exact HTTP method and the exact absolute URL constructed from
//! the configured public base URL plus the request path/query. Body-consuming
//! endpoints additionally require a `payload` tag equal to the SHA-256 hash of
//! the raw request body before JSON parsing.
//!
//! Accepted event ids are retained in a bounded, single-process in-memory replay
//! cache for the full freshness window. If the cache is full of still-fresh ids,
//! authentication fails closed rather than evicting reusable ids.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, FromRequest, FromRequestParts, Request},
    http::{HeaderMap, Method, StatusCode, header, request::Parts},
};
use base64::{Engine, engine::general_purpose};
use fedi_decentralized_service_fleet_manager::{
    GuardianTelemetryRegistrationRequest, MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ApiError, AppState, RecipientId};

const AUTH_EVENT_MAX_AGE_SECONDS: i64 = 60;
const AUTH_EVENT_FUTURE_SKEW_SECONDS: i64 = 5;
const AUTH_REPLAY_CACHE_MAX_ENTRIES: usize = 4096;
const MAX_AUTHENTICATED_BODY_BYTES: usize = 16 * 1024;

/// Completely verified telemetry registration. This concrete extractor never
/// exposes the intermediate "valid NIP-98 signer" state: it returns only after
/// live PeerBadge, attestation, invite/config, and signer bindings pass.
pub(crate) struct AuthenticatedTelemetryRegistration(
    pub crate::telemetry_receiver::VerifiedTelemetryRegistration,
);

/// Recipient authenticated by a NIP-98-style Nostr HTTP authorization event.
pub(crate) struct AuthenticatedRecipient(pub RecipientId);

/// JSON payload authenticated together with the recipient that signed it.
pub(crate) struct AuthenticatedJson<T> {
    pub recipient_id: RecipientId,
    pub payload: T,
}

/// Empty request body authenticated together with the recipient that signed it.
pub(crate) struct AuthenticatedEmptyBody(pub RecipientId);

impl FromRequestParts<AppState> for AuthenticatedRecipient {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let expected_url =
            expected_absolute_url(state, parts.uri.path_and_query().map(|pq| pq.as_str()))?;
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        let verified = verify_authorization(
            state,
            &parts.headers,
            peer,
            &parts.method,
            &expected_url,
            None,
        )
        .await?;
        Ok(Self(verified.recipient_id))
    }
}

impl FromRequest<AppState> for AuthenticatedEmptyBody {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        let expected_url =
            expected_absolute_url(state, parts.uri.path_and_query().map(|pq| pq.as_str()))?;
        let body = axum::body::to_bytes(body, MAX_AUTHENTICATED_BODY_BYTES)
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "request body is too large",
                )
            })?;
        let verified = verify_authorization(
            state,
            &parts.headers,
            peer,
            &parts.method,
            &expected_url,
            Some(&body),
        )
        .await?;
        if !body.is_empty() {
            return Err(ApiError::bad_request(
                "unexpected_body",
                "request body must be empty",
            ));
        }
        Ok(Self(verified.recipient_id))
    }
}

impl<T> FromRequest<AppState> for AuthenticatedJson<T>
where
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        let expected_url =
            expected_absolute_url(state, parts.uri.path_and_query().map(|pq| pq.as_str()))?;
        let body = axum::body::to_bytes(body, MAX_AUTHENTICATED_BODY_BYTES)
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "request body is too large",
                )
            })?;
        let verified = verify_authorization(
            state,
            &parts.headers,
            peer,
            &parts.method,
            &expected_url,
            Some(&body),
        )
        .await?;
        let payload = serde_json::from_slice(&body).map_err(|_| {
            ApiError::bad_request("invalid_json", "request body must be valid JSON")
        })?;
        Ok(Self {
            recipient_id: verified.recipient_id,
            payload,
        })
    }
}

impl FromRequest<AppState> for AuthenticatedTelemetryRegistration {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        let expected_url =
            expected_absolute_url(state, parts.uri.path_and_query().map(|pq| pq.as_str()))?;
        let body = axum::body::to_bytes(body, MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES)
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "request body is too large",
                )
            })?;
        let verified = verify_authorization_event(
            state,
            &parts.headers,
            peer,
            &parts.method,
            &expected_url,
            Some(&body),
        )
        .await?;
        let request: GuardianTelemetryRegistrationRequest =
            serde_json::from_slice(&body).map_err(|_| {
                ApiError::bad_request("invalid_json", "request body must be valid JSON")
            })?;
        let verified =
            crate::telemetry_receiver::verify_registration(state, verified.recipient_id, request)
                .await?;
        Ok(Self(verified))
    }
}

fn ensure_recipient_admitted(state: &AppState, recipient_id: &RecipientId) -> Result<(), ApiError> {
    if state.config().recipient_admitted(recipient_id) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "recipient_not_admitted",
        "recipient is not admitted",
    ))
}

#[derive(Clone, Debug)]
struct VerifiedNostrAuth {
    recipient_id: RecipientId,
}

async fn verify_authorization(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    method: &Method,
    expected_url: &str,
    body: Option<&Bytes>,
) -> Result<VerifiedNostrAuth, ApiError> {
    let verified =
        verify_authorization_event(state, headers, peer, method, expected_url, body).await?;
    ensure_recipient_admitted(state, &verified.recipient_id)?;
    Ok(verified)
}

/// Common NIP-98 cryptographic/replay/source validation. This is private and
/// the only non-mobile caller is the concrete telemetry extractor above,
/// which immediately performs the stronger live FMan trust checks before
/// returning any value to a handler.
async fn verify_authorization_event(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    method: &Method,
    expected_url: &str,
    body: Option<&Bytes>,
) -> Result<VerifiedNostrAuth, ApiError> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(auth_failed)?;
    let event = decode_authorization_event(authorization)?;

    verify_event_shape_and_signature(&event.raw_pubkey, &event.event)?;
    verify_tags(&event.event, method, expected_url, body)?;
    verify_timestamp(&event.event)?;
    let recipient_id = RecipientId(event.event.pubkey.to_string());
    enforce_auth_source_budget(state, headers, peer).await?;

    state
        .record_nostr_auth_event(
            &event.event.id.to_string(),
            event.event.created_at.as_secs(),
        )
        .await
        .map_err(|err| match err {
            NostrAuthReplayError::Replayed => ApiError::new(
                StatusCode::UNAUTHORIZED,
                "auth_replay",
                "authorization event was already used",
            ),
            NostrAuthReplayError::CacheFull => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_replay_cache_full",
                "authorization replay cache is full",
            ),
        })?;

    Ok(VerifiedNostrAuth { recipient_id })
}

async fn enforce_auth_source_budget(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Result<(), ApiError> {
    let limits = state.config().rate_limits();
    if limits.auth_events_per_source_prefix == 0 || limits.auth_event_window_seconds == 0 {
        return Ok(());
    }
    let source = crate::rate_limits::effective_source_prefix(
        peer,
        headers,
        limits.trusted_proxy_cidrs.as_ref(),
    );
    if state
        .rate_limiters()
        .check(
            crate::rate_limits::RateLimitFamily::AuthSource,
            source,
            limits.auth_events_per_source_prefix,
            limits.auth_event_window_seconds,
        )
        .await
    {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "auth_source_rate_limited",
        "authentication source rate limited",
    ))
}

struct DecodedAuthorizationEvent {
    raw_pubkey: String,
    event: nostr::Event,
}

fn decode_authorization_event(authorization: &str) -> Result<DecodedAuthorizationEvent, ApiError> {
    let Some(encoded) = authorization.strip_prefix("Nostr ") else {
        return Err(auth_failed());
    };
    if encoded.is_empty() {
        return Err(auth_failed());
    }
    let decoded = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| auth_failed())?;
    let value: Value = serde_json::from_slice(&decoded).map_err(|_| auth_failed())?;
    let raw_pubkey = value
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or_else(auth_failed)?
        .to_owned();
    let event: nostr::Event = serde_json::from_value(value).map_err(|_| auth_failed())?;
    Ok(DecodedAuthorizationEvent { raw_pubkey, event })
}

fn verify_event_shape_and_signature(
    raw_pubkey: &str,
    event: &nostr::Event,
) -> Result<(), ApiError> {
    if event.kind != nostr::Kind::HttpAuth {
        return Err(auth_failed());
    }
    if raw_pubkey != event.pubkey.to_string() {
        return Err(auth_failed());
    }
    event.verify().map_err(|_| auth_failed())?;
    Ok(())
}

fn verify_tags(
    event: &nostr::Event,
    method: &Method,
    expected_url: &str,
    body: Option<&Bytes>,
) -> Result<(), ApiError> {
    let Some(url) = tag_value(event, "u") else {
        return Err(auth_failed());
    };
    if reqwest::Url::parse(url).is_err() || url != expected_url {
        return Err(auth_failed());
    }
    let Some(authorized_method) = tag_value(event, "method") else {
        return Err(auth_failed());
    };
    if authorized_method != method.as_str() {
        return Err(auth_failed());
    }
    if let Some(body) = body {
        let Some(payload) = tag_value(event, "payload") else {
            return Err(auth_failed());
        };
        let body_hash = hex::encode(Sha256::digest(body));
        if payload != body_hash {
            return Err(auth_failed());
        }
    }
    Ok(())
}

fn tag_value<'a>(event: &'a nostr::Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|tag| {
        let values = tag.as_slice();
        (values.len() >= 2 && values[0] == name).then_some(values[1].as_str())
    })
}

fn verify_timestamp(event: &nostr::Event) -> Result<(), ApiError> {
    let now = crate::time::unix_timestamp();
    let created_at = i64::try_from(event.created_at.as_secs()).map_err(|_| auth_failed())?;
    if created_at < now - AUTH_EVENT_MAX_AGE_SECONDS {
        return Err(auth_failed());
    }
    if created_at > now + AUTH_EVENT_FUTURE_SKEW_SECONDS {
        return Err(auth_failed());
    }
    Ok(())
}

fn expected_absolute_url(
    state: &AppState,
    path_and_query: Option<&str>,
) -> Result<String, ApiError> {
    let public_base_url = state.config().public_base_url();
    if public_base_url.is_empty() {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "auth_base_url_not_configured",
            "public base URL is not configured for signed request auth",
        ));
    }
    Ok(format!(
        "{public_base_url}{}",
        path_and_query.unwrap_or("/")
    ))
}

fn auth_failed() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_nostr_auth",
        "invalid Nostr authorization",
    )
}

/// Bounded in-memory replay cache for accepted Nostr auth event ids.
#[derive(Clone, Default)]
pub(crate) struct NostrAuthReplayCache {
    inner: Arc<tokio::sync::Mutex<NostrAuthReplayCacheInner>>,
}

#[derive(Default)]
struct NostrAuthReplayCacheInner {
    entries: HashMap<String, i64>,
    order: VecDeque<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum NostrAuthReplayError {
    Replayed,
    CacheFull,
}

impl NostrAuthReplayCache {
    pub async fn record_unused(
        &self,
        event_id: &str,
        created_at: u64,
    ) -> Result<(), NostrAuthReplayError> {
        let now = crate::time::unix_timestamp();
        let created_at = i64::try_from(created_at).map_err(|_| NostrAuthReplayError::Replayed)?;
        let expires_at = created_at
            .saturating_add(AUTH_EVENT_MAX_AGE_SECONDS)
            .saturating_add(AUTH_EVENT_FUTURE_SKEW_SECONDS)
            .max(now.saturating_add(1));
        let mut inner = self.inner.lock().await;
        inner.prune(now);
        if inner.entries.contains_key(event_id) {
            return Err(NostrAuthReplayError::Replayed);
        }
        if inner.entries.len() >= AUTH_REPLAY_CACHE_MAX_ENTRIES {
            return Err(NostrAuthReplayError::CacheFull);
        }
        inner.entries.insert(event_id.to_owned(), expires_at);
        inner.order.push_back(event_id.to_owned());
        Ok(())
    }
}

impl NostrAuthReplayCacheInner {
    fn prune(&mut self, now: i64) {
        while let Some(event_id) = self.order.front() {
            if self
                .entries
                .get(event_id)
                .is_some_and(|expires_at| *expires_at > now)
            {
                break;
            }
            if let Some(event_id) = self.order.pop_front() {
                self.entries.remove(&event_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replay_cache_fails_closed_instead_of_evicting_fresh_ids() {
        let cache = NostrAuthReplayCache::default();
        let now = crate::time::unix_timestamp() as u64;
        for i in 0..AUTH_REPLAY_CACHE_MAX_ENTRIES {
            cache
                .record_unused(&format!("event-{i}"), now)
                .await
                .expect("record fresh event");
        }

        assert_eq!(
            cache.record_unused("overflow", now).await,
            Err(NostrAuthReplayError::CacheFull)
        );
        assert_eq!(
            cache.record_unused("event-0", now).await,
            Err(NostrAuthReplayError::Replayed)
        );
    }
}
