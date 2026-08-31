//! Portable FI backup envelope.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead as _, KeyInit as _, Payload},
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use hkdf::Hkdf;
use nostr_sdk::{Keys, PublicKey, SecretKey};
use rand::{RngCore as _, rngs::OsRng};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{FiError, FiResult};

const BACKUP_FORMAT_VERSION: u16 = 1;
const ENCRYPTED_BACKUP_VERSION: u16 = 1;
const AUTHOR_KEY_INFO: &[u8] = b"fedi-fi-backup/nostr-author/v1";
const CONTENT_KEY_INFO: &[u8] = b"fedi-fi-backup/content-encryption/v1";
const AEAD_DOMAIN: &[u8] = b"fedi-fi-backup/event-aead/v1\0";
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const FRAME_LEN: usize = 4;
const ZSTD_LEVEL: i32 = 3;
const DECOMPRESSED_MAX_BYTES: usize = 4 * 1024 * 1024;
const ENCRYPTED_ENVELOPE_MAX_BYTES: usize = 90 * 1024;
const PADDING_BUCKETS: [usize; 4] = [8 * 1024, 16 * 1024, 32 * 1024, 64 * 1024];

/// Provisional FI encrypted-backup Nostr event kind.
pub const FI_BACKUP_EVENT_KIND: u16 = 37706;
/// Stable addressable coordinate for FI backup version 1.
pub const FI_BACKUP_D_TAG: &str = "fedi-fi-backup:v1";

/// Opaque, versioned copy of one formed federation's durable FI recovery state.
///
/// This value is sensitive and is not encrypted. Consumers must encrypt it
/// before storing it outside their protected local backup boundary.
pub struct FiBackup {
    bytes: Vec<u8>,
}

/// Dedicated FI backup key family derived internally from the scoped FI root.
pub(crate) struct FiBackupKeys {
    author: Keys,
    content_key: Zeroizing<[u8; 32]>,
}

impl FiBackupKeys {
    pub(crate) fn derive(root_secret: &[u8], environment: ManifoldEnvironment) -> FiResult<Self> {
        let hkdf = Hkdf::<Sha256>::new(Some(environment_salt(environment)), root_secret);
        let mut content_key = Zeroizing::new([0_u8; 32]);
        hkdf.expand(CONTENT_KEY_INFO, content_key.as_mut())
            .expect("HKDF-SHA256 supports a 32-byte backup key");

        for counter in 0..=u8::MAX {
            let mut info = Vec::with_capacity(AUTHOR_KEY_INFO.len() + 1);
            info.extend_from_slice(AUTHOR_KEY_INFO);
            info.push(counter);
            let mut candidate = Zeroizing::new([0_u8; 32]);
            hkdf.expand(&info, candidate.as_mut())
                .expect("HKDF-SHA256 supports a 32-byte backup key");
            if let Ok(secret) = SecretKey::from_slice(candidate.as_ref()) {
                return Ok(Self {
                    author: Keys::new(secret),
                    content_key,
                });
            }
        }
        Err(FiError::Identity(
            "could not derive a valid FI backup author key".to_owned(),
        ))
    }

    pub(crate) fn author_public_key(&self) -> PublicKey {
        self.author.public_key()
    }

    fn content_key(&self) -> &[u8; 32] {
        &self.content_key
    }
}

/// Opaque encrypted form of one portable [`FiBackup`].
pub struct EncryptedFiBackup {
    bytes: Vec<u8>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EncryptedBackupEnvelope {
    version: u16,
    blob: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackupEnvelope {
    version: u16,
    payload_sha256: String,
    payload: String,
}

impl FiBackup {
    /// Parse and integrity-check a portable FI backup.
    pub fn from_bytes(bytes: Vec<u8>) -> FiResult<Self> {
        let (envelope, _) = parse_envelope(&bytes)?;
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| invalid_backup("could not normalize the backup envelope"))?;
        Ok(Self { bytes })
    }

    /// Borrow the canonical backup bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume this backup and return its canonical bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn encode<T: Serialize>(payload: &T) -> FiResult<Self> {
        let payload = serde_json::to_vec(payload)
            .map_err(|_| invalid_backup("could not encode durable FI state"))?;
        let envelope = BackupEnvelope {
            version: BACKUP_FORMAT_VERSION,
            payload_sha256: payload_sha256(&payload),
            payload: URL_SAFE_NO_PAD.encode(payload),
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| invalid_backup("could not encode the backup envelope"))?;
        Ok(Self { bytes })
    }

