//! The sealed accounting payload a payer attaches to each guardian-fee
//! deposit.
//!
//! The payer (Fedi app) seals a breakdown of what the remittance was charged
//! for to the recipient account's public key, so only the operator holding
//! that key can read it. This module is the recipient half of that format;
//! the wire layout, ECDH construction, and domain string must stay
//! byte-compatible with the payer's implementation, so both directions live
//! here and a round-trip test pins them.

use bitcoin::secp256k1::{self, SECP256K1, ecdh::SharedSecret};
use fedimint_aead::LessSafeKey;
use fedimint_derive_secret::DerivableSecret;
use serde::{Deserialize, Serialize};

/// Domain separator mixed into the ECDH output. Shared with the payer.
const ECDH_DOMAIN: &[u8] = b"guardian-fee-remittance-ecdh";

/// One line of a remittance's accounting breakdown: what module and
/// direction of payer traffic the fee was charged on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemittanceBreakdownItem {
    pub module: String,
    pub direction: String,
    pub amount_msats: u64,
}

/// Plaintext accounting payload, versioned independently of its envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemittanceMetadata {
    pub version: u16,
    pub total_msats: u64,
    pub breakdown: Vec<RemittanceBreakdownItem>,
    pub remitted_at_unix: u64,
}

/// Ciphertext envelope as stored on the deposit's history item. It carries
/// the payer's ephemeral public key, which is what makes the deposit
/// readable by the recipient key alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemittanceCiphertext {
    version: u16,
    ephemeral_pubkey: secp256k1::PublicKey,
    ciphertext_hex: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RemittanceMetadataError {
    #[error("guardian remittance envelope is not valid JSON: {0}")]
    Envelope(#[source] serde_json::Error),
    #[error("unsupported guardian remittance envelope version {0}")]
    EnvelopeVersion(u16),
    #[error("guardian remittance ciphertext is not valid hex: {0}")]
    CiphertextHex(#[source] hex::FromHexError),
    #[error("guardian remittance ciphertext did not decrypt: {0}")]
    Decrypt(#[source] anyhow::Error),
    #[error("guardian remittance plaintext is not valid JSON: {0}")]
    Plaintext(#[source] serde_json::Error),
    #[error("unsupported guardian remittance metadata version {0}")]
    MetadataVersion(u16),
}

/// Read a deposit's sealed accounting payload with the recipient account key.
///
/// A deposit whose metadata does not decrypt is still a real deposit: callers
/// report the amount and surface the error rather than dropping the entry.
pub fn decrypt(
    account_key: &secp256k1::Keypair,
    sealed: &[u8],
) -> Result<RemittanceMetadata, RemittanceMetadataError> {
    let envelope: RemittanceCiphertext =
        serde_json::from_slice(sealed).map_err(RemittanceMetadataError::Envelope)?;
    if envelope.version != 1 {
        return Err(RemittanceMetadataError::EnvelopeVersion(envelope.version));
    }

    let key = shared_key(&envelope.ephemeral_pubkey, &account_key.secret_key());
    let mut ciphertext =
        hex::decode(&envelope.ciphertext_hex).map_err(RemittanceMetadataError::CiphertextHex)?;
    let plaintext =
        fedimint_aead::decrypt(&mut ciphertext, &key).map_err(RemittanceMetadataError::Decrypt)?;

    let metadata: RemittanceMetadata =
        serde_json::from_slice(plaintext).map_err(RemittanceMetadataError::Plaintext)?;
    if metadata.version != 1 {
        return Err(RemittanceMetadataError::MetadataVersion(metadata.version));
    }
    Ok(metadata)
}

/// The payer's half of the format. Kept beside [`decrypt`] so the two cannot
/// drift, and exercised by the round-trip test.
pub fn encrypt(
    recipient: &secp256k1::PublicKey,
    metadata: &RemittanceMetadata,
) -> anyhow::Result<Vec<u8>> {
    let plaintext = serde_json::to_vec(metadata)?;
    let (ephemeral_secret, ephemeral_pubkey) = SECP256K1.generate_keypair(&mut rand::thread_rng());
    let ciphertext = fedimint_aead::encrypt(plaintext, &shared_key(recipient, &ephemeral_secret))?;
    Ok(serde_json::to_vec(&RemittanceCiphertext {
        version: 1,
        ephemeral_pubkey,
        ciphertext_hex: hex::encode(ciphertext),
    })?)
}

fn shared_key(pubkey: &secp256k1::PublicKey, secret: &secp256k1::SecretKey) -> LessSafeKey {
    let shared_secret = SharedSecret::new(pubkey, secret);
    LessSafeKey::new(
        DerivableSecret::new_root(&shared_secret.secret_bytes(), ECDH_DOMAIN)
            .to_chacha20_poly1305_key(),
    )
}

#[cfg(test)]
#[path = "../tests/remittance_metadata.rs"]
mod tests;
