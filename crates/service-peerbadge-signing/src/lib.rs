//! PeerBadge signing protocol shared by native servers and clients.
//!
//! The versioned CBOR contract is pinned by `tests/golden.rs`. SDK issuance
//! documents remain JSON strings so clients can pass them through unchanged.

use fedi_iroh_rpc::{RpcError, service};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod info_serde;

pub const PEERBADGE_SIGNING_ALPN: &[u8] = b"fedi/peerbadge-signing/1";

/// Digest signed directly with BIP-340 by the allowlisted signer.
///
/// The NUL separator is part of the v1 domain, and the issuer binding prevents
/// a challenge issued by one server from authorizing a different issuer.
#[must_use]
pub fn challenge_digest(nonce: &[u8; 32], level: u8, issuer_id_pubkey: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"peerbadge-signing/open-session/1\0");
    hash.update(nonce);
    hash.update([level]);
    hash.update(issuer_id_pubkey);
    hash.finalize().into()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeResponse {
    #[serde(with = "serde_bytes")]
    pub nonce: Vec<u8>,
    /// Unix seconds.
    pub expires_at: u64,
    /// Server's x-only issuer public key as 64 hex characters.
    pub issuer_id_pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenSessionRequest {
    pub signer_pubkey: String,
    pub level: u8,
    #[serde(with = "serde_bytes")]
    pub nonce: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenSessionResponse {
    /// Bearer secret: 32 random bytes encoded as lowercase hex.
    pub session_id: String,
    #[serde(serialize_with = "info_serde::serialize")]
    pub info: serde_json::Value,
    /// IssuerAuthority JSON, exactly as published in kind 37703.
    pub issuer_authority: String,
    pub expires_at: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemSessionRequest {
    pub session_id: String,
    /// Opaque peerbadge-sdk IssuanceRequest JSON.
    pub request: String,
}

impl std::fmt::Debug for RedeemSessionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedeemSessionRequest")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemSessionResponse {
    /// Opaque peerbadge-sdk IssuanceResponse JSON.
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusResponse {
    pub state: SessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Open,
    Redeemed,
    Expired,
}

/// Wire errors must contain only public, fixed descriptions, never request
/// contents or raw SDK/parser diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SigningError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("level not allowed (maximum {max})")]
    LevelNotAllowed { max: u8 },
    #[error("invalid or expired challenge")]
    BadChallenge,
    #[error("rate limited")]
    RateLimited,
    #[error("session not found")]
    SessionNotFound,
    #[error("session expired")]
    SessionExpired,
    #[error("session already redeemed")]
    SessionAlreadyRedeemed,
    #[error("invalid issuance request")]
    InvalidRequest(String),
    #[error("RPC transport failure")]
    Transport(String),
}

impl From<RpcError> for SigningError {
    fn from(error: RpcError) -> Self {
        // A remote error or parser diagnostic can contain attacker-controlled
        // request data. Keep only its category at the service boundary.
        let message = match error {
            RpcError::RequestTooLarge => "request too large",
            RpcError::RequestTimedOut => "request timed out",
            RpcError::ResponseTooLarge => "response too large",
            RpcError::UnknownMethod(_) => "unknown RPC method",
            RpcError::Encode(_) => "RPC encoding failed",
            RpcError::Decode(_) => "RPC decoding failed",
            RpcError::Remote(_) => "remote RPC transport failure",
            RpcError::Iroh(_) => "iroh transport failure",
        };
        Self::Transport(message.to_owned())
    }
}

#[service]
pub trait PeerBadgeSigningService {
    async fn challenge(&self, req: ChallengeRequest) -> Result<ChallengeResponse, SigningError>;
    async fn open_session(
        &self,
        req: OpenSessionRequest,
    ) -> Result<OpenSessionResponse, SigningError>;
    async fn redeem_session(
        &self,
        req: RedeemSessionRequest,
    ) -> Result<RedeemSessionResponse, SigningError>;
    async fn session_status(
        &self,
        req: SessionStatusRequest,
    ) -> Result<SessionStatusResponse, SigningError>;
}