    pub(crate) fn decode<T: DeserializeOwned>(&self) -> FiResult<T> {
        let (_, payload) = parse_envelope(&self.bytes)?;
        serde_json::from_slice(&payload)
            .map_err(|_| invalid_backup("backup payload is not valid FI recovery state"))
    }
}

impl FiBackup {
    pub(crate) fn encrypt(&self, keys: &FiBackupKeys) -> FiResult<EncryptedFiBackup> {
        let compressed = zstd::bulk::compress(&self.bytes, ZSTD_LEVEL)
            .map_err(|_| encrypted_backup_error("could not compress backup payload"))?;
        let framed_len = FRAME_LEN
            .checked_add(compressed.len())
            .ok_or_else(|| encrypted_backup_error("compressed backup length overflowed"))?;
        let bucket = padding_bucket(framed_len)
            .ok_or_else(|| encrypted_backup_error("compressed backup exceeds the 64 KiB bucket"))?;

        let mut plaintext = vec![0_u8; bucket];
        plaintext[..FRAME_LEN].copy_from_slice(
            &u32::try_from(compressed.len())
                .map_err(|_| encrypted_backup_error("compressed backup length overflowed"))?
                .to_be_bytes(),
        );
        plaintext[FRAME_LEN..framed_len].copy_from_slice(&compressed);
        OsRng.fill_bytes(&mut plaintext[framed_len..]);

        EncryptedFiBackup::seal_plaintext(&plaintext, keys)
    }
}

impl EncryptedFiBackup {
    /// Parse the bounded public encryption envelope.
    pub fn from_bytes(bytes: Vec<u8>) -> FiResult<Self> {
        let envelope = parse_encrypted_envelope(&bytes)?;
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| encrypted_backup_error("could not normalize encryption envelope"))?;
        Ok(Self { bytes })
    }

    /// Borrow the canonical encrypted-envelope bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume this encrypted backup and return its canonical bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn encode(blob: Vec<u8>) -> FiResult<Self> {
        let envelope = EncryptedBackupEnvelope {
            version: ENCRYPTED_BACKUP_VERSION,
            blob: URL_SAFE_NO_PAD.encode(blob),
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| encrypted_backup_error("could not encode encryption envelope"))?;
        Ok(Self { bytes })
    }

    fn seal_plaintext(plaintext: &[u8], keys: &FiBackupKeys) -> FiResult<Self> {
        if !PADDING_BUCKETS.contains(&plaintext.len()) {
            return Err(encrypted_backup_error(
                "plaintext has no recognized bucket size",
            ));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(keys.content_key())
            .expect("XChaCha20-Poly1305 accepts a 32-byte key");
        let mut nonce = [0_u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &associated_data(keys.author_public_key()),
                },
            )
            .map_err(|_| encrypted_backup_error("could not encrypt backup payload"))?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Self::encode(blob)
    }

    pub(crate) fn decrypt(&self, keys: &FiBackupKeys) -> FiResult<FiBackup> {
        let envelope = parse_encrypted_envelope(&self.bytes)?;
        let blob = URL_SAFE_NO_PAD
            .decode(envelope.blob)
            .map_err(|_| encrypted_backup_error("ciphertext is not valid base64url"))?;
        let bucket = PADDING_BUCKETS
            .iter()
            .copied()
            .find(|bucket| blob.len() == NONCE_LEN + TAG_LEN + bucket)
            .ok_or_else(|| encrypted_backup_error("ciphertext has no recognized bucket size"))?;
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new_from_slice(keys.content_key())
            .expect("XChaCha20-Poly1305 accepts a 32-byte key");
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &associated_data(keys.author_public_key()),
                },
            )
            .map_err(|_| encrypted_backup_error("backup payload authentication failed"))?;
        if plaintext.len() != bucket {
            return Err(encrypted_backup_error(
                "decrypted backup has the wrong bucket size",
            ));
        }
        let (len, rest) = plaintext
            .split_at_checked(FRAME_LEN)
            .ok_or_else(|| encrypted_backup_error("backup length frame is missing"))?;
        let compressed_len =
            u32::from_be_bytes(len.try_into().expect("frame length is four bytes")) as usize;
        let (compressed, _) = rest
            .split_at_checked(compressed_len)
            .ok_or_else(|| encrypted_backup_error("backup length frame is invalid"))?;
        if padding_bucket(FRAME_LEN + compressed_len) != Some(bucket) {
            return Err(encrypted_backup_error(
                "backup does not use its smallest padding bucket",
            ));
        }
        let bytes = zstd::bulk::decompress(compressed, DECOMPRESSED_MAX_BYTES)
            .map_err(|_| encrypted_backup_error("backup payload decompression failed"))?;
        FiBackup::from_bytes(bytes)
    }
}

