//! Portable FI backup envelope.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{FiError, FiResult};

const BACKUP_FORMAT_VERSION: u16 = 1;

/// Opaque, versioned copy of one formed federation's durable FI recovery state.
///
/// This value is sensitive and is not encrypted. Consumers must encrypt it
/// before storing it outside their protected local backup boundary.
pub struct FiBackup {
    bytes: Vec<u8>,
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
