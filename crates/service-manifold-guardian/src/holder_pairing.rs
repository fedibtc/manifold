//! A private Holder submission, separate from public advertisements and from
//! guardian-link phone notifications. The capability never enters a signature.
//!
//! A compatible Holder app parses the versioned operator QR locally, confirms
//! the displayed environment and MG identity, then signs the existing Holder
//! authorization for `subject_pubkey` and packages it with the issuer
//! credential in a holder-signed kind-37705 event. It dials exactly
//! `endpoint_id`, using [`HOLDER_PAIRING_ALPN`], and passes that event to
//! `submit_holder_authorization`. It compares the receipt with the event ID it
//! sent. The existing `HolderPairingServiceClient`/server types use the normal
//! `fedi-iroh-rpc` framing. No relay publication, holder-app URL, redirect, or
//! capability in an event tag, content, advertisement, telemetry, or log is
//! part of pairing. A client unable to submit privately must not publish in its
//! place.

use fedi_decentralized_services::domain::ProtocolV1;
use fedi_iroh_rpc::service;
use serde::{Deserialize, Serialize};

use crate::LinkSecret;

/// A pairing-only endpoint advertises no other MG protocols.
pub const HOLDER_PAIRING_ALPN: &[u8] = b"fedi/mg/holder-pairing/1";

/// Operator-only QR contents, returned only when the host creates a grant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HolderPairingInvite {
    /// Distinct discriminator: a legacy subject-only QR is never a grant.
    pub protocol: HolderPairingProtocol,
    /// Deployment shown to the Holder before signing.
    pub environment: String,
    /// MG identity the authorization must name.
    pub subject_pubkey: nostr::PublicKey,
    /// Ephemeral, authenticated Iroh peer; no caller-provided submission URL.
    pub endpoint_id: String,
    /// Optional direct route for local pairing. It never replaces peer ID
    /// verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_address: Option<std::net::SocketAddr>,
    /// Single-use bearer, redacted in Debug.
    pub secret: LinkSecret,
    /// Absolute deadline in Unix milliseconds.
    pub expires_at_ms: i64,
}

/// Only the explicitly versioned pairing QR is accepted.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum HolderPairingProtocol {
    /// Version 1, on [`HOLDER_PAIRING_ALPN`].
    #[serde(rename = "mg_holder_pairing_v1")]
    V1,
}

/// Submit a signed kind-37705 event privately, not to Nostr. Keep the bearer
/// separate from both signed objects, including all event tags.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitHolderAuthorization {
    /// Wire schema version.
    pub version: ProtocolV1,
    /// Secret from the QR, only inside this encrypted request.
    pub secret: LinkSecret,
    /// Complete signed Nostr event serialized as JSON.
    pub event_json: String,
}

impl std::fmt::Debug for SubmitHolderAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SubmitHolderAuthorization([REDACTED])")
    }
}

/// Acknowledges the durable pin; never returns another bearer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HolderPairingReceipt {
    /// Exact accepted event ID.
    pub event_id: nostr::EventId,
}

/// No request data is included in wire errors.
#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum HolderPairingError {
    /// The grant was absent, rotated, expired, or consumed.
    #[error("invalid or expired pairing invitation")]
    InvalidInvite,
    /// The authenticated authorization or badge did not validate for this MG.
    #[error("authorization rejected")]
    InvalidAuthorization,
    /// The daemon failed to complete this operation.
    #[error("pairing unavailable")]
    Unavailable,
}

impl From<fedi_iroh_rpc::RpcError> for HolderPairingError {
    fn from(_: fedi_iroh_rpc::RpcError) -> Self {
        Self::Unavailable
    }
}

/// Reply to the submitting app.
pub type PairingResult<T> = Result<T, HolderPairingError>;

/// Only the client that scanned a currently live invitation may enroll.
#[service]
pub trait HolderPairingService {
    /// Validate, consume the grant and pin the authorization in one operation.
    async fn submit_holder_authorization(
        &self,
        request: SubmitHolderAuthorization,
    ) -> PairingResult<HolderPairingReceipt>;
}