fn parse_encrypted_envelope(bytes: &[u8]) -> FiResult<EncryptedBackupEnvelope> {
    if bytes.len() > ENCRYPTED_ENVELOPE_MAX_BYTES {
        return Err(encrypted_backup_error("encryption envelope is too large"));
    }
    let envelope: EncryptedBackupEnvelope = serde_json::from_slice(bytes)
        .map_err(|_| encrypted_backup_error("encryption envelope is not valid JSON"))?;
    if envelope.version != ENCRYPTED_BACKUP_VERSION {
        return Err(encrypted_backup_error(
            "encryption envelope version is unsupported",
        ));
    }
    Ok(envelope)
}

fn padding_bucket(framed_len: usize) -> Option<usize> {
    PADDING_BUCKETS
        .iter()
        .copied()
        .find(|bucket| framed_len <= *bucket)
}

fn associated_data(author: PublicKey) -> Vec<u8> {
    let author = author.to_string();
    let mut aad = Vec::with_capacity(AEAD_DOMAIN.len() + author.len() + FI_BACKUP_D_TAG.len() + 4);
    aad.extend_from_slice(AEAD_DOMAIN);
    aad.extend_from_slice(author.as_bytes());
    aad.extend_from_slice(&FI_BACKUP_EVENT_KIND.to_be_bytes());
    aad.extend_from_slice(FI_BACKUP_D_TAG.as_bytes());
    aad.extend_from_slice(&ENCRYPTED_BACKUP_VERSION.to_be_bytes());
    aad
}

fn environment_salt(environment: ManifoldEnvironment) -> &'static [u8] {
    match environment {
        ManifoldEnvironment::Development => b"fedi-fi-backup/environment/development/v1",
        ManifoldEnvironment::Staging => b"fedi-fi-backup/environment/staging/v1",
        ManifoldEnvironment::Production => b"fedi-fi-backup/environment/production/v1",
    }
}

fn encrypted_backup_error(message: &str) -> FiError {
    FiError::Storage(format!("invalid encrypted FI backup: {message}"))
}

