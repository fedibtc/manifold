use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use fedi_decentralized_push_gateway_provider::{ProviderFuture, PushProvider, PushProviderError};
use fedi_decentralized_push_gateway_types::{FcmRegistrationToken, Notification, PushRegistration};

use crate::{
    FcmProviderConfig, FirebaseCredentials,
    outbound_validation::{fcm_data_value, fcm_outbound_data, validate_fcm_outbound_notification},
};

const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const REFRESH_SKEW_SECONDS: u64 = 60;

/// FCM HTTP v1 push provider.
#[derive(Clone)]
pub struct FcmPushProvider {
    credentials: FirebaseCredentials,
    client: Client,
    send_url: String,
    token_cache: Arc<Mutex<Option<CachedAccessToken>>>,
    concurrency: Arc<Semaphore>,
}

impl FcmPushProvider {
    /// Creates an FCM provider from runtime configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime HTTP client cannot be initialized.
    pub fn new(config: &FcmProviderConfig) -> Result<Self, FcmProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| FcmProviderError::HttpClient)?;
        Ok(Self {
            credentials: config.credentials().clone(),
            client,
            send_url: format!(
                "{}/v1/projects/{}/messages:send",
                config.send_endpoint_base().trim_end_matches('/'),
                config.credentials().project_id()
            ),
            token_cache: Arc::new(Mutex::new(None)),
            concurrency: Arc::new(Semaphore::new(config.max_concurrency())),
        })
    }

    /// Creates an FCM provider with an already configured HTTP client.
    ///
    /// This is primarily used by fake-server tests that do not need system TLS
    /// root discovery inside hermetic CI sandboxes. Callers are responsible for
    /// configuring TLS roots and timeouts on the injected client; production
    /// code should use [`Self::new`].
    #[doc(hidden)]
    pub fn with_http_client(config: &FcmProviderConfig, client: Client) -> Self {
        Self {
            credentials: config.credentials().clone(),
            client,
            send_url: format!(
                "{}/v1/projects/{}/messages:send",
                config.send_endpoint_base().trim_end_matches('/'),
                config.credentials().project_id()
            ),
            token_cache: Arc::new(Mutex::new(None)),
            concurrency: Arc::new(Semaphore::new(config.max_concurrency())),
        }
    }

    async fn deliver_once(
        &self,
        registration: &PushRegistration,
        notification: &Notification,
    ) -> Result<(), PushProviderError> {
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_unavailable"))?;
        let request = FcmSendRequest::try_from_notification(registration, notification)?;
        let access_token = self.access_token().await?;
        let response = self
            .client
            .post(&self.send_url)
            .bearer_auth(access_token)
            .json(&request)
            .send()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_network"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.json::<FcmErrorEnvelope>().await.ok();
        Err(classify_fcm_error(status, body.as_ref()))
    }

    async fn validate_registration_once(
        &self,
        token: &FcmRegistrationToken,
    ) -> Result<(), PushProviderError> {
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_unavailable"))?;
        let access_token = self.access_token().await?;
        let response = self
            .client
            .post(&self.send_url)
            .bearer_auth(access_token)
            .json(&FcmSendRequest::validation(token))
            .send()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_network"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.json::<FcmErrorEnvelope>().await.ok();
        Err(classify_fcm_error(status, body.as_ref()))
    }

    async fn access_token(&self) -> Result<String, PushProviderError> {
        if let Some(token) = self.cached_token() {
            return Ok(token);
        }

        let assertion = self
            .jwt_assertion()
            .map_err(|_| PushProviderError::unavailable("provider_auth"))?;
        let response = self
            .client
            .post(self.credentials.token_uri())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_auth"))?;
        if !response.status().is_success() {
            return Err(PushProviderError::unavailable("provider_auth"));
        }
        let token = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(|_| PushProviderError::unavailable("provider_auth"))?;
        let expires_at = now_seconds().saturating_add(token.expires_in.unwrap_or(3600));
        let cached = CachedAccessToken {
            token: token.access_token,
            expires_at,
        };
        let token = cached.token.clone();
        *self.token_cache.lock().expect("fcm token cache mutex") = Some(cached);
        Ok(token)
    }

    fn cached_token(&self) -> Option<String> {
        self.token_cache
            .lock()
            .expect("fcm token cache mutex")
            .as_ref()
            .filter(|token| token.expires_at > now_seconds().saturating_add(REFRESH_SKEW_SECONDS))
            .map(|token| token.token.clone())
    }

    fn jwt_assertion(&self) -> Result<String, jsonwebtoken::errors::Error> {
        let now = now_seconds();
        let claims = OAuthJwtClaims {
            iss: self.credentials.client_email(),
            scope: FCM_SCOPE,
            aud: self.credentials.token_uri(),
            iat: now,
            exp: now + 3600,
        };
        encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(self.credentials.private_key().as_bytes())?,
        )
    }
}

