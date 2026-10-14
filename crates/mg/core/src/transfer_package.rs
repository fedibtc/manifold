//! The bounded, source-authenticated guardian-transfer envelope.
//!
//! This codec does not authorize a transfer or validate a fedimint config.
//! Callers must verify the FA grant, ownership, durable lifecycle fences and
//! native config before accepting material. In particular, successful opening
//! does not permit starting a child or bypassing the backup publication barrier.
//!
//! V1 is `magic/version[8] || ephemeral SEC1 key[33] || grant hash[32] ||
//! nonce[12] || ciphertext || Poly1305 tag[16] || Schnorr signature[64]`.
//! The grant hash covers canonical JSON of every grant field. HKDF-SHA256 binds
//! the complete header to a fresh ephemeral ECDH secret. BIP-340 recipients are
//! lifted with even parity, and opening normalizes the recipient scalar to match.
//! The source service signs the domain-separated hash of header and complete
//! AEAD output, including nonce. Verify that signature before decryption.
//!
//! The plaintext carries exactly the four guardian config files and `api_auth`,
//! as five UTF-8 strings with big-endian u32 byte lengths. It cannot describe a
//! path, wallet, database, compression stream or additional archive entry.

use fedi_decentralized_service_manifold_guardian::{
    SEALED_SEAT_PACKAGE_MAX_BYTES, SealedSeatPackage, SeatTransferGrant,
};
use fedimint_aead::{LessSafeKey, UnboundKey};
use hkdf::Hkdf;
use secp256k1::{Keypair, Parity, PublicKey, SECP256K1, SecretKey, ecdh::SharedSecret};
use sha2::{Digest, Sha256};

use crate::backup::GuardianArchive;

const MAGIC: &[u8; 8] = b"FMTR\0\0\0\x01";
const HEADER_LEN: usize = 8 + 33 + 32;
const SIGNATURE_LEN: usize = 64;
const AEAD_OVERHEAD: usize = 12 + 16;
const KDF_DOMAIN: &[u8] = b"fman/guardian-transfer/v1/ecdh-hkdf";
const SIGNATURE_DOMAIN: &[u8] = b"fman/guardian-transfer/v1/source-signature\0";
const MAX_PLAINTEXT: usize =
    SEALED_SEAT_PACKAGE_MAX_BYTES - HEADER_LEN - SIGNATURE_LEN - AEAD_OVERHEAD;
/// A password cannot consume an arbitrary share of the bounded package.
const MAX_API_AUTH: usize = 1024;

/// Source-authenticated material awaiting native config and lifecycle validation.
///
/// Neither `Debug` nor serialization exposes these secrets accidentally. Backup
/// and durable installation have separate, deliberately chosen representations.
pub struct TransferMaterial {
    /// The immutable four-file guardian archive, never a consensus database.
    pub archive: GuardianArchive,
    /// The original guardian password, not a destination-derived replacement.
    pub api_auth: String,
}

impl std::fmt::Debug for TransferMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TransferMaterial(redacted)")
    }
}

/// A sanitized refusal that never includes credentials, config or parser input.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum PackageError {
    /// The fixed format, UTF-8 or plaintext field shape was invalid.
    #[error("invalid guardian transfer package")]
    Invalid,
    /// The package would exceed its existing RPC budget.
    #[error("guardian transfer package exceeds its size limit")]
    TooLarge,
    /// A local identity or the complete grant does not match this package.
    #[error("guardian transfer package binding mismatch")]
    Binding,
    /// Source signature verification or authenticated decryption failed.
    #[error("guardian transfer package authentication failed")]
    Authentication,
}

