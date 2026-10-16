//! `fedimintd`'s legacy encrypted config layout, and the way off it.
//!
//! Before the plaintext layout, a formed seat's private config sat on disk
//! as `private.encrypt`: ChaCha20-Poly1305 under an Argon2 key stretched
//! from `private.salt` and the seat's `api_auth`, with that `api_auth`
//! written beside it as `password.private`. Releases before this one wrote
//! that layout, and backup format version 1 carried it. Both are read here
//! and nowhere else: [`migrate_seat_data_dir`] converts a seat directory at
//! startup and [`LegacyGuardianArchive::decrypt`] converts a restored
//! archive, each under the `api_auth` the fleet derives from its mnemonic.

use serde::{Deserialize, Serialize};

use crate::backup::{GuardianArchive, PRIVATE_CONFIG_FILE, read_optional};

/// The archive as backup format version 1 carried it: the private config
/// still in `fedimintd`'s legacy encrypted layout, ChaCha20-Poly1305 under an
/// Argon2 key stretched from the seat's `api_auth` and `private.salt`.
///
/// Read only. Published archives at this version stay readable because the
/// relay keeps serving them until the fleet republishes under the current
/// version; a restore meanwhile [decrypts](Self::decrypt) one with the
/// `api_auth` it derives from the mnemonic, the same credential the original
/// `fedimintd` encrypted it under.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyGuardianArchive {
    /// `private.encrypt`, hex as written on disk.
    pub private_encrypt: String,
    /// `private.salt`, as written on disk.
    pub private_salt: String,
    pub local_json: String,
    pub consensus_json: String,
}

impl LegacyGuardianArchive {
    /// Decrypt the private config into the current archive shape, under the
    /// seat's `api_auth`.
    pub fn decrypt(&self, api_auth: &str) -> Result<GuardianArchive, LegacyArchiveError> {
        Ok(GuardianArchive {
            private_json: decrypt_private_config(
                &self.private_encrypt,
                &self.private_salt,
                api_auth,
            )?,
            local_json: self.local_json.clone(),
            consensus_json: self.consensus_json.clone(),
        })
    }
}