impl PushProvider for FcmPushProvider {
    fn validate_registration<'a>(&'a self, token: &'a FcmRegistrationToken) -> ProviderFuture<'a> {
        Box::pin(async move { self.validate_registration_once(token).await })
    }

    fn deliver<'a>(
        &'a self,
        registration: &'a PushRegistration,
        notification: &'a Notification,
    ) -> ProviderFuture<'a> {
        Box::pin(async move { self.deliver_once(registration, notification).await })
    }
}

impl std::fmt::Debug for FcmPushProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FcmPushProvider")
            .field("credentials", &self.credentials)
            .field("send_url", &self.send_url)
            .field("token_cache", &"<redacted>")
            .field("concurrency", &self.concurrency.available_permits())
            .finish()
    }
}

/// Sanitized FCM provider construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FcmProviderError {
    /// HTTP client creation failed.
    HttpClient,
}

impl std::fmt::Display for FcmProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("failed to initialize FCM provider")
    }
}

impl std::error::Error for FcmProviderError {}

#[derive(Clone)]
struct CachedAccessToken {
    token: String,
    expires_at: u64,
}

#[derive(Serialize)]
struct OAuthJwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

#[derive(Serialize)]
struct FcmSendRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    validate_only: Option<bool>,
    message: FcmMessage<'a>,
}

#[derive(Serialize)]
struct FcmMessage<'a> {
    token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    notification: Option<FcmNotification<'a>>,
    data: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    android: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    apns: Option<Value>,
}

#[derive(Serialize)]
struct FcmNotification<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
}

impl<'a> FcmSendRequest<'a> {
    fn try_from_notification(
        registration: &'a PushRegistration,
        notification: &'a Notification,
    ) -> Result<Self, PushProviderError> {
        validate_fcm_outbound_notification(notification)
            .map_err(|_| PushProviderError::invalid_payload("invalid_payload"))?;
        let data = fcm_outbound_data(notification)
            .iter()
            .map(|(key, value)| (key.clone(), fcm_data_value(value)))
            .collect::<BTreeMap<_, _>>();

        Ok(Self {
            validate_only: None,
            message: FcmMessage {
                token: &registration.fcm_token.0,
                notification: if notification.title.is_some() || notification.body.is_some() {
                    Some(FcmNotification {
                        title: notification.title.as_deref(),
                        body: notification.body.as_deref(),
                    })
                } else {
                    None
                },
                data,
                android: Some(json!({
                    "priority": "HIGH",
                    "ttl": "3600s",
                })),
                apns: Some(json!({
                    "headers": {
                        "apns-priority": "10",
                        "apns-expiration": (now_seconds() + 3600).to_string(),
                    },
                })),
            },
        })
    }

    fn validation(token: &'a FcmRegistrationToken) -> Self {
        Self {
            validate_only: Some(true),
            message: FcmMessage {
                token: &token.0,
                notification: None,
                data: BTreeMap::from([("validation".to_owned(), "1".to_owned())]),
                android: None,
                apns: None,
            },
        }
    }
}

#[derive(Deserialize)]
struct FcmErrorEnvelope {
    error: Option<FcmError>,
}

#[derive(Deserialize)]
struct FcmError {
    details: Option<Vec<FcmErrorDetail>>,
}

#[derive(Deserialize)]
struct FcmErrorDetail {
    #[serde(rename = "@type")]
    kind: Option<String>,
    error_code: Option<String>,
}

fn classify_fcm_error(status: StatusCode, body: Option<&FcmErrorEnvelope>) -> PushProviderError {
    let detail_code = body
        .and_then(|body| body.error.as_ref())
        .and_then(|error| error.details.as_ref())
        .and_then(|details| {
            details.iter().find_map(|detail| {
                detail
                    .kind
                    .as_deref()
                    .filter(|kind| kind.contains("FcmError"))
                    .and(detail.error_code.as_deref())
            })
        });

    if matches!(detail_code, Some("UNREGISTERED" | "INVALID_ARGUMENT")) {
        return PushProviderError::invalid_token("invalid_token");
    }
    if status == StatusCode::BAD_REQUEST {
        return PushProviderError::invalid_payload("invalid_payload");
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return PushProviderError::unavailable("provider_auth");
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return PushProviderError::unavailable("provider_quota");
    }
    PushProviderError::unavailable("provider_transient")
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
