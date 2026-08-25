//! Mnemonic-only recovery of a whole fleet
//! ([SPEC-nostr-backup-restore](../../specs/SPEC-nostr-backup-restore.md),
//! *Restore*).
//!
//! This is one of the two identity choices in onboarding
//! ([`crate::onboarding`]); the other is generating a fresh mnemonic. It is not
//! a repair tool. Once the identity choice is made, this host can never be
//! restored into again.
//!
//! That is what makes this module simple: it runs against a database with no
//! identity and no seats, so it inserts rather than reconciles, and creates
//! seat directories rather than writing into existing ones.
//!
//! Two constraints follow from the same fact and are enforced here rather than
//! documented:
//!
//! - **It never writes into an existing seat directory.** It creates, adopts
//!   (a read-only shape-and-digest check against what an interrupted attempt
//!   of this same restore left), or refuses, which keeps the destructive-site
//!   module outside the deletion paths relevant to
//!   [`CLAIM-fleet-manager-preserves-published-guardian-data`](../../specs/CLAIM-fleet-manager-preserves-published-guardian-data.md)
//!   and makes a misdirected restore incapable of destroying a live guardian.
//! - **It refuses an install that already has an identity.** Nothing mints a
//!   mnemonic implicitly, so an identity row means an operator onboarded this
//!   host, and burying that is never what they meant.
//!
//! The caller owns the third constraint — an explicit acknowledgement that the
//! original guardian is permanently offline — because it is an operator
//! decision, not a state this module can observe. Two hosts running one
//! guardian identity equivocate, and mnemonic-only recovery makes standing up a
//! second copy easy.

use fedi_decentralized_service_fleet_manager::SeatId;

use crate::backup::{
    RecoverError, RecoveredFleet, is_adoptable_restored_seat_dir, write_restored_seat_dir,
};
use crate::db::{Db, NewPayment, RestoredGuardianConfig, RestoredSeat};
use crate::identity::RootMnemonic;
use crate::seat_process::{SeatProcessConfig, seat_data_dir, seat_dir};

