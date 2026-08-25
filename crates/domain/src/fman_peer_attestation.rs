//! FMan peer-attestation signed-object types.

#[cfg(test)]
mod tests;

use serde_json::json;
use sha2::{Digest, Sha256};
use stability_pool_common::Account;

use crate::{
    FederationId, GuardianIdentity, HashBytes, PeerId, ProtocolV1, Pubkey, SchnorrSignatureProof,
    Timestamp,
};

/// Canonical payload type string for FMan peer attestation signatures.
pub const FMAN_PEER_ATTESTATION_CANONICAL_TYPE: &str = "fedi.fman.peer-attestation";

/// Domain separator for FMan peer-attestation identity signatures.
///
/// The FMan service/advertisement key signs
/// [`FmanPeerAttestationStatement::digest`] output with this prefix. FLIPs
/// verify the proof under [`FmanPeerAttestationStatement::fman_pubkey`] before
/// using the attestation as a seat-to-FMan binding claim.
pub const FMAN_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR: &[u8] =
    b"fedi-fman-peer-attestation/v1\0";

/// Domain separator for a seat endpoint key's proof of an attestation.
pub const FMAN_SEAT_ENDPOINT_PROOF_DOMAIN: &[u8] = b"fedi-fman-seat-endpoint-proof/v1\0";

/// Proof that the endpoint key published for a peer endorses its FMan
/// attestation.
///
/// This is deliberately a sibling of [`FmanPeerAttestation`], not part of it:
/// the FMan service key attests the binding while the seat's independently
/// configured API endpoint key proves that binding at directory admission.
///
/// The proof names no peer of its own. The signature covers the attestation
/// digest, so the attestation's own `peer_id` is the authenticated one; a
/// second copy here could only ever disagree with it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatEndpointProof {
    /// Ed25519 signature over [`FmanPeerAttestationStatement::seat_endpoint_proof_message`].
    pub signature: Vec<u8>,
}

/// FMan-signed claim binding one FMan identity to one concrete Fedimint peer.
///
/// This follows the credential SDK signed-object convention used by
/// `HolderAuthorization`: a protocol version marker, a named statement object,
/// and a Schnorr proof over a type-tagged, versioned canonical statement
/// payload. The signature domain is
/// [`FMAN_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FmanPeerAttestation {
    /// Protocol version for this attestation shape.
    pub version: ProtocolV1,

    /// Statement signed by the FMan service/advertisement key.
    pub attestation: FmanPeerAttestationStatement,

    /// Schnorr signature proof over the domain-separated canonical statement.
    pub proof: SchnorrSignatureProof,
}

impl FmanPeerAttestation {
    /// Compute the signature digest for this FMan peer attestation.
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

    /// Verify this attestation's FMan service-key signature.
    ///
    /// This verifies only the SDK-style signed object itself: the signature must
    /// be valid under `attestation.fman_pubkey` for the canonical attestation
    /// payload. `attestation.fman_pubkey` must be the canonical lowercase hex
    /// serialization returned by `nostr::PublicKey::to_string()`; alternate
    /// encodings such as `npub`, NIP-21 URIs, or uppercase hex are invalid. FLIP
    /// policy remains responsible for matching the returned statement to an
    /// invite-code-previewed config peer and for fetching FMan trust material.
    ///
    /// # Errors
    ///
    /// Returns an error when the FMan pubkey is malformed, canonicalization
    /// fails, or the Schnorr signature does not verify.
    pub fn verify(
        &self,
    ) -> Result<FmanPeerAttestationStatement, FmanPeerAttestationVerificationError> {
        let public_key = nostr::PublicKey::parse(&self.attestation.fman_pubkey.0)
            .map_err(|_| FmanPeerAttestationVerificationError::InvalidFmanPubkey)?;
        if self.attestation.fman_pubkey.0 != public_key.to_string() {
            return Err(FmanPeerAttestationVerificationError::InvalidFmanPubkey);
        }
        let public_key = public_key
            .xonly()
            .map_err(|_| FmanPeerAttestationVerificationError::InvalidFmanPubkey)?;
        let message = nostr::secp256k1::Message::from_digest(
            self.digest()
                .map_err(|_| FmanPeerAttestationVerificationError::CanonicalizationFailed)?,
        );

        nostr::SECP256K1
            .verify_schnorr(&self.proof.signature, &message, &public_key)
            .map_err(|_| FmanPeerAttestationVerificationError::InvalidSignature)?;

        Ok(self.attestation.clone())
    }
}