impl TransferMaterial {
    /// Read only the bounded immutable config, never the guardian database.
    /// Oversize/missing files fail before source retirement is committed.
    pub(crate) async fn read(
        data_dir: &std::path::Path,
        api_auth: String,
    ) -> Result<Self, PackageError> {
        use tokio::io::AsyncReadExt;
        let mut remaining = MAX_PLAINTEXT.saturating_sub(20 + api_auth.len());
        let mut fields = Vec::with_capacity(4);
        for name in [
            "private.encrypt",
            "private.salt",
            "local.json",
            "consensus.json",
        ] {
            let mut bytes = Vec::new();
            tokio::fs::File::open(data_dir.join(name))
                .await
                .map_err(|_| PackageError::Invalid)?
                .take(remaining as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| PackageError::Invalid)?;
            remaining = remaining
                .checked_sub(bytes.len())
                .ok_or(PackageError::TooLarge)?;
            fields.push(String::from_utf8(bytes).map_err(|_| PackageError::Invalid)?);
        }
        let mut fields = fields.into_iter();
        let material = Self {
            archive: GuardianArchive {
                private_encrypt: fields.next().expect("four fields"),
                private_salt: fields.next().expect("four fields"),
                local_json: fields.next().expect("four fields"),
                consensus_json: fields.next().expect("four fields"),
            },
            api_auth,
        };
        material.check_size()?;
        Ok(material)
    }

    /// Check representability before retiring a source or allocating ciphertext.
    pub fn check_size(&self) -> Result<(), PackageError> {
        if self.api_auth.is_empty() || MAX_API_AUTH < self.api_auth.len() {
            return Err(PackageError::Invalid);
        }
        let mut total: usize = 5 * 4;
        for field in self.fields() {
            total = total
                .checked_add(field.len())
                .ok_or(PackageError::TooLarge)?;
        }
        if MAX_PLAINTEXT < total {
            return Err(PackageError::TooLarge);
        }
        Ok(())
    }

    fn fields(&self) -> [&str; 5] {
        [
            &self.archive.private_encrypt,
            &self.archive.private_salt,
            &self.archive.local_json,
            &self.archive.consensus_json,
            &self.api_auth,
        ]
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, PackageError> {
        self.check_size()?;
        let mut bytes = Vec::new();
        for field in self.fields() {
            bytes.extend_from_slice(&(field.len() as u32).to_be_bytes());
            bytes.extend_from_slice(field.as_bytes());
        }
        Ok(bytes)
    }

    pub(crate) fn decode(mut bytes: &[u8]) -> Result<Self, PackageError> {
        if MAX_PLAINTEXT < bytes.len() {
            return Err(PackageError::TooLarge);
        }
        let archive = GuardianArchive {
            private_encrypt: take_string(&mut bytes)?,
            private_salt: take_string(&mut bytes)?,
            local_json: take_string(&mut bytes)?,
            consensus_json: take_string(&mut bytes)?,
        };
        let api_auth = take_string(&mut bytes)?;
        if !bytes.is_empty() {
            return Err(PackageError::Invalid);
        }
        let material = Self { archive, api_auth };
        material.check_size()?;
        Ok(material)
    }
}

/// Seal a stopped, backup-confirmed guardian to the FA's named destination.
///
/// This function deliberately takes only seat material and the service signing
/// key, never the source root mnemonic or wallet keys. The caller owns the stop,
/// authorization, expiry and publication barriers.
pub fn seal(
    material: &TransferMaterial,
    grant: &SeatTransferGrant,
    source_service_key: &Keypair,
) -> Result<SealedSeatPackage, PackageError> {
    if source_service_key.x_only_public_key().0 != grant.source.service_pubkey {
        return Err(PackageError::Binding);
    }
    let plaintext = material.encode()?;
    let (ephemeral_secret, ephemeral_public) = SECP256K1.generate_keypair(&mut rand::thread_rng());
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&ephemeral_public.serialize());
    header.extend_from_slice(&grant_hash(grant)?);
    let key = shared_key(
        &grant.destination_service_pubkey.public_key(Parity::Even),
        &ephemeral_secret,
        &header,
    );
    let ciphertext =
        fedimint_aead::encrypt(plaintext, &key).map_err(|_| PackageError::Authentication)?;
    let mut sealed = header;
    sealed.extend_from_slice(&ciphertext);
    let signature = SECP256K1.sign_schnorr(&signature_hash(&sealed), source_service_key);
    sealed.extend_from_slice(signature.as_ref());
    SealedSeatPackage::new(sealed).map_err(|_| PackageError::TooLarge)
}