/// Why a legacy encrypted private config did not decrypt. Not an `io::Error`:
/// the bytes were read fine, they are not what the credential opens.
#[derive(Debug, thiserror::Error)]
#[error("legacy encrypted private config does not open under this seat's api_auth")]
pub struct LegacyArchiveError(#[source] anyhow::Error);

/// `fedimintd`'s legacy `private.encrypt` reader, byte for byte: hex, then
/// ChaCha20-Poly1305 under `get_encryption_key(trim(api_auth), salt)`. The
/// salt is used untrimmed, as `fedimintd` reads it.
fn decrypt_private_config(
    private_encrypt: &str,
    private_salt: &str,
    api_auth: &str,
) -> Result<String, LegacyArchiveError> {
    let key = fedimint_aead::get_encryption_key(api_auth.trim(), private_salt)
        .map_err(LegacyArchiveError)?;
    let mut bytes =
        hex::decode(private_encrypt.trim()).map_err(|err| LegacyArchiveError(err.into()))?;
    let plaintext = fedimint_aead::decrypt(&mut bytes, &key).map_err(LegacyArchiveError)?;
    String::from_utf8(plaintext.to_vec()).map_err(|err| LegacyArchiveError(err.into()))
}

/// The legacy files `fedimintd` falls back to when `private.json` is absent.
const LEGACY_PRIVATE_CONFIG_FILE: &str = "private.encrypt";
const LEGACY_PRIVATE_SALT_FILE: &str = "private.salt";
const LEGACY_PASSWORD_FILE: &str = "password.private";
const LEGACY_FILES: [&str; 3] = [
    LEGACY_PRIVATE_CONFIG_FILE,
    LEGACY_PRIVATE_SALT_FILE,
    LEGACY_PASSWORD_FILE,
];
/// `private.encrypt` becomes `private.encrypt.old`, and so on: out of
/// `fedimintd`'s way, still on disk for an operator.
const LEGACY_SET_ASIDE_EXT: &str = "old";

/// Move a seat's data directory from `fedimintd`'s legacy encrypted layout to
/// the plaintext one: decrypt `private.encrypt` under the seat's `api_auth`
/// into `private.json`, then set `private.encrypt`, `private.salt` and
/// `password.private` aside as `*.old`: `fedimintd` no longer sees them, and
/// an operator still can if anything about the conversion turns out wrong.
/// A directory already in the plaintext layout only has leftover legacy
/// files set aside; one with neither private config (the ceremony has not
/// finished, or the directory is gone) is not touched.
///
/// Safe to interrupt at any point: `private.json` is written whole (staged
/// and renamed into place), and `fedimintd` prefers it whenever it exists, so
/// the child reads the same config before, during and after the migration.
/// The archive the backup publishes is read from the plaintext layout only,
/// so a seat is never backed up mid-migration.
pub(crate) async fn migrate_seat_data_dir(
    seat_data_dir: &std::path::Path,
    api_auth: &str,
) -> Result<LayoutMigration, LayoutMigrationError> {
    let private_json = seat_data_dir.join(PRIVATE_CONFIG_FILE);
    let outcome = if tokio::fs::try_exists(&private_json).await? {
        LayoutMigration::AlreadyPlaintext
    } else {
        let Some(private_encrypt) =
            read_optional(seat_data_dir.join(LEGACY_PRIVATE_CONFIG_FILE)).await?
        else {
            return Ok(LayoutMigration::NothingToDo);
        };
        let private_salt = tokio::fs::read_to_string(seat_data_dir.join(LEGACY_PRIVATE_SALT_FILE))
            .await
            .map_err(|err| anyhow::anyhow!("read {LEGACY_PRIVATE_SALT_FILE}: {err}"))?;
        let plaintext = decrypt_private_config(&private_encrypt, &private_salt, api_auth)?;
        let staged = seat_data_dir.join(format!("{PRIVATE_CONFIG_FILE}.tmp"));
        tokio::fs::write(&staged, plaintext).await?;
        tokio::fs::File::open(&staged).await?.sync_all().await?;
        tokio::fs::rename(&staged, &private_json).await?;
        LayoutMigration::Converted
    };
    // `private.json` is durable before any legacy file moves, so a power cut
    // leaves either both layouts or the new one, never neither. Outside the
    // branch: a run resumed after a crash between the rename and this point
    // finds `private.json` in place but not yet durable.
    tokio::fs::File::open(seat_data_dir)
        .await?
        .sync_all()
        .await?;
    for name in LEGACY_FILES {
        let legacy = seat_data_dir.join(name);
        let set_aside = seat_data_dir.join(format!("{name}.{LEGACY_SET_ASIDE_EXT}"));
        match tokio::fs::rename(&legacy, set_aside).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    tokio::fs::File::open(seat_data_dir)
        .await?
        .sync_all()
        .await?;
    Ok(outcome)
}

/// What [`migrate_seat_data_dir`] found and did. Reported per seat at
/// startup so a fleet's conversion can be followed from its logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutMigration {
    /// `private.encrypt` was decrypted into `private.json` and the legacy
    /// files were set aside.
    Converted,
    /// `private.json` was already there; any leftover legacy files were set
    /// aside.
    AlreadyPlaintext,
    /// Neither private config exists: the ceremony has not finished, or the
    /// directory is gone.
    NothingToDo,
}

impl LayoutMigration {
    /// A fixed label for tracing fields.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Converted => "converted",
            Self::AlreadyPlaintext => "already_plaintext",
            Self::NothingToDo => "nothing_to_do",
        }
    }
}

/// Why [`migrate_seat_data_dir`] refused. The variant is a bounded category
/// for tracing; the message underneath may name files and dependency text
/// and stays in the startup error.
#[derive(Debug, thiserror::Error)]
pub enum LayoutMigrationError {
    #[error(transparent)]
    Decrypt(#[from] LegacyArchiveError),
    #[error(transparent)]
    Io(#[from] anyhow::Error),
}

impl From<std::io::Error> for LayoutMigrationError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.into())
    }
}

impl LayoutMigrationError {
    /// A fixed label for tracing fields.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Decrypt(_) => "decrypt",
            Self::Io(_) => "io",
        }
    }
}

#[cfg(test)]
#[path = "../tests/legacy_layout.rs"]
mod tests;
