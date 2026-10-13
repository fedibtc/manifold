//! MG peer-attestation signed-object types.

#[cfg(test)]
mod tests;

use serde_json::json;
use sha2::{Digest, Sha256};
use stability_pool_common::Account;

use crate::{
    FederationId, GuardianIdentity, HashBytes, PeerId, ProtocolV1, Pubkey, SchnorrSignatureProof,
    Timestamp,
};

/// Canonical payload type string for MG peer attestation signatures.
pub const MG_PEER_ATTESTATION_CANONICAL_TYPE: &str = "fedi.fman.peer-attestation";

/// Domain separator for MG peer-attestation identity signatures.
///
/// The MG service/advertisement key signs
/// [`MgPeerAttestationStatement::digest`] output with this prefix. FLIPs
/// verify the proof under [`MgPeerAttestationStatement::mg_pubkey`] before
/// using the attestation as a seat-to-MG binding claim.
pub const MG_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR: &[u8] =
    b"fedi-fman-peer-attestation/v1\0";

/// Domain separator for a seat endpoint key's proof of an attestation.
pub const MG_SEAT_ENDPOINT_PROOF_DOMAIN: &[u8] = b"fedi-fman-seat-endpoint-proof/v1\0";

/// Proof that the endpoint key published for a peer endorses its MG
/// attestation.
///
/// This is deliberately a sibling of [`MgPeerAttestation`], not part of it:
/// the MG service key attests the binding while the seat's independently
/// configured API endpoint key proves that binding at directory admission.
///
/// The proof names no peer of its own. The signature covers the attestation
/// digest, so the attestation's own `peer_id` is the authenticated one; a
/// second copy here could only ever disagree with it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatEndpointProof {
    /// Ed25519 signature over [`MgPeerAttestationStatement::seat_endpoint_proof_message`].
    pub signature: Vec<u8>,
}

/// MG-signed claim binding one MG identity to one concrete Fedimint peer.
///
/// This follows the PeerBadge SDK signed-object convention used by
/// `HolderAuthorization`: a protocol version marker, a named statement object,
/// and a Schnorr proof over a type-tagged, versioned canonical statement
/// payload. The signature domain is
/// [`MG_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MgPeerAttestation {
    /// Protocol version for this attestation shape.
    pub version: ProtocolV1,

    /// Statement signed by the MG service/advertisement key.
    pub attestation: MgPeerAttestationStatement,

    /// Schnorr signature proof over the domain-separated canonical statement.
    pub proof: SchnorrSignatureProof,
}

impl MgPeerAttestation {
    /// Compute the signature digest for this MG peer attestation.
    ///
    /// The digest signs the nested statement using the same type-tagged
    /// statement-canonicalization pattern as SDK `HolderAuthorization`.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn digest(&self) -> Result<[u8; 32], serde_json::Error> {
        self.attestation.digest()
    }

    /// Verify this attestation's MG service-key signature.
    ///
    /// This verifies only the SDK-style signed object itself: the signature must
    /// be valid under `attestation.fman_pubkey` for the canonical attestation
    /// payload. `attestation.fman_pubkey` must be the canonical lowercase hex
    /// serialization returned by `nostr::PublicKey::to_string()`; alternate
    /// encodings such as `npub`, NIP-21 URIs, or uppercase hex are invalid. FLIP
    /// policy remains responsible for matching the returned statement to an
    /// invite-code-previewed config peer and for fetching MG trust material.
    ///
    /// # Errors
    ///
    /// Returns an error when the MG pubkey is malformed, canonicalization
    /// fails, or the Schnorr signature does not verify.
    pub fn verify(&self) -> Result<MgPeerAttestationStatement, MgPeerAttestationVerificationError> {
        let public_key = nostr::PublicKey::parse(&self.attestation.mg_pubkey.0)
            .map_err(|_| MgPeerAttestationVerificationError::InvalidMgPubkey)?;
        if self.attestation.mg_pubkey.0 != public_key.to_string() {
            return Err(MgPeerAttestationVerificationError::InvalidMgPubkey);
        }
        let public_key = public_key
            .xonly()
            .map_err(|_| MgPeerAttestationVerificationError::InvalidMgPubkey)?;
        let message = nostr::secp256k1::Message::from_digest(
            self.digest()
                .map_err(|_| MgPeerAttestationVerificationError::CanonicalizationFailed)?,
        );

        nostr::SECP256K1
            .verify_schnorr(&self.proof.signature, &message, &public_key)
            .map_err(|_| MgPeerAttestationVerificationError::InvalidSignature)?;

        Ok(self.attestation.clone())
    }
}

