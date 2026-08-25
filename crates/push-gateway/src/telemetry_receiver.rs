//! Verified FMan telemetry registration and durable target admission.

use std::str::FromStr as _;

use axum::Json;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_push_gateway_storage::{EncryptedTelemetryTarget, TelemetryRepository};
use fedi_decentralized_service_fleet_manager::{
    GuardianTelemetryRegistrationRequest, GuardianTelemetryRegistrationResponse,
};
use fedi_iroh_rpc::iroh::{Endpoint, EndpointId, endpoint::presets};
use tokio::sync::OnceCell;

use crate::{
    ApiError, AppState, RecipientId, TelemetryReceiverConfig,
    nostr_http_auth::AuthenticatedTelemetryRegistration, telemetry_crypto::TelemetrySecretCipher,
};

/// Fully verified FMan target. Construction is private to the concrete NIP-98
/// extractor, so no handler can persist a merely signed body.
pub(crate) struct VerifiedTelemetryRegistration {
    request: GuardianTelemetryRegistrationRequest,
    fman_pubkey: String,
}

/// Receiver/collector dependencies instantiated only for complete config.
pub(crate) struct TelemetryRuntime {
    verifier: PeerBadgeVerifier,
    repository: TelemetryRepository,
    cipher: TelemetrySecretCipher,
    collector_endpoint: OnceCell<Endpoint>,
}

impl TelemetryRuntime {
    pub(crate) async fn new(
        config: &TelemetryReceiverConfig,
        database: &crate::Database,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let verifier = PeerBadgeVerifier::try_from_profile(&config.environment().profile()?)?;
        Ok(Self {
            verifier,
            repository: TelemetryRepository::new(database.pool().clone()),
            cipher: TelemetrySecretCipher::new(config.encryption_key()),
            collector_endpoint: OnceCell::new(),
        })
    }

    pub(crate) fn repository(&self) -> &TelemetryRepository {
        &self.repository
    }

    pub(crate) fn cipher(&self) -> &TelemetrySecretCipher {
        &self.cipher
    }

    pub(crate) async fn collector_endpoint(
        &self,
    ) -> Result<&Endpoint, fedi_iroh_rpc::iroh::endpoint::BindError> {
        self.collector_endpoint
            .get_or_try_init(|| async { Endpoint::builder(presets::N0).bind().await })
            .await
    }
}

/// Verify the Holder credential after NIP-98 has bound its subject to the exact
/// endpoint and capability in this request.
pub(crate) async fn verify_registration(
    state: &AppState,
    signer: RecipientId,
    request: GuardianTelemetryRegistrationRequest,
) -> Result<VerifiedTelemetryRegistration, ApiError> {
    let runtime = state
        .telemetry_runtime()
        .ok_or_else(telemetry_unavailable)?;
    EndpointId::from_str(&request.iroh_endpoint_id).map_err(|_| invalid_registration())?;

    let badge = runtime
        .verifier
        .verify(&request.holder_authorization)
        .await
        .map_err(|_| invalid_registration())?;
    if badge.subject().0.to_string() != signer.0 {
        return Err(invalid_registration());
    }

    Ok(VerifiedTelemetryRegistration {
        request,
        fman_pubkey: signer.0,
    })
}

/// Persist one completely verified FMan target.
pub(crate) async fn register_guardian_telemetry(
    state: axum::extract::State<AppState>,
    AuthenticatedTelemetryRegistration(verified): AuthenticatedTelemetryRegistration,
) -> Result<Json<GuardianTelemetryRegistrationResponse>, ApiError> {
    let runtime = state
        .telemetry_runtime()
        .ok_or_else(telemetry_unavailable)?;
    let aad = format!("guardian-telemetry:{}", verified.fman_pubkey);
    let plaintext = serde_json::to_vec(&(
        &verified.request.iroh_endpoint_id,
        verified.request.capability.as_bytes(),
    ))
    .map_err(|_| internal_error())?;
    let (secret_nonce, secret_ciphertext) = runtime
        .cipher
        .encrypt(&plaintext, aad.as_bytes())
        .map_err(|_| internal_error())?;
    runtime
        .repository
        .upsert_verified_target(&EncryptedTelemetryTarget {
            fman_pubkey: verified.fman_pubkey,
            secret_nonce,
            secret_ciphertext,
        })
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "telemetry target persistence failed");
            internal_error()
        })?;
    Ok(Json(GuardianTelemetryRegistrationResponse {
        version: fedi_decentralized_domain::ProtocolV1,
    }))
}

fn invalid_registration() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "telemetry_registration_refused",
        "telemetry registration was refused",
    )
}

fn telemetry_unavailable() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "telemetry_unavailable",
        "telemetry receiver is unavailable",
    )
}

fn internal_error() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "internal server error",
    )
}
