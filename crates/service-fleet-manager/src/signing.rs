//! Fleet Manager 0.1 authentication: sign-over-received-bytes envelopes.
//!
//! [`SignedRequest`] and [`SignedResponse`] carry `(signer id, payload bytes,
//! signature)` and both sides sign/verify the exact transported bytes before
//! the payload is parsed — nothing is ever re-serialized, so JSON
//! canonicalization can never drift between signer and verifier
//! (SPEC-signed-envelopes). `fleet-manager`
//! (the daemon) and `fi-cli` must both use these helpers so the scheme cannot
//! drift between them.
//!
//! Scheme: secp256k1 BIP-340 Schnorr (ARCH-fleet-manager-identity *Signature
//! scheme*). `FiId` is the hex-encoded 32-byte x-only FI public key (no
//! key registry); the FMan key is the locator `service_pubkey`. The signed
//! message is `SHA256(domain || verb label || \0 || payload)` (see
//! [`fi_request_signing_digest`] / [`manager_response_signing_digest`]),
//! with distinct domains per direction and a per-verb
//! label so a signature minted for one verb can never replay as another verb
//! whose payload happens to share the same shape (e.g. `GetStatus` vs
//! `GetInviteCode`).
//!
//! [`VerifiedFiRequest<T>`] is the daemon-side proof of FI verification;
//! [`SignatureVerified<T>`] is the FI-side proof of manager-response
//! verification. Their constructors are private to the corresponding
//! verification paths, so later layers cannot manufacture either proof.

use std::marker::PhantomData;

use secp256k1::{Keypair, SECP256K1, XOnlyPublicKey};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{
    CreateSeatRequest, CreateSeatResponse, FiId, FiSignature, GetDkgCodeRequest,
    GetFedimintStatsRequest, GetInviteCodeRequest, GetPeerAttestationRequest, GetQuoteResponse,
    GetStatusRequest, ManagerSignature, ProposeFormationMetaRequest, RegisterGatewayRequest,
    RestartDkgRequest, SetMetaFieldRequest, StartDkgRequest, Timestamp,
};

/// Domain separator for FI-signed requests. `v1` versions the signing
/// scheme itself (like the `fman/v1/*` derivation labels), not the
/// product release; it changes only on an incompatible scheme redesign.
pub const FI_REQUEST_SIGNATURE_DOMAIN: &[u8] = b"fedi-fman-fi-request/v1\0";

/// Domain separator for FMan-signed commitment responses.
pub const FMAN_RESPONSE_SIGNATURE_DOMAIN: &[u8] = b"fedi-fman-response/v1\0";

/// Freshness window for signed FI requests (SPEC-signed-envelopes: ±1 h).
/// It bounds but does not prevent replay (no nonce) — acceptable for 0.1.
pub const FI_REQUEST_FRESHNESS_WINDOW_SECS: u64 = 60 * 60;

/// Why an envelope failed to verify. The daemon logs the detail and answers
/// the wire with the deliberately coarse
/// [`crate::FleetManagerError::Unauthorized`].
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("payload is not a valid {expected} request")]
    Payload { expected: &'static str },
    #[error("signature does not verify")]
    BadSignature,
    #[error("payload signer id does not match the envelope signer id")]
    SignerMismatch,
    #[error("request timestamp outside the ±{FI_REQUEST_FRESHNESS_WINDOW_SECS}s freshness window")]
    Stale,
    #[error("serialize payload: {0}")]
    Serialize(String),
    #[error("sign payload: {0}")]
    Sign(String),
}

impl From<AuthError> for crate::FleetManagerError {
    fn from(_: AuthError) -> Self {
        crate::FleetManagerError::Unauthorized
    }
}

/// A request type carried inside [`SignedRequest`]: names its per-verb
/// signing label and exposes the fields the auth boundary checks.
pub trait FiSignedRequest: Serialize + DeserializeOwned {
    /// Per-verb signature domain label.
    const LABEL: &'static str;

    /// Freshness challenge timestamp.
    fn ts(&self) -> Timestamp;

    /// Signer identity: the hex-encoded x-only secp256k1 public key.
    fn fi_id(&self) -> &FiId;
}