/// Error returned when verifying an [`MgPeerAttestation`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MgPeerAttestationVerificationError {
    /// The attestation's `fman_pubkey` is not a valid canonical Nostr/secp256k1
    /// public key.
    InvalidMgPubkey,

    /// The attestation statement could not be canonicalized for verification.
    CanonicalizationFailed,

    /// The Schnorr signature does not verify for the claimed MG pubkey.
    InvalidSignature,
}

impl core::fmt::Display for MgPeerAttestationVerificationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMgPubkey => f.write_str("invalid MG pubkey"),
            Self::CanonicalizationFailed => f.write_str("attestation canonicalization failed"),
            Self::InvalidSignature => f.write_str("invalid MG peer attestation signature"),
        }
    }
}

impl std::error::Error for MgPeerAttestationVerificationError {}

/// Statement signed inside an [`MgPeerAttestation`].
///
/// The statement means: MG `fman_pubkey` attests it operates Fedimint
/// `peer_id` / `guardian_identity` in `federation_id` with final
/// `federation_config_hash`, and commits that seat to its guardian-fee account.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MgPeerAttestationStatement {
    /// MG service/advertisement public key that signs the attestation, encoded
    /// as canonical lowercase hex `nostr::PublicKey::to_string()` output.
    #[serde(rename = "fman_pubkey")]
    pub mg_pubkey: Pubkey,

    /// Fedimint federation id derived from the final config.
    pub federation_id: FederationId,

    /// Hash of the final federation config previewed from the invite code.
    pub federation_config_hash: HashBytes,

    /// Fedimint peer id operated by this MG.
    pub peer_id: PeerId,

    /// Guardian or consensus identity for this peer in the final config.
    pub guardian_identity: GuardianIdentity,

    /// Full stability-pool account this seat accepted for guardian-fee revenue.
    ///
    /// The FA checks this signed value against the earlier service-key-signed
    /// seat acceptance before publishing the directory. Once the exact
    /// directory reaches consensus, every guardian can validate the complete
    /// recipient set without trusting a later FA-supplied account claim.
    pub guardian_fee_account: Account,

    /// Unix timestamp in seconds when the attestation was issued.
    pub issued_at: Timestamp,
}

impl MgPeerAttestationStatement {
    /// Build JCS canonical bytes for this MG peer attestation statement.
    ///
    /// The canonicalized value includes an explicit payload type, protocol
    /// version, and the signed statement. This mirrors the SDK
    /// holder-authorization canonicalization profile while using the MG
    /// peer-attestation type string.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        canonicalize_statement(self)
    }

    /// Compute the signature digest for this MG peer attestation statement.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn digest(&self) -> Result<[u8; 32], serde_json::Error> {
        let digest = Sha256::new()
            .chain_update(MG_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR)
            .chain_update(self.canonical_bytes()?)
            .finalize();

        Ok(digest.into())
    }

    /// Build the bytes signed by the seat's configured API endpoint key.
    ///
    /// # Errors
    ///
    /// Returns an error if the attestation statement cannot be canonicalized.
    pub fn seat_endpoint_proof_message(&self) -> Result<Vec<u8>, serde_json::Error> {
        let digest = self.digest()?;
        let mut message = Vec::with_capacity(MG_SEAT_ENDPOINT_PROOF_DOMAIN.len() + digest.len());
        message.extend_from_slice(MG_SEAT_ENDPOINT_PROOF_DOMAIN);
        message.extend_from_slice(&digest);
        Ok(message)
    }
}

/// Build JCS canonical bytes for an MG peer attestation statement.
///
/// The canonicalized value includes a type string, protocol version, and the
/// statement signed by the MG service/advertisement key.
///
/// # Errors
///
/// Returns an error if canonical JSON serialization fails.
fn canonicalize_statement(attestation: &MgPeerAttestationStatement) -> serde_json::Result<Vec<u8>> {
    let payload = json!({
        "type": MG_PEER_ATTESTATION_CANONICAL_TYPE,
        "version": ProtocolV1,
        "attestation": attestation,
    });

    serde_json_canonicalizer::to_vec(&payload)
}
