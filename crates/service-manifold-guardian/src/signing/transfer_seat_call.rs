//! Optional destination admission outside the unchanged signed transfer payload.

#[cfg(test)]
mod tests;

use fedi_decentralized_services::domain::HolderAuthorizationEnvelope;
use secp256k1::{SECP256K1, XOnlyPublicKey};
use sha2::{Digest, Sha256};

use super::{AuthError, MgSignedRequest, manager_request_signing_digest};
use crate::{ManagerSignature, Timestamp, TransferSeatRequest};

/// Maximum canonical proof size, in addition to the RPC transport's frame cap.
pub const DESTINATION_ADMISSION_MAX_BYTES: usize = 128 * 1024;

/// The badge subject's approval of one exact request and credential snapshot.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DestinationAdmissionProof {
    /// Proof protocol version; unsupported present versions must be rejected.
    pub version: u32,
    /// One holder-authorized credential, verified under source-owned policy.
    pub holder_authorization: HolderAuthorizationEnvelope,
    /// BIP-340 signature by the authorization's subject.
    pub signature: ManagerSignature,
}

impl std::fmt::Debug for DestinationAdmissionProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DestinationAdmissionProof")
            .finish_non_exhaustive()
    }
}

/// Transfer-specific extensible outer envelope, compatible with legacy decoders.
///
/// Direct fields deliberately preserve CBOR's binary key/signature decoding mode;
/// Serde flatten would lose it. Missing or null proof selects legacy admission.
/// The payload and service signature are moved intact, never reserialized.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TransferSeatCall {
    /// Original signed-envelope service key.
    service_pubkey: XOnlyPublicKey,
    /// Exact original signed JSON bytes.
    payload: Vec<u8>,
    /// Original service-key signature.
    manager_signature: ManagerSignature,
    /// Optional subject co-signature; invalid present proofs never fall back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination_proof: Option<DestinationAdmissionProof>,
}

/// Proof context obtainable only after authenticating the original request.
pub struct DestinationAdmission {
    /// Digest of the exact authenticated request bytes.
    request_digest: [u8; 32],
    /// Received subject proof; still requires complete trust verification.
    proof: DestinationAdmissionProof,
}

impl std::fmt::Debug for TransferSeatCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferSeatCall")
            .field("service_pubkey", &self.service_pubkey)
            .finish_non_exhaustive()
    }
}

impl DestinationAdmissionProof {
    /// Compute the domain-separated digest binding request and credential bytes.
    pub fn signing_digest(
        request: &MgSignedRequest<TransferSeatRequest>,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<[u8; 32], AuthError> {
        proof_digest(
            manager_request_signing_digest("transfer_seat", &request.payload),
            envelope,
        )
    }
}

fn proof_digest(
    request_digest: [u8; 32],
    envelope: &HolderAuthorizationEnvelope,
) -> Result<[u8; 32], AuthError> {
    let canonical = serde_json_canonicalizer::to_vec(envelope)
        .map_err(|err| AuthError::Serialize(err.to_string()))?;
    Ok(Sha256::new()
        .chain_update(b"fedi-fman-transfer-admission/v1\0")
        .chain_update(request_digest)
        .chain_update(canonical)
        .finalize()
        .into())
}

impl TransferSeatCall {
    /// Attach a subject proof without changing any service-signed bytes.
    pub fn new(
        request: MgSignedRequest<TransferSeatRequest>,
        destination_proof: Option<DestinationAdmissionProof>,
    ) -> Self {
        Self {
            service_pubkey: request.service_pubkey,
            payload: request.payload,
            manager_signature: request.manager_signature,
            destination_proof,
        }
    }

    /// Authenticate signer, exact payload, and freshness before exposing context.
    pub fn verify(
        self,
        now: Timestamp,
    ) -> Result<(TransferSeatRequest, Option<DestinationAdmission>), AuthError> {
        let request = MgSignedRequest {
            service_pubkey: self.service_pubkey,
            payload: self.payload,
            manager_signature: self.manager_signature,
            marker: std::marker::PhantomData,
        };
        let verified = request.verify(now)?;
        let admission = self.destination_proof.map(|proof| DestinationAdmission {
            request_digest: manager_request_signing_digest("transfer_seat", &request.payload),
            proof,
        });
        Ok((verified, admission))
    }
}

impl From<MgSignedRequest<TransferSeatRequest>> for TransferSeatCall {
    fn from(request: MgSignedRequest<TransferSeatRequest>) -> Self {
        Self::new(request, None)
    }
}

impl DestinationAdmission {
    /// Bound size and version, then authenticate the subject's request co-signature.
    ///
    /// This is not credential admission: the caller must also verify issuance,
    /// authorization, source trust policy, and fresh issuer revocation.
    pub fn verify_subject(&self) -> Result<&HolderAuthorizationEnvelope, AuthError> {
        if self.proof.version != 1 {
            return Err(AuthError::BadSignature);
        }
        let size = serde_json_canonicalizer::to_vec(&self.proof)
            .map_err(|err| AuthError::Serialize(err.to_string()))?
            .len();
        if size > DESTINATION_ADMISSION_MAX_BYTES {
            return Err(AuthError::BadSignature);
        }
        let envelope = &self.proof.holder_authorization;
        let subject = envelope.holder_authorization.authorization.subject_pubkey.0;
        let key =
            XOnlyPublicKey::from_slice(&subject.to_bytes()).map_err(|_| AuthError::BadSignature)?;
        SECP256K1
            .verify_schnorr(
                &self.proof.signature.0,
                &proof_digest(self.request_digest, envelope)?,
                &key,
            )
            .map_err(|_| AuthError::BadSignature)?;
        Ok(envelope)
    }
}