/// An FI-signed verb that targets an already-created seat. The daemon's only
/// seat-selection path requires this marker, so a verb without an explicit
/// impl below has no seat authority at all. Implementing it for a new verb is
/// the deliberate grant of (ownership-checked) seat access.
pub trait SeatScopedFiRequest: FiSignedRequest {
    fn seat_id(&self) -> &crate::SeatId;
}

/// A commitment response type carried inside [`SignedResponse`].
pub trait ManagerSignedResponse: Serialize + DeserializeOwned {
    /// Per-verb signature domain label.
    const LABEL: &'static str;
}

/// One entry per FI-signed verb. Also emits the test-only label roster so
/// the uniqueness/NUL-freedom test below can never miss a verb added here.
macro_rules! fi_signed_requests {
    ($($ty:ty => $label:literal,)+) => {
        $(impl FiSignedRequest for $ty {
            const LABEL: &'static str = $label;

            fn ts(&self) -> Timestamp {
                self.ts
            }

            fn fi_id(&self) -> &FiId {
                &self.fi_id
            }
        })+

        #[cfg(test)]
        const FI_REQUEST_LABELS: &[&str] = &[$($label),+];
    };
}

fi_signed_requests! {
    CreateSeatRequest => "create_seat",
    GetDkgCodeRequest => "get_dkg_code",
    StartDkgRequest => "start_dkg",
    RestartDkgRequest => "restart_dkg",
    GetStatusRequest => "get_status",
    GetInviteCodeRequest => "get_invite_code",
    GetPeerAttestationRequest => "get_peer_attestation",
    SetMetaFieldRequest => "set_meta_field",
    ProposeFormationMetaRequest => "propose_formation_meta",
    RegisterGatewayRequest => "register_gateway",
    GetFedimintStatsRequest => "get_fedimint_stats",
}

/// Seat-scoped grants. `CreateSeatRequest` is deliberately absent: it
/// allocates a fresh seat and must never select an existing one.
macro_rules! seat_scoped_fi_requests {
    ($($ty:ty,)+) => {
        $(impl SeatScopedFiRequest for $ty {
            fn seat_id(&self) -> &crate::SeatId {
                &self.seat_id
            }
        })+
    };
}

seat_scoped_fi_requests! {
    GetDkgCodeRequest,
    StartDkgRequest,
    RestartDkgRequest,
    GetStatusRequest,
    GetInviteCodeRequest,
    GetPeerAttestationRequest,
    SetMetaFieldRequest,
    ProposeFormationMetaRequest,
    RegisterGatewayRequest,
    GetFedimintStatsRequest,
}

/// One entry per FMan-signed commitment response; same roster trick as
/// [`fi_signed_requests`]. Labels may repeat request labels — the two
/// directions use distinct signature domains.
macro_rules! manager_signed_responses {
    ($($ty:ty => $label:literal,)+) => {
        $(impl ManagerSignedResponse for $ty {
            const LABEL: &'static str = $label;
        })+

        #[cfg(test)]
        const MANAGER_RESPONSE_LABELS: &[&str] = &[$($label),+];
    };
}

manager_signed_responses! {
    GetQuoteResponse => "get_quote",
    CreateSeatResponse => "create_seat",
}

/// Wire envelope for an FI-signed request: the signer id, serde-JSON `payload`
/// bytes of `T`, plus the FI signature over exactly those bytes.
///
/// Fields are private so [`SignedRequest::verify`] is the only way to read
/// the payload — code outside this module cannot parse the bytes without the
/// signature check.
#[derive(serde::Deserialize, serde::Serialize, Clone, Eq, PartialEq)]
#[serde(bound(serialize = "", deserialize = ""))]
pub struct SignedRequest<T> {
    /// FI x-only public key. Duplicated outside the payload so the
    /// daemon can verify the signature before parsing attacker-controlled
    /// payload bytes; after parsing, the inner request's `fi_id` must match.
    fi_id: FiId,
    /// Serde-JSON bytes of the inner request.
    payload: Vec<u8>,
    /// FI BIP-340 signature over
    /// [`fi_request_signing_digest`]`(T::LABEL, payload)`.
    fi_signature: FiSignature,
    #[serde(skip)]
    marker: PhantomData<fn() -> T>,
}