fn parse_envelope(bytes: &[u8]) -> FiResult<(BackupEnvelope, Vec<u8>)> {
    let envelope: BackupEnvelope = serde_json::from_slice(bytes)
        .map_err(|_| invalid_backup("backup envelope is not valid JSON"))?;
    if envelope.version != BACKUP_FORMAT_VERSION {
        return Err(invalid_backup("backup format version is unsupported"));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(&envelope.payload)
        .map_err(|_| invalid_backup("backup payload is not valid base64url"))?;
    if envelope.payload_sha256 != payload_sha256(&payload) {
        return Err(invalid_backup("backup payload checksum does not match"));
    }
    Ok((envelope, payload))
}

fn payload_sha256(payload: &[u8]) -> String {
    hex::encode(Sha256::digest(payload))
}

fn invalid_backup(message: &str) -> FiError {
    FiError::Storage(format!("invalid FI backup: {message}"))
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng as _, rngs::StdRng};

    use super::*;

    fn portable_backup() -> FiBackup {
        FiBackup::encode(&serde_json::json!({
            "formation": "formed",
            "seats": ["a", "b"],
        }))
        .expect("encode portable backup")
    }

    #[test]
    fn backup_keys_are_stable_and_domain_separated() {
        let root = [7_u8; 32];
        let first = FiBackupKeys::derive(&root, ManifoldEnvironment::Development).unwrap();
        let second = FiBackupKeys::derive(&root, ManifoldEnvironment::Development).unwrap();
        let staging = FiBackupKeys::derive(&root, ManifoldEnvironment::Staging).unwrap();
        let protocol = Keys::new(SecretKey::from_slice(&root).unwrap());

        assert_eq!(first.author_public_key(), second.author_public_key());
        assert_eq!(first.content_key(), second.content_key());
        assert_ne!(first.author_public_key(), staging.author_public_key());
        assert_ne!(first.content_key(), staging.content_key());
        assert_ne!(first.author_public_key(), protocol.public_key());
    }

    #[test]
    fn encrypted_backup_round_trip_uses_a_fresh_nonce() {
        let keys = FiBackupKeys::derive(&[7; 32], ManifoldEnvironment::Development).unwrap();
        let backup = portable_backup();
        let first = backup.encrypt(&keys).unwrap();
        let second = backup.encrypt(&keys).unwrap();

        assert_ne!(first.as_bytes(), second.as_bytes());
        let restored = first.decrypt(&keys).unwrap();
        assert_eq!(restored.as_bytes(), backup.as_bytes());
    }

    #[test]
    fn encrypted_backup_rejects_wrong_keys_tampering_and_bad_bounds() {
        let keys = FiBackupKeys::derive(&[7; 32], ManifoldEnvironment::Development).unwrap();
        let other = FiBackupKeys::derive(&[8; 32], ManifoldEnvironment::Development).unwrap();
        let encrypted = portable_backup().encrypt(&keys).unwrap();
        assert!(encrypted.decrypt(&other).is_err());
        let staging = FiBackupKeys::derive(&[7; 32], ManifoldEnvironment::Staging).unwrap();
        assert!(encrypted.decrypt(&staging).is_err());

        let mut envelope: EncryptedBackupEnvelope =
            serde_json::from_slice(encrypted.as_bytes()).unwrap();
        let mut blob = URL_SAFE_NO_PAD.decode(&envelope.blob).unwrap();
        blob[NONCE_LEN] ^= 1;
        envelope.blob = URL_SAFE_NO_PAD.encode(blob);
        let tampered =
            EncryptedFiBackup::from_bytes(serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(tampered.decrypt(&keys).is_err());

        envelope.version = ENCRYPTED_BACKUP_VERSION + 1;
        assert!(EncryptedFiBackup::from_bytes(serde_json::to_vec(&envelope).unwrap()).is_err());
        envelope.version = ENCRYPTED_BACKUP_VERSION;
        envelope.blob = "!".to_owned();
        let malformed =
            EncryptedFiBackup::from_bytes(serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(malformed.decrypt(&keys).is_err());

        let mut bad_length = vec![0_u8; PADDING_BUCKETS[0]];
        bad_length[..FRAME_LEN].copy_from_slice(&u32::MAX.to_be_bytes());
        let malformed_frame = EncryptedFiBackup::seal_plaintext(&bad_length, &keys).unwrap();
        assert!(malformed_frame.decrypt(&keys).is_err());

        let mut nonminimal = vec![0_u8; PADDING_BUCKETS[1]];
        nonminimal[..FRAME_LEN].copy_from_slice(&0_u32.to_be_bytes());
        let nonminimal_frame = EncryptedFiBackup::seal_plaintext(&nonminimal, &keys).unwrap();
        assert!(nonminimal_frame.decrypt(&keys).is_err());

        let mut random = vec![0_u8; PADDING_BUCKETS[3] + 1];
        StdRng::seed_from_u64(42).fill_bytes(&mut random);
        assert!(FiBackup { bytes: random }.encrypt(&keys).is_err());

        let bomb = FiBackup {
            bytes: vec![0_u8; DECOMPRESSED_MAX_BYTES + 1],
        }
        .encrypt(&keys)
        .unwrap();
        assert!(bomb.decrypt(&keys).is_err());
    }
}
