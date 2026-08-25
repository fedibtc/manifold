//! Encrypted storage for daemon-owned secrets.
//!
//! The gatewayd credential, the bitcoind password, and the provider key are
//! sealed with AES-256-GCM under a local deployment key. Admin views and logs
//! carry a redacted indicator, never the value.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, ensure};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Writes one encrypted secret, replacing any record already under that name.
///
/// Returns the prepared statement instead of running it. Its three callers
/// execute against a pool or a transaction and map failure into their own error
/// type; what they share, and all this owns, is the shape of the row.
pub(crate) fn upsert_secret_record<'q>(
    secret_name: &'q str,
    record: &'q EncryptedSecretRecord,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    sqlx::query(
        "INSERT INTO secret_records \
         (name, version, algorithm, key_id, nonce, ciphertext, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, unixepoch(), unixepoch()) \
         ON CONFLICT(name) DO UPDATE SET \
           version = excluded.version, \
           algorithm = excluded.algorithm, \
           key_id = excluded.key_id, \
           nonce = excluded.nonce, \
           ciphertext = excluded.ciphertext, \
           updated_at = unixepoch()",
    )
    .bind(secret_name)
    .bind(record.version)
    .bind(&record.algorithm)
    .bind(&record.key_id)
    .bind(&record.nonce)
    .bind(&record.ciphertext)
}

pub(crate) const SECRET_RECORD_VERSION: i64 = 1;
pub(crate) const SECRET_RECORD_ALGORITHM: &str = "AES-256-GCM";
pub(crate) const SECRET_RECORD_KEY_ID: &str = "local-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EncryptedSecretRecord {
    pub version: i64,
    pub algorithm: String,
    pub key_id: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Local AES-GCM secret store for daemon-owned at-rest secrets.
#[derive(Clone)]
pub(crate) struct SecretStore {
    key: [u8; KEY_LEN],
}

impl fmt::Debug for SecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretStore")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl SecretStore {
    /// Loads an explicit key or reads/generates the local data-dir key file.
    pub(crate) fn load_or_create(
        key_path: impl AsRef<Path>,
        explicit_key: Option<&str>,
    ) -> anyhow::Result<Self> {
        let key = match explicit_key {
            Some(key) => parse_key(key).context("invalid explicit FLIP secret-store key")?,
            None => load_or_create_key_file(key_path.as_ref())?,
        };

        Ok(Self { key })
    }

    /// Creates a store from a hex-encoded 32-byte key.
    ///
    /// Test-only. Production reaches the key through `load_or_create`, which
    /// already covers both an explicit key and the data-dir key file, so this
    /// is a third way in that no deployment takes.
    #[cfg(test)]
    pub(crate) fn from_hex_key(key: &str) -> anyhow::Result<Self> {
        Ok(Self {
            key: parse_key(key)?,
        })
    }

    /// Generates a fresh hex-encoded 32-byte key.
    pub(crate) fn generate_hex_key() -> String {
        let key = Aes256Gcm::generate_key(OsRng);
        hex::encode(key.as_slice())
    }

    /// Encrypts one UTF-8 secret value for a stable secret name.
    pub(crate) fn encrypt(
        &self,
        secret_name: &str,
        plaintext: &str,
    ) -> anyhow::Result<EncryptedSecretRecord> {
        let cipher = self.cipher();
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let aad = associated_data(
            secret_name,
            SECRET_RECORD_VERSION,
            SECRET_RECORD_ALGORITHM,
            SECRET_RECORD_KEY_ID,
        );
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|error| anyhow::anyhow!("failed to encrypt secret {secret_name}: {error}"))?;

        Ok(EncryptedSecretRecord {
            version: SECRET_RECORD_VERSION,
            algorithm: SECRET_RECORD_ALGORITHM.to_owned(),
            key_id: SECRET_RECORD_KEY_ID.to_owned(),
            nonce: nonce.as_slice().to_owned(),
            ciphertext,
        })
    }

    /// Decrypts one stored secret value.
    pub(crate) fn decrypt(
        &self,
        secret_name: &str,
        record: &EncryptedSecretRecord,
    ) -> anyhow::Result<String> {
        ensure!(
            record.version == SECRET_RECORD_VERSION,
            "unsupported secret record version for {secret_name}: {version}",
            version = record.version
        );
        ensure!(
            record.algorithm == SECRET_RECORD_ALGORITHM,
            "unsupported secret algorithm for {secret_name}: {algorithm}",
            algorithm = record.algorithm
        );
        ensure!(
            record.nonce.len() == NONCE_LEN,
            "invalid nonce length for {secret_name}: {len}",
            len = record.nonce.len()
        );

        let cipher = self.cipher();
        let nonce = Nonce::from_slice(&record.nonce);
        let aad = associated_data(
            secret_name,
            record.version,
            &record.algorithm,
            &record.key_id,
        );
        let plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: record.ciphertext.as_slice(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|error| anyhow::anyhow!("failed to decrypt secret {secret_name}: {error}"))?;

        String::from_utf8(plaintext)
            .with_context(|| format!("decrypted secret {secret_name} is not valid UTF-8"))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key))
    }
}

fn load_or_create_key_file(path: &Path) -> anyhow::Result<[u8; KEY_LEN]> {
    match fs::read_to_string(path) {
        Ok(key) => {
            return parse_key(key.trim()).with_context(|| {
                format!("invalid FLIP secret-store key file at {}", path.display())
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read secret-store key {}", path.display()));
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create secret-store key dir {}", parent.display())
        })?;
    }

    let hex_key = SecretStore::generate_hex_key();
    if write_new_key_file(path, &hex_key)? {
        parse_key(&hex_key)
    } else {
        let key = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read concurrently-created secret-store key {}",
                path.display()
            )
        })?;
        parse_key(key.trim()).with_context(|| {
            format!(
                "invalid concurrently-created secret-store key at {}",
                path.display()
            )
        })
    }
}

fn write_new_key_file(path: &Path, hex_key: &str) -> anyhow::Result<bool> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(path) {
        Ok(mut file) => file
            .write_all(hex_key.as_bytes())
            .with_context(|| format!("failed to write secret-store key {}", path.display()))
            .map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("failed to create secret-store key {}", path.display())),
    }
}

fn parse_key(key: &str) -> anyhow::Result<[u8; KEY_LEN]> {
    let decoded = hex::decode(key.trim()).context("secret-store key must be hex encoded")?;
    decoded.try_into().map_err(|decoded: Vec<u8>| {
        anyhow::anyhow!(
            "secret-store key must decode to {KEY_LEN} bytes, got {len}",
            len = decoded.len()
        )
    })
}

fn associated_data(secret_name: &str, version: i64, algorithm: &str, key_id: &str) -> String {
    format!("fedi-flip-secret-store/v1:{secret_name}:{version}:{algorithm}:{key_id}")
}

#[cfg(test)]
#[path = "../tests/secret_store.rs"]
mod tests;