impl<T> std::fmt::Debug for SignedRequest<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedRequest")
            .field("fi_id", &self.fi_id)
            .field("payload", &"<redacted>")
            .field("fi_signature", &"<redacted>")
            .finish()
    }
}

impl<T: FiSignedRequest> SignedRequest<T> {
    /// Serialize and sign `request` (the FI side).
    pub fn create(request: &T, key: &Keypair) -> Result<Self, AuthError> {
        Self::create_with_signer(request, |digest| {
            Ok(FiSignature(SECP256K1.sign_schnorr(&digest, key)))
        })
    }

    /// Serialize `request` once and ask an external FI identity to sign the
    /// exact protocol digest.
    ///
    /// This is the consumer-neutral counterpart of [`Self::create`]: mobile
    /// bridges and hardware-backed identities can keep the secret outside the
    /// protocol library while this type still owns serialization, domain
    /// separation, and envelope construction.
    pub fn create_with_signer(
        request: &T,
        signer: impl FnOnce([u8; 32]) -> Result<FiSignature, String>,
    ) -> Result<Self, AuthError> {
        let payload =
            serde_json::to_vec(request).map_err(|err| AuthError::Serialize(err.to_string()))?;
        let signature =
            signer(fi_request_signing_digest(T::LABEL, &payload)).map_err(AuthError::Sign)?;
        Ok(Self {
            fi_id: *request.fi_id(),
            payload,
            fi_signature: signature,
            marker: PhantomData,
        })
    }

    /// Verify the envelope against the received bytes (the daemon side).
    ///
    /// Order: take the signer key from the envelope, verify the signature over
    /// the received bytes, parse the payload, then check inner signer id and
    /// freshness against `now`. Identity binding (`fi_id == customer_id`
    /// recorded at `CreateSeat`) stays with the seat lookup, which is where the
    /// customer id lives.
    pub fn verify(&self, now: Timestamp) -> Result<VerifiedFiRequest<T>, AuthError> {
        SECP256K1
            .verify_schnorr(
                &self.fi_signature.0,
                &fi_request_signing_digest(T::LABEL, &self.payload),
                &self.fi_id.0,
            )
            .map_err(|_| AuthError::BadSignature)?;
        let request: T = serde_json::from_slice(&self.payload)
            .map_err(|_| AuthError::Payload { expected: T::LABEL })?;
        if request.fi_id() != &self.fi_id {
            return Err(AuthError::SignerMismatch);
        }
        if FI_REQUEST_FRESHNESS_WINDOW_SECS < now.0.abs_diff(request.ts().0) {
            return Err(AuthError::Stale);
        }
        Ok(VerifiedFiRequest { inner: request })
    }
}

/// Wire envelope for an FMan-signed commitment response: serde-JSON `payload`
/// bytes of `T` plus the FMan signature over exactly those bytes. The daemon
/// persists `(payload, signature)` verbatim and replays them on idempotent
/// retries, so a retry observes byte-identical proof material.
///
/// Fields are private so [`SignedResponse::verify`] is the only way to read
/// the payload; the daemon's persist path can borrow the raw parts via
/// [`SignedResponse::as_parts`] without introducing a second envelope type.
#[derive(serde::Deserialize, serde::Serialize, Clone, Eq, PartialEq)]
#[serde(bound(serialize = "", deserialize = ""))]
pub struct SignedResponse<T> {
    /// Serde-JSON bytes of the inner response.
    payload: Vec<u8>,
    /// FMan BIP-340 signature over
    /// [`manager_response_signing_digest`]`(T::LABEL, payload)`.
    manager_signature: ManagerSignature,
    #[serde(skip)]
    marker: PhantomData<fn() -> T>,
}

impl<T> std::fmt::Debug for SignedResponse<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedResponse")
            .field("payload", &"<redacted>")
            .field("manager_signature", &"<redacted>")
            .finish()
    }
}

impl<T: ManagerSignedResponse> SignedResponse<T> {
    /// Serialize and sign `response` (the daemon side, first serving).
    pub fn create(response: &T, key: &Keypair) -> Result<Self, AuthError> {
        let payload =
            serde_json::to_vec(response).map_err(|err| AuthError::Serialize(err.to_string()))?;
        let signature =
            SECP256K1.sign_schnorr(&manager_response_signing_digest(T::LABEL, &payload), key);
        Ok(Self {
            payload,
            manager_signature: ManagerSignature(signature),
            marker: PhantomData,
        })
    }