/// Error returned when verifying an [`FmanPeerAttestation`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FmanPeerAttestationVerificationError {
    /// The attestation's `fman_pubkey` is not a valid canonical Nostr/secp256k1
    /// public key.
    InvalidFmanPubkey,

    /// The attestation statement could not be canonicalized for verification.
    CanonicalizationFailed,

    /// The Schnorr signature does not verify for the claimed FMan pubkey.
    InvalidSignature,
}

impl core::fmt::Display for FmanPeerAttestationVerificationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidFmanPubkey => f.write_str("invalid FMan pubkey"),
            Self::CanonicalizationFailed => f.write_str("attestation canonicalization failed"),
            Self::InvalidSignature => f.write_str("invalid FMan peer attestation signature"),
        }
    }
}

impl std::error::Error for FmanPeerAttestationVerificationError {}

/// Statement signed inside an [`FmanPeerAttestation`].
///
/// The statement means: FMan `fman_pubkey` attests it operates Fedimint
/// `peer_id` / `guardian_identity` in `federation_id` with final
/// `federation_config_hash`, and commits that seat to its guardian-fee account.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FmanPeerAttestationStatement {
    /// FMan service/advertisement public key that signs the attestation, encoded
    /// as canonical lowercase hex `nostr::PublicKey::to_string()` output.
    pub fman_pubkey: Pubkey,

    /// Fedimint federation id derived from the final config.
    pub federation_id: FederationId,

    /// Hash of the final federation config previewed from the invite code.
    pub federation_config_hash: HashBytes,

    /// Fedimint peer id operated by this FMan.
    pub peer_id: PeerId,

    /// Guardian or consensus identity for this peer in the final config.
    pub guardian_identity: GuardianIdentity,

    /// Full stability-pool account this seat accepted for guardian-fee revenue.
    ///
    /// The FI checks this signed value against the earlier service-key-signed
    /// seat acceptance before publishing the directory. Once the exact
    /// directory reaches consensus, every guardian can validate the complete
    /// recipient set without trusting a later FI-supplied account claim.
    pub guardian_fee_account: Account,

    /// Unix timestamp in seconds when the attestation was issued.
    pub issued_at: Timestamp,
}

impl FmanPeerAttestationStatement {
    /// Build JCS canonical bytes for this FMan peer attestation statement.
    ///
    /// The canonicalized value includes an explicit payload type, protocol
    /// version, and the signed statement. This mirrors the SDK
    /// holder-authorization canonicalization profile while using the FMan
    /// peer-attestation type string.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        canonicalize_statement(self)
    }

    /// Compute the signature digest for this FMan peer attestation statement.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn digest(&self) -> Result<[u8; 32], serde_json::Error> {
        let digest = Sha256::new()
            .chain_update(FMAN_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR)
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
        let mut message = Vec::with_capacity(FMAN_SEAT_ENDPOINT_PROOF_DOMAIN.len() + digest.len());
        message.extend_from_slice(FMAN_SEAT_ENDPOINT_PROOF_DOMAIN);
        message.extend_from_slice(&digest);
        Ok(message)
    }
}

/// Build JCS canonical bytes for an FMan peer attestation statement.
///
/// The canonicalized value includes a type string, protocol version, and the
/// statement signed by the FMan service/advertisement key.
///
/// # Errors
///
/// Returns an error if canonical JSON serialization fails.
fn canonicalize_statement(
    attestation: &FmanPeerAttestationStatement,
) -> serde_json::Result<Vec<u8>> {
    let payload = json!({
        "type": FMAN_PEER_ATTESTATION_CANONICAL_TYPE,
        "version": ProtocolV1,
        "attestation": attestation,
    });

    serde_json_canonicalizer::to_vec(&payload)
}