/// Why a restore refused.
///
/// Every variant is a distinct operator action — retype the phrase, remove a
/// directory, upgrade the build, give up — so the wire carries the variant
/// (`AdminErrorKind`) beside the sentence rather than only the sentence. Two
/// of them are raised by [`crate::onboarding`] rather than by this module: a
/// restore is refused before it reads anything when the phrase is not a
/// mnemonic, or when the operator has not acknowledged that the original host
/// is gone.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RestoreError {
    #[error("this Fleet Manager has already been onboarded; a host is set up once")]
    AlreadyOnboarded,
    // The phrase itself never reaches this message, here or anywhere else
    // (SECURITY.md).
    #[error("that is not a valid mnemonic phrase")]
    InvalidMnemonic,
    #[error("restore requires acknowledging that the original guardians are permanently offline")]
    NotAcknowledged,
    #[error(
        "backup document {0} was published by this mnemonic but cannot be read by this build; \
         restoring the rest would silently rebuild part of a fleet"
    )]
    UnreadableDocument(String),
    #[error(
        "seat {0} would be restored over an existing seat directory whose contents are not this \
         backup's; this host has no identity, so no guardian runs here, but the restore will not \
         touch a directory it cannot prove it created"
    )]
    SeatDirectoryExists(SeatId),
    #[error("seat {seat_id} is formed but its guardian archive is not on the relays")]
    MissingArchive { seat_id: SeatId },
    #[error(transparent)]
    Config(#[from] crate::backup::RestoreConfigError),
    #[error(transparent)]
    Db(#[from] crate::db::DbError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl RestoreError {
    /// The wire discriminant for this refusal.
    ///
    /// The three transparent variants wrap errors from elsewhere, so they carry
    /// no restore-specific action and report as `Other` — a consumer branching
    /// on them would be branching on someone else's failure.
    pub(crate) fn kind(&self) -> crate::admin::AdminErrorKind {
        use crate::admin::AdminErrorKind as Kind;
        match self {
            Self::AlreadyOnboarded => Kind::AlreadyOnboarded,
            Self::InvalidMnemonic => Kind::InvalidMnemonic,
            Self::NotAcknowledged => Kind::RestoreNotAcknowledged,
            Self::UnreadableDocument(_) => Kind::UnreadableBackupDocument,
            Self::SeatDirectoryExists(_) => Kind::SeatDirectoryExists,
            Self::MissingArchive { .. } => Kind::MissingGuardianArchive,
            Self::Config(_) | Self::Db(_) | Self::Other(_) => Kind::Other,
        }
    }
}

/// Read everything this mnemonic published and reassemble the fleet.
///
/// Reads nothing local and writes nothing at all: a failure here leaves the
/// host exactly as it was. The fetching, decoding, and archive reassembly all
/// live behind [`crate::backup::BackupArchive`] — they are the storage
/// format's business, not this module's.
pub(crate) async fn recover(
    identity: &RootMnemonic,
    archive: &dyn crate::backup::BackupArchive,
) -> Result<RecoveredFleet, RestoreError> {
    archive.recover(identity).await.map_err(|err| match err {
        RecoverError::UnreadableDocument(event) => RestoreError::UnreadableDocument(event),
        RecoverError::Other(err) => RestoreError::Other(err),
    })
}

/// Install a recovered fleet onto a fresh host.
///
/// Three steps, in this order: check every destination, move every seat
/// directory into place, then write the seats and the identity as one
/// transaction.
///
/// The identity goes last because it is the record of the decision — "has an
/// identity" is "has been onboarded", and onboarding happens once. An
/// interrupted install must leave the host un-onboarded and retryable without
/// filesystem surgery, rather than coming back as a Fleet Manager that is
/// missing half its guardians and can never ask for the rest.
///
/// Retryability across the filesystem writes comes from two mechanics:
///
/// - **Staging.** Archives are written under `restore-staging/` and renamed
///   into place, so a final seat directory only ever appears complete — a
///   crash mid-write leaves debris only in the staging root, which the next
///   attempt wipes (it is a reserved path only an interrupted restore
///   creates, on a host that has no identity and so no live guardian).
/// - **Adoption.** A final directory that already exists is adopted if and
///   only if it is exactly what a staged write renames into place — the
///   archive files hashing to the digest the seat's document names, the
///   re-derived password, and nothing else — which is exactly what a crash
///   between the renames and the fleet transaction leaves. Anything else
///   refuses: adoption reads, verifies, and never writes, so the
///   never-write-into-an-existing-directory constraint relevant to
///   [`CLAIM-fleet-manager-preserves-published-guardian-data`](../../specs/CLAIM-fleet-manager-preserves-published-guardian-data.md)
///   holds.
pub(crate) async fn install(
    db: &Db,
    process: &SeatProcessConfig,
    identity: &RootMnemonic,
    fleet: &RecoveredFleet,
) -> Result<(), RestoreError> {
    if db.load_identity().await?.is_some() {
        return Err(RestoreError::AlreadyOnboarded);
    }

    let staging_root = process.data_root.join("restore-staging");
    if tokio::fs::try_exists(&staging_root).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(&staging_root)
            .await
            .map_err(|err| anyhow::anyhow!("wipe leftover restore staging: {err}"))?;
    }

    // Check every destination before creating any of them, deciding per seat
    // between adopting an existing complete directory and staging a write.
    let mut to_write = Vec::new();
    for seat in &fleet.seats {
        let seat_no = crate::facts::SeatNo(seat.seat_no);
        let root = seat_dir(process, seat_no);
        if seat_root_has_unexpected_entries(&root).await? {
            return Err(RestoreError::SeatDirectoryExists(seat.seat_id.clone()));
        }
        let dir = seat_data_dir(process, seat_no);
        let exists = tokio::fs::try_exists(&dir).await.unwrap_or(true);
        match &seat.guardian {
            // Restore creates no directory for an unformed seat, so any
            // directory at its path is someone else's.
            None if exists => {
                return Err(RestoreError::SeatDirectoryExists(seat.seat_id.clone()));
            }
            None => {}
            Some(guardian) if exists => {
                let api_auth = identity.derive_seat_keys(&seat.seat_id).api_auth;
                let adoptable =
                    is_adoptable_restored_seat_dir(&dir, &guardian.archive_sha256, &api_auth)
                        .await
                        .map_err(|err| anyhow::anyhow!("inspect existing seat directory: {err}"))?;
                if !adoptable {
                    return Err(RestoreError::SeatDirectoryExists(seat.seat_id.clone()));
                }
            }
            Some(_) => {
                if !fleet.archives.contains_key(&seat.seat_id) {
                    return Err(RestoreError::MissingArchive {
                        seat_id: seat.seat_id.clone(),
                    });
                }
                to_write.push(seat);
            }
        }
    }

    for seat in &to_write {
        let guardian = seat.guardian.as_ref().expect("only formed seats staged");
        let archive = fleet.archives.get(&seat.seat_id).expect("checked above");
        let staged = staging_root.join(seat.seat_no.to_string());
        // `write_restored_seat_dir` re-checks the digest itself: relay bytes
        // are untrusted, and the check belongs on the side of the boundary
        // that cannot skip it. The password is re-derived, not restored — the
        // backup deliberately excludes it.
        let api_auth = identity.derive_seat_keys(&seat.seat_id).api_auth;
        write_restored_seat_dir(&staged, archive, &guardian.archive_sha256, &api_auth).await?;
        let dir = seat_data_dir(process, crate::facts::SeatNo(seat.seat_no));
        if let Some(parent) = dir.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(crate::backup::RestoreConfigError::Write)?;
        }
        // Same filesystem (both under the data root), so the rename is atomic:
        // the final directory appears whole or not at all.
        tokio::fs::rename(&staged, &dir)
            .await
            .map_err(crate::backup::RestoreConfigError::Write)?;
    }
    // The staging root is empty now; leaving it would only make the next
    // wipe-on-entry look like it had something to clean.
    let _ = tokio::fs::remove_dir_all(&staging_root).await;

    let mut restored = Vec::with_capacity(fleet.seats.len());
    for seat in &fleet.seats {
        restored.push(RestoredSeat {
            facts: seat.to_seat_facts(),
            payment: seat.payment.as_ref().map(|payment| NewPayment {
                evidence: payment.evidence.clone(),
            }),
            guardian: seat
                .guardian
                .as_ref()
                .map(|guardian| RestoredGuardianConfig {
                    archive_digest: guardian.archive_sha256.clone(),
                    federation_invite: guardian.federation_invite.clone(),
                }),
            decommissioned_at_ms: seat.decommissioned_at_ms,
            // The relay demonstrably serves exactly this document — we just
            // fetched it — so the restored fleet's first scan republishes
            // nothing.
            published_doc_sha256: crate::backup::seat_document_sha256(seat),
        });
    }
    db.install_restored_fleet(identity, &restored, fleet.format_version)
        .await?;
    Ok(())
}

async fn seat_root_has_unexpected_entries(root: &std::path::Path) -> Result<bool, RestoreError> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(anyhow::anyhow!("inspect restored seat root: {error}").into());
        }
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| anyhow::anyhow!("inspect restored seat root entry: {error}"))?
    {
        if entry.file_name() != "data" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "../tests/restore.rs"]
mod tests;