    /// Rebuild the envelope from persisted bytes (the daemon side, idempotent
    /// replay).
    pub fn from_parts(payload: Vec<u8>, manager_signature: ManagerSignature) -> Self {
        Self {
            payload,
            manager_signature,
            marker: PhantomData,
        }
    }

    /// Borrow the exact signed parts for durable storage without dismantling
    /// the envelope. This keeps the envelope as the single representation in
    /// daemon code while storage still writes its two database columns.
    pub fn as_parts(&self) -> (&[u8], &ManagerSignature) {
        (&self.payload, &self.manager_signature)
    }

    /// Take the raw `(payload, signature)` for persistence; the inverse of
    /// [`SignedResponse::from_parts`].
    pub fn into_parts(self) -> (Vec<u8>, ManagerSignature) {
        (self.payload, self.manager_signature)
    }

    /// Verify against the FMan's locator `service_pubkey` and parse the
    /// payload (the FI side, and the daemon re-verifying a presented quote).
    pub fn verify(
        &self,
        service_pubkey: &XOnlyPublicKey,
    ) -> Result<SignatureVerified<T>, AuthError> {
        SECP256K1
            .verify_schnorr(
                &self.manager_signature.0,
                &manager_response_signing_digest(T::LABEL, &self.payload),
                service_pubkey,
            )
            .map_err(|_| AuthError::BadSignature)?;
        let response: T = serde_json::from_slice(&self.payload)
            .map_err(|_| AuthError::Payload { expected: T::LABEL })?;
        Ok(SignatureVerified {
            inner: response,
            payload_sha256: payload_sha256(&self.payload),
        })
    }
}

/// Proof that an FI request verified against the exact received bytes, its
/// declared inner identity matched the signing key, and its timestamp was
/// fresh. Only [`SignedRequest::verify`] constructs one.
#[derive(Debug)]
pub struct VerifiedFiRequest<T> {
    inner: T,
}

impl<T> VerifiedFiRequest<T> {
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: FiSignedRequest> VerifiedFiRequest<T> {
    /// The verified signer. `verify` checked the inner `fi_id` against the
    /// envelope key the signature verified under, so the payload field *is*
    /// the signer identity.
    pub fn signer(&self) -> &FiId {
        self.inner.fi_id()
    }
}

impl<T> std::ops::Deref for VerifiedFiRequest<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

/// Proof that a manager response verified against the exact received bytes.
/// Only [`SignedResponse::verify`] constructs one. Carries the SHA-256 of the
/// signed payload, which for a quote response is the quote's identity — so an
/// id can only be derived from a verified quote.
#[derive(Debug)]
pub struct SignatureVerified<T> {
    inner: T,
    payload_sha256: [u8; 32],
}

impl<T> SignatureVerified<T> {
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// SHA-256 of the exact signed payload bytes.
    pub(crate) fn payload_sha256(&self) -> [u8; 32] {
        self.payload_sha256
    }
}

impl<T> std::ops::Deref for SignatureVerified<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

fn payload_sha256(payload: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(payload).into()
}

/// The 32-byte message an FI signs for a request envelope.
pub fn fi_request_signing_digest(label: &str, payload: &[u8]) -> [u8; 32] {
    signing_digest(FI_REQUEST_SIGNATURE_DOMAIN, label, payload)
}

/// The 32-byte message an FMan signs for a commitment response envelope.
pub fn manager_response_signing_digest(label: &str, payload: &[u8]) -> [u8; 32] {
    signing_digest(FMAN_RESPONSE_SIGNATURE_DOMAIN, label, payload)
}

/// `SHA256(domain || label || \0 || payload)`, signed as a 32-byte BIP-340
/// message. Fedimint signs the same shape (`SHA256(tag || bytes)` via
/// `Message::from_digest`, e.g. `fedimint-core`'s api_announcement), and a
/// fixed 32-byte message keeps the scheme reachable from bindings that only
/// expose digest-input Schnorr signing.
fn signing_digest(domain: &[u8], label: &str, payload: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain);
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(payload);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests;