/// Authenticate the source and open a package for this exact FA-authorized grant.
///
/// Returns unvalidated native config. Callers must not install it merely because
/// the source possesses the signing key. Renewal requires a fresh seal until
/// acceptance; an already accepted package is persisted and resumed, not reopened
/// under a different grant.
pub fn open(
    package: &SealedSeatPackage,
    grant: &SeatTransferGrant,
    destination_service_key: &Keypair,
) -> Result<TransferMaterial, PackageError> {
    let bytes = package.as_bytes();
    if SEALED_SEAT_PACKAGE_MAX_BYTES < bytes.len() {
        return Err(PackageError::TooLarge);
    }
    if bytes.len() < HEADER_LEN + AEAD_OVERHEAD + SIGNATURE_LEN || &bytes[..8] != MAGIC {
        return Err(PackageError::Invalid);
    }
    if destination_service_key.x_only_public_key().0 != grant.destination_service_pubkey
        || bytes[41..HEADER_LEN] != grant_hash(grant)?
    {
        return Err(PackageError::Binding);
    }
    let signed_end = bytes.len() - SIGNATURE_LEN;
    let signature = secp256k1::schnorr::Signature::from_slice(&bytes[signed_end..])
        .map_err(|_| PackageError::Authentication)?;
    SECP256K1
        .verify_schnorr(
            &signature,
            &signature_hash(&bytes[..signed_end]),
            &grant.source.service_pubkey,
        )
        .map_err(|_| PackageError::Authentication)?;
    let ephemeral = PublicKey::from_slice(&bytes[8..41]).map_err(|_| PackageError::Invalid)?;
    let mut secret = destination_service_key.secret_key();
    if destination_service_key.x_only_public_key().1 == Parity::Odd {
        secret = secret.negate();
    }
    let key = shared_key(&ephemeral, &secret, &bytes[..HEADER_LEN]);
    let mut ciphertext = bytes[HEADER_LEN..signed_end].to_vec();
    let plaintext =
        fedimint_aead::decrypt(&mut ciphertext, &key).map_err(|_| PackageError::Authentication)?;
    TransferMaterial::decode(plaintext)
}

fn shared_key(public: &PublicKey, secret: &SecretKey, header: &[u8]) -> LessSafeKey {
    let shared = SharedSecret::new(public, secret);
    let hkdf = Hkdf::<Sha256>::new(Some(KDF_DOMAIN), &shared.secret_bytes());
    let mut key = [0; 32];
    hkdf.expand(header, &mut key)
        .expect("32 bytes fit HKDF-SHA256 output");
    LessSafeKey::new(
        UnboundKey::new(&ring::aead::CHACHA20_POLY1305, &key)
            .expect("ChaCha20-Poly1305 accepts a 32-byte key"),
    )
}

fn grant_hash(grant: &SeatTransferGrant) -> Result<[u8; 32], PackageError> {
    let canonical = serde_json_canonicalizer::to_vec(grant).map_err(|_| PackageError::Invalid)?;
    Ok(Sha256::digest(canonical).into())
}

fn signature_hash(signed: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(SIGNATURE_DOMAIN);
    hash.update(signed);
    hash.finalize().into()
}

fn take_string(bytes: &mut &[u8]) -> Result<String, PackageError> {
    let prefix = bytes.get(..4).ok_or(PackageError::Invalid)?;
    let len = u32::from_be_bytes(prefix.try_into().expect("four-byte prefix")) as usize;
    *bytes = &bytes[4..];
    // Never reserve using an untrusted declared length. Check against the
    // already bounded remaining plaintext before allocating the field.
    let field = bytes.get(..len).ok_or(PackageError::Invalid)?;
    let field = std::str::from_utf8(field).map_err(|_| PackageError::Invalid)?;
    let value = field.to_owned();
    *bytes = &bytes[len..];
    Ok(value)
}

#[cfg(test)]
mod tests;
