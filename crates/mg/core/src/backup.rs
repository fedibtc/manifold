//! What a fleet backs up and what a restore gets back
//! ([SPEC-nostr-backup-restore](../../specs/SPEC-nostr-backup-restore.md)).
//!
//! This module owns the *data*: which of a seat's state is irreplaceable
//! ([`SeatBackupDocument`]), how the guardian archive is read from and written
//! back to a seat's data directory, and the one assembler that decides what a
//! seat's publication contains ([`seat_publication_plan`]). How that data is
//! laid out on a relay — events, coordinates, sealing, slicing, padding,
//! the schema version — belongs to the storage adapter behind [`BackupSink`]
//! and [`BackupArchive`] (`mg-nostr`'s `format` module). When a seat is
//! published (`backup_worker`) and how a fleet is rebuilt from what was
//! published (`restore`) sit above both.

use std::collections::HashMap;

use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationSize, InviteCode, Plan, SeatId,
};
use serde::{Deserialize, Serialize};

use crate::facts::{SeatFacts, SeatNo};
use crate::identity::RootMnemonic;

/// One seat's irreplaceable state.
///
/// Two independent things are irreplaceable here, and they become available at
/// different times, so the document is published more than once per seat:
///
/// - **The payment record**, from the moment the seat is created. Its typed
///   claim evidence cannot be fetched again — obtaining it means submitting
///   an issuance backed by inputs the FA has already spent — so losing it
///   loses the FA's money.
/// - **The guardian archive**, only once DKG has written one. It does not live
///   *here*: this document changes over a seat's life and is republished each
///   time, while `fedimintd`'s config files are written once at the end of the
///   ceremony and never again. Welding an immutable payload into a mutable
///   document means re-publishing key shares every time an invite or a
///   decommission is recorded, so the archive travels beside the document
///   ([`SeatPublication`]) and this document names it.
///
/// Native keys derive from the mnemonic; imported guardian authentication does
/// not and travels in the sealed transfer recovery facts. Consensus history
/// still comes from peers.
///
/// **No ceremony state is carried, deliberately.** Before consensus a seat owns
/// nothing irreplaceable on the `fedimintd` side: no key shares exist, and a
/// restored unformed seat simply runs the ceremony,
/// which is a correct outcome rather than a loss. The only thing worth
/// protecting in that window is the money, which is why the payment is backed
/// up from creation and the ceremony is not backed up at all.
///
/// After formation the immutable formed-seat fact must be re-established, and
/// [`Self::guardian`] already answers exactly that question: `fedimintd` writes
/// its config when the ceremony completes and at no other time, so
/// `guardian.is_some()` is the native-seat predicate. A removed transfer source
/// instead retains its formed invite in the archive-less tombstone. Carrying
/// an observation timestamp beside it would be a second source of truth for one
/// fact.
///
/// **No seat number is carried.** A seat number names a data directory and a
/// port block on one host, so the restoring host assigns its own. Documents
/// from older builds carry a `seat_no` key, which readers ignore.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeatBackupDocument {
    /// The seat's public identity, which *is* its quote identity
    /// ([`SeatId::quote_id`]).
    pub seat_id: SeatId,
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,
    pub plan: Plan,
    pub federation_size: FederationSize,
    pub created_at_ms: i64,
    /// The accepted payment's recovery material. `None` only for a free seat,
    /// which has no money to lose.
    pub payment: Option<PaymentBackup>,
    /// Where this seat's guardian archive is and what it must hash to. `None`
    /// until DKG has written one — a paid seat is backed up long before it
    /// reaches that point.
    pub guardian: Option<GuardianArchiveRef>,
    /// Set for a seat the operator retired. Carried rather than omitted so a
    /// restore does not resurrect a guardian into a federation the operator
    /// deliberately left; relay-side event deletion is honoured at the relay's
    /// discretion and cannot be relied on for this.
    pub decommissioned_at_ms: Option<i64>,
    /// Transfer recovery facts; absent on legacy/native seats. Omission preserves
    /// their original document digest. Never contains a live signed grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<TransferBackup>,
}

/// Transfer facts are independent of the guardian archive: a removed source
/// retains its formed identity and permanent fence without retaining key shares.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TransferBackup {
    pub incoming: Option<IncomingTransferBackup>,
    pub outgoing: Option<OutgoingTransferBackup>,
}

/// Incoming provenance survives renewal and whole-fleet recovery. Accepted auth
/// is confidential and is sealed by the destination's normal backup encryption.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum IncomingTransferBackup {
    /// No accepted material: restore as expired, never as an ordinary spare.
    Awaiting {
        binding: crate::transfer_state::TransferMove,
        callback_commitment: Option<[u8; 32]>,
    },
    /// Restore through the import validation/installation path, not straight into
    /// a running child. The archive remains beside the document.
    Accepted {
        binding: crate::transfer_state::TransferMove,
        api_auth: String,
        callback_commitment: Option<[u8; 32]>,
    },
}

impl std::fmt::Debug for IncomingTransferBackup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IncomingTransferBackup(redacted)")
    }
}

impl IncomingTransferBackup {
    pub(crate) fn callback_commitment(&self) -> Option<[u8; 32]> {
        match self {
            Self::Awaiting {
                callback_commitment,
                ..
            }
            | Self::Accepted {
                callback_commitment,
                ..
            } => *callback_commitment,
        }
    }
}

/// The source's permanent fence. Removing and Removed publish the same
/// archive-less tombstone before any local guardian secrets are deleted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutgoingTransferBackup {
    pub removed: bool,
    pub federation_invite: InviteCode,
}

/// The part of a seat's payment that cannot be reconstructed.
///
/// These are public, non-secret claim inputs. Terminal progress is deliberately
/// absent — a restored wallet reconciles the evidence against its durable
/// operation log rather than trusting a stale observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaymentBackup {
    pub evidence: crate::wallet::EcashClaimEvidence,
}

impl PaymentBackup {
    pub fn from_record(record: &crate::db::PaymentRecord) -> Self {
        Self {
            evidence: record.evidence.clone(),
        }
    }
}

/// What a seat's document says about the guardian archive published beside it.
///
/// Two facts, and no bytes: the archive is immutable, so a mutable document
/// carries a *reference* to it rather than a copy. Presence is also the
/// formed-seat fact the lifecycle needs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GuardianArchiveRef {
    /// Digest of the archive published beside this seat's document. This is
    /// what binds a given archive to *this seat's* document: the storage
    /// adapter checks the reassembled archive against it on restore.
    pub archive_sha256: String,
    /// The federation's invite code: names the federation this seat guards, so
    /// a recovered fleet can be reported to an operator in terms of the
    /// federations it rejoins rather than opaque seat ids. It belongs here
    /// rather than in the archive because it is observed from the child after
    /// the ceremony, while the archive is fixed at the ceremony's end.
    pub federation_invite: Option<InviteCode>,
}

/// The `fedimintd` config files a restored seat needs, carried opaquely.
///
/// Exactly the set `fedimintd`'s own `download_guardian_backup` archives —
/// `local.json`, `consensus.json`, `private.salt`, `private.encrypt` — and
/// deliberately not the rest of the data directory: the consensus database
/// comes back from peers, `client.json` and `invite-code` are derived, and
/// `password.private` is a plaintext copy of an api_auth either derived locally
/// or carried separately in an imported seat's sealed recovery document.
///
/// Read from disk rather than pulled from that endpoint, for two reasons the
/// endpoint cannot give: it needs a live child API, and it re-encrypts the
/// private config under a fresh salt on every call, so no two answers are the
/// same bytes — which would make an immutable archive a document that changes
/// every time it is published.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GuardianArchive {
    /// `private.encrypt`, still encrypted under the original guardian's
    /// `api_auth` (ARCH-manifold-guardian-identity), hex as written on disk.
    pub private_encrypt: String,
    /// `private.salt`, required to decrypt the above.
    pub private_salt: String,
    /// `local.json`.
    pub local_json: String,
    /// `consensus.json`: public, identical on every guardian, and not
    /// obtainable from a running federation without the target guardian's own
    /// `api_auth`. Most of the archive's bytes.
    pub consensus_json: String,
}

impl GuardianArchive {
    /// The bytes the digest is over and the storage adapter seals. CBOR: the
    /// payload is carried and sealed as bytes, so a binary encoding is the
    /// natural fit, and ciborium writes a struct's fields in declaration
    /// order — deterministic, so a republication produces the same bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes).expect("a guardian archive serializes");
        bytes
    }

    pub fn digest(&self) -> String {
        sha256_hex(self.to_bytes())
    }
}

/// `fedimintd` config file names, as written into a seat's data directory.
/// Named here because this module is the only thing that reads them; the
/// daemon otherwise treats the directory as the child's private business
/// (ARCH-manifold-guardian-seat-processes).
const LOCAL_CONFIG_FILE: &str = "local.json";
const CONSENSUS_CONFIG_FILE: &str = "consensus.json";
const PRIVATE_CONFIG_FILE: &str = "private.encrypt";
const PRIVATE_SALT_FILE: &str = "private.salt";
/// `fedimintd` refuses to load an existing config without this file — it holds
/// the `api_auth` that decrypts `private.encrypt`. It is not *in* the archive
/// so restore obtains the effective authentication and writes it separately.
const PLAINTEXT_PASSWORD_FILE: &str = "password.private";

/// Read one seat's guardian archive out of its `fedimintd` data directory.
///
/// Returns `Ok(None)` before the ceremony has written the files, which is the
/// normal state of a seat that has not completed DKG and has nothing
/// irreplaceable to back up yet.
pub(crate) async fn read_guardian_archive(
    seat_data_dir: &std::path::Path,
) -> std::io::Result<Option<GuardianArchive>> {
    let read = |name: &str| {
        let path = seat_data_dir.join(name);
        async move {
            match tokio::fs::read_to_string(&path).await {
                Ok(contents) => Ok(Some(contents)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err),
            }
        }
    };

    let (Some(private_encrypt), Some(private_salt), Some(local_json), Some(consensus_json)) = (
        read(PRIVATE_CONFIG_FILE).await?,
        read(PRIVATE_SALT_FILE).await?,
        read(LOCAL_CONFIG_FILE).await?,
        read(CONSENSUS_CONFIG_FILE).await?,
    ) else {
        // A partially written set is treated as "not ready" rather than
        // backed up incomplete: an incomplete archive restores nothing.
        return Ok(None);
    };

    Ok(Some(GuardianArchive {
        private_encrypt,
        private_salt,
        local_json,
        consensus_json,
    }))
}

/// Write a restored seat's directory: the recovered archive plus the
/// re-derived `password.private`, into a directory this call creates (restore
/// stages it beside the final seat directory and renames it into place, so a
/// seat directory only ever appears complete — and complete includes the
/// password, without which `fedimintd` would treat the restored config as
/// absent and enter a fresh ceremony).
///
/// The archive is checked against the digest the seat's document names *here*
/// rather than by the caller — relay bytes are only safe because of that check,
/// so the check belongs on this side of the boundary where it cannot be
/// skipped. `api_auth` is the effective, possibly imported seat credential
/// (ARCH-manifold-guardian-identity): the exact bytes `fedimintd` itself writes to
/// this file after a ceremony.
pub(crate) async fn write_restored_seat_dir(
    seat_data_dir: &std::path::Path,
    archive: &GuardianArchive,
    expected_digest: &str,
    api_auth: &str,
) -> Result<(), RestoreConfigError> {
    let found = archive.digest();
    if found != expected_digest {
        return Err(RestoreConfigError::ArchiveMismatch {
            expected: expected_digest.to_owned(),
            found,
        });
    }
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(seat_data_dir)
        .await?;
    for (name, contents) in [
        (PRIVATE_CONFIG_FILE, archive.private_encrypt.as_str()),
        (PRIVATE_SALT_FILE, archive.private_salt.as_str()),
        (LOCAL_CONFIG_FILE, archive.local_json.as_str()),
        (CONSENSUS_CONFIG_FILE, archive.consensus_json.as_str()),
        (PLAINTEXT_PASSWORD_FILE, api_auth),
    ] {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(seat_data_dir.join(name))
            .await?;
        file.write_all(contents.as_bytes()).await?;
        file.sync_all().await?;
    }
    sync_directory(seat_data_dir).await?;
    Ok(())
}

/// Make directory entry changes durable before a SQLite fact can authorize a
/// child start. These paths are within the exclusively owned Unix data root.
pub(crate) async fn sync_directory(path: &std::path::Path) -> std::io::Result<()> {
    tokio::fs::File::open(path).await?.sync_all().await
}

/// Whether an existing directory is exactly what an interrupted attempt of
/// this same restore renamed into place: the five files
/// [`write_restored_seat_dir`] writes, nothing else, with the archive hashing
/// to the expected digest and the password being this seat's derived
/// `api_auth`. Reads and never writes — anything but an exact match makes the
/// caller refuse rather than touch the directory, so the shape check is
/// deliberately as narrow as the writer.
pub(crate) async fn is_adoptable_restored_seat_dir(
    seat_data_dir: &std::path::Path,
    expected_digest: &str,
    api_auth: &str,
) -> std::io::Result<bool> {
    let mut entries = tokio::fs::read_dir(seat_data_dir).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            return Ok(false);
        }
        names.push(entry.file_name());
    }
    names.sort_unstable();
    let mut expected = [
        PRIVATE_CONFIG_FILE,
        PRIVATE_SALT_FILE,
        LOCAL_CONFIG_FILE,
        CONSENSUS_CONFIG_FILE,
        PLAINTEXT_PASSWORD_FILE,
    ]
    .map(std::ffi::OsString::from);
    expected.sort_unstable();
    if names != expected {
        return Ok(false);
    }
    let Some(archive) = read_guardian_archive(seat_data_dir).await? else {
        return Ok(false);
    };
    Ok(archive.digest() == expected_digest
        && tokio::fs::read_to_string(seat_data_dir.join(PLAINTEXT_PASSWORD_FILE)).await?
            == api_auth)
}

/// Refusal to restore a guardian config: the restore side rejecting
/// relay-supplied bytes or failing to write them.
#[derive(Debug, thiserror::Error)]
pub enum RestoreConfigError {
    #[error(
        "guardian archive does not match the digest the backup names (expected {expected}, got {found})"
    )]
    ArchiveMismatch { expected: String, found: String },
    #[error("write guardian archive: {0}")]
    Write(#[from] std::io::Error),
}

pub fn sha256_hex(contents: impl AsRef<[u8]>) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(contents.as_ref()))
}

impl SeatBackupDocument {
    pub fn new(
        facts: &SeatFacts,
        payment: Option<PaymentBackup>,
        guardian: Option<GuardianArchiveRef>,
        decommissioned_at_ms: Option<i64>,
    ) -> Self {
        Self {
            seat_id: facts.seat_id.clone(),
            fa_id: facts.fa_id,
            plan: facts.plan.clone(),
            federation_size: facts.federation_size,
            created_at_ms: facts.created_at_ms,
            payment,
            guardian,
            decommissioned_at_ms,
            transfer: None,
        }
    }

    /// Imported authentication is not derived from this host's root.
    pub(crate) fn effective_api_auth(&self, identity: &RootMnemonic) -> String {
        match self
            .transfer
            .as_ref()
            .and_then(|transfer| transfer.incoming.as_ref())
        {
            Some(IncomingTransferBackup::Accepted { api_auth, .. }) => api_auth.clone(),
            _ => identity.derive_seat_keys(&self.seat_id).api_auth,
        }
    }

    /// Validate shape before restore touches files or commits fleet facts.
    pub(crate) fn validate_transfer(&self) -> anyhow::Result<()> {
        let Some(transfer) = &self.transfer else {
            return Ok(());
        };
        anyhow::ensure!(
            transfer.incoming.is_some() || transfer.outgoing.is_some(),
            "empty transfer recovery facts"
        );
        if let Some(outgoing) = &transfer.outgoing {
            anyhow::ensure!(
                outgoing.removed == self.guardian.is_none(),
                "transfer retirement archive mismatch"
            );
            if let Some(guardian) = &self.guardian {
                anyhow::ensure!(
                    guardian.federation_invite.as_ref() == Some(&outgoing.federation_invite),
                    "transfer retirement invite mismatch"
                );
            }
            anyhow::ensure!(
                !outgoing.removed || transfer.incoming.is_none(),
                "removed guardian retains imported secrets"
            );
        }
        if let Some(incoming) = &transfer.incoming {
            let binding = match incoming {
                IncomingTransferBackup::Awaiting { binding, .. } => {
                    anyhow::ensure!(
                        self.guardian.is_none() && transfer.outgoing.is_none(),
                        "unaccepted transfer has guardian facts"
                    );
                    binding
                }
                IncomingTransferBackup::Accepted {
                    binding, api_auth, ..
                } => {
                    anyhow::ensure!(
                        !api_auth.is_empty() && api_auth.len() <= 1024,
                        "invalid imported authentication"
                    );
                    let invite = self
                        .guardian
                        .as_ref()
                        .and_then(|guardian| guardian.federation_invite.as_ref())
                        .ok_or_else(|| {
                            anyhow::anyhow!("accepted transfer lacks guardian identity")
                        })?;
                    let invite: fedimint_core::invite_code::InviteCode = invite
                        .0
                        .parse()
                        .map_err(|_| anyhow::anyhow!("invalid imported federation invite"))?;
                    anyhow::ensure!(
                        invite.federation_id().to_string() == binding.federation_id.0,
                        "imported federation identity mismatch"
                    );
                    binding
                }
            };
            anyhow::ensure!(
                binding.fa_id == self.fa_id && binding.destination_seat_id == self.seat_id,
                "incoming transfer seat ownership mismatch"
            );
        }
        Ok(())
    }

    /// Rebuild the durable creation facts a restored fleet inserts, under the
    /// seat number the restoring host assigned.
    pub fn to_seat_facts(&self, seat_no: SeatNo) -> SeatFacts {
        SeatFacts {
            seat_id: self.seat_id.clone(),
            seat_no,
            fa_id: self.fa_id,
            plan: self.plan.clone(),
            federation_size: self.federation_size,
            created_at_ms: self.created_at_ms,
        }
    }
}

/// One seat's publication: everything the sink must make durable together.
///
/// Assembled only by [`seat_publication_plan`] in production; the fields are
/// data, and how they become events — sealing, slicing, ordering,
/// coordinates — is the sink's.
pub struct SeatPublication {
    pub document: SeatBackupDocument,
    /// The guardian archive to publish beside the document, present only when
    /// it is not already confirmed on the relay (the archive is immutable, so
    /// a confirmed digest means its bytes are already served).
    /// [`SeatBackupDocument::guardian`] names its digest.
    pub archive: Option<GuardianArchive>,
}

impl SeatPublication {
    /// The hash this publication is recorded and reconciled under: SHA-256
    /// of the plaintext document bytes (the sealed event is nonce-randomized,
    /// so its bytes are not a stable identity).
    pub fn doc_sha256(&self) -> String {
        seat_document_sha256(&self.document)
    }

    /// The digest the confirmed record carries after this publishes.
    pub fn archive_digest(&self) -> Option<&str> {
        self.document
            .guardian
            .as_ref()
            .map(|guardian| guardian.archive_sha256.as_str())
    }
}

/// The hash a publication is recorded and reconciled under. One definition,
/// shared by the assembler and the restore seeding: a restored fleet records
/// the hash of the document it fetched, and only equality with what this
/// install would assemble keeps the worker from republishing it. CBOR is
/// deterministic for a struct (ciborium writes fields in declaration order).
pub(crate) fn seat_document_sha256(document: &SeatBackupDocument) -> String {
    let mut bytes = Vec::new();
    ciborium::into_writer(document, &mut bytes).expect("a seat document serializes");
    sha256_hex(bytes)
}

/// Assemble one seat's publication.
///
/// **The single assembler.** Each publication *replaces* the last at the same
/// relay coordinate: a document assembled from whatever a call site happens to
/// hold would silently erase what an earlier publication had already made
/// durable. So no call site builds a document, and none of them decides what
/// else has to go with it — a publication is a whole value, and the caller's
/// only job is to hand it to a sink.
///
/// A seat that has ever run consensus holds key shares that exist nowhere
/// else, so a publication without its archive is not that seat's publication:
/// this refuses to assemble one — leaving the worker to retry, which is what
/// should happen while `fedimintd` is still writing its config out. The
/// predicate is the durable formed invite, which a mnemonic restore also
/// re-establishes, so no restart or forgotten runtime flag can downgrade a
/// relay document to "no archive exists".
///
/// `confirmed_archive_digest` is the digest a previous publication confirmed,
/// if any. It short-circuits both the archive file read and the bytes'
/// republication: immutable, confirmed, done.
///
/// It reads only from the database and the seat's data directory, never from
/// the running child. The invite is written to the immutable formed-seat row
/// when the final configuration is installed, so a rebuild with no child still
/// reproduces it.
pub(crate) async fn seat_publication_plan(
    db: &crate::db::Db,
    process: &crate::seat_process::SeatProcessConfig,
    facts: &SeatFacts,
    confirmed_archive_digest: Option<&str>,
) -> anyhow::Result<SeatPublication> {
    let payment = db
        .payment(&facts.seat_id)
        .await?
        .map(|record| PaymentBackup::from_record(&record));
    let incoming = db.incoming_transfer(&facts.seat_id).await?;
    let outgoing = db.outgoing_transfer(&facts.seat_id).await?;
    let removed = matches!(
        outgoing,
        Some(
            crate::transfer_state::OutgoingTransfer::Removing
                | crate::transfer_state::OutgoingTransfer::Removed
        )
    );
    let formed_invite = db.formed_federation_invite(&facts.seat_id).await?;
    let federation_invite = formed_invite.clone().or_else(|| {
        incoming
            .as_ref()
            .and_then(|incoming| incoming.invite_code().cloned())
    });
    let decommissioned_at_ms = db.decommissioned_at_ms(&facts.seat_id).await?;

    // Accepted bytes, including original auth, are durable before installation.
    // Do not require a final directory to make that acceptance recoverable.
    let material = if removed {
        None
    } else {
        incoming
            .as_ref()
            .and_then(|incoming| incoming.material())
            .map(|(bytes, _)| crate::transfer_package::TransferMaterial::decode(bytes))
            .transpose()?
    };
    let (archive, archive_sha256) = if removed {
        (None, None)
    } else if let Some(material) = &material {
        let digest = material.archive.digest();
        (
            (confirmed_archive_digest != Some(digest.as_str())).then(|| material.archive.clone()),
            Some(digest),
        )
    } else {
        match confirmed_archive_digest {
            Some(digest) => (None, Some(digest.to_owned())),
            // Only an immutable formed row associates the final directory with a
            // completed federation. Until it lands, the document carries no
            // guardian even if event delivery left files in place.
            None if federation_invite.is_none() => (None, None),
            None => {
                let seat_data_dir = crate::seat_process::seat_data_dir(process, facts.seat_no);
                let archive = read_guardian_archive(&seat_data_dir).await?;
                let archive = archive.ok_or_else(|| {
                    anyhow::anyhow!(
                        "consensus has run but this seat has no guardian archive to back up"
                    )
                })?;
                let digest = archive.digest();
                (Some(archive), Some(digest))
            }
        }
    };
    let guardian = archive_sha256.map(|archive_sha256| GuardianArchiveRef {
        archive_sha256,
        federation_invite,
    });
    let mut document = SeatBackupDocument::new(facts, payment, guardian, decommissioned_at_ms);
    if incoming.is_some() || outgoing.is_some() {
        document.transfer = Some(TransferBackup {
            incoming: if removed {
                None
            } else {
                incoming.map(|incoming| match material {
                    Some(material) => IncomingTransferBackup::Accepted {
                        binding: incoming.binding,
                        api_auth: material.api_auth,
                        callback_commitment: incoming.callback_commitment,
                    },
                    None => IncomingTransferBackup::Awaiting {
                        binding: incoming.binding,
                        callback_commitment: incoming.callback_commitment,
                    },
                })
            },
            outgoing: outgoing
                .map(|_| {
                    formed_invite
                        .map(|federation_invite| OutgoingTransferBackup {
                            removed,
                            federation_invite,
                        })
                        .ok_or_else(|| anyhow::anyhow!("retired source lacks formed identity"))
                })
                .transpose()?,
        });
    }
    document.validate_transfer()?;
    Ok(SeatPublication { document, archive })
}

/// What a restore found on the relays, before anything is written.
///
/// Assembled whole and inspected before it touches the disk, so an operator can
/// be shown what is about to be recovered, and so a half-readable backup fails
/// before it has created anything.
pub struct RecoveredFleet {
    pub seats: Vec<SeatBackupDocument>,
    /// Each seat's guardian archive, already reassembled and verified against
    /// the digest it must hash to.
    pub archives: HashMap<SeatId, GuardianArchive>,
    /// The storage format version the fetched documents were read under. The
    /// install records confirmed publications scoped to it, so the worker
    /// republishes nothing while the sink still writes this version and
    /// republishes everything once it does not.
    pub format_version: u32,
}

impl RecoveredFleet {
    /// Seats that came back holding a guardian config. Removed transfer sources
    /// retain formed accounting separately and are not included in this count.
    pub fn formed(&self) -> usize {
        self.seats
            .iter()
            .filter(|seat| seat.guardian.is_some())
            .count()
    }
}

/// Why a [`BackupArchive`] could not hand back the fleet.
#[derive(Debug, thiserror::Error)]
pub enum RecoverError {
    /// A document published by this mnemonic did not decrypt or parse. Fatal
    /// rather than skipped: restoring the rest would silently rebuild part of
    /// a fleet. The string names the offending relay event for the operator.
    #[error("backup document {0} cannot be read by this build")]
    UnreadableDocument(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Where confirmed backup documents go.
///
/// The boundary keeps relay connections — and the whole relay storage format —
/// out of seat logic exactly as the `EcashWallet` trait keeps the mint client
/// out of fleet logic (ARCH-manifold-guardian): the publisher knows only that a
/// publication either became durable or did not, never how it is encoded or
/// how relays are reached. The daemon wires the real implementation in; tests
/// substitute a fake.
#[async_trait::async_trait]
pub trait BackupSink: Send + Sync + 'static {
    /// Publish the whole publication and confirm it reads back. `Ok` is what
    /// the worker records as the seat's confirmed publication, so an
    /// implementation must not report success on a write it has not re-read: a
    /// relay that accepted an event and dropped it would otherwise leave a
    /// seat believed to be backed up forever, and the republish that would
    /// have fixed it is exactly what the record suppresses.
    ///
    /// The archive's bytes must be durable before the document that names
    /// them: a stored document is a promise that its digest verifies fetchable
    /// bytes, so an implementation may not confirm the document while any part
    /// of the archive is unconfirmed.
    async fn publish(&self, publication: &SeatPublication) -> anyhow::Result<()>;

    /// The storage format version this sink writes. Confirmed-publication
    /// records are scoped to it: a record written under another version
    /// describes events this sink's own reader would refuse, so changing the
    /// version republishes every seat.
    fn format_version(&self) -> u32;

    /// Whether read-back confirmations represent recoverable storage.
    /// Discarding adapters must never authorize a destructive transfer step.
    fn supports_transfer_barrier(&self) -> bool {
        false
    }
}

/// A sink that confirms every publication and stores none of them.
///
/// [`crate::fleet::Fleet::open`] wires this for callers — tests, mostly — that
/// open a fleet without standing up a relay: the worker runs its full
/// reconciliation against it, but a restore against what it "published" finds
/// nothing. A real deployment wires the relay-backed sink instead
/// (`mg-nostr`), and the daemon always does. Version 0 is no real format's
/// version, so a record this sink's confirmations wrote never convinces a
/// later real sink that anything is on a relay.
pub struct DiscardBackupSink;

#[async_trait::async_trait]
impl BackupSink for DiscardBackupSink {
    async fn publish(&self, _publication: &SeatPublication) -> anyhow::Result<()> {
        Ok(())
    }

    fn format_version(&self) -> u32 {
        0
    }
}

/// Where a mnemonic's already-published documents are read back from.
///
/// The restore side of [`BackupSink`], and separate from it because it is
/// used at a different time by a different caller: before any fleet exists,
/// by an operator who has just typed a phrase. The identity is passed per call
/// rather than held, because it is not known until that moment.
///
/// **Complete or nothing.** A restore happens once, so an implementation
/// whose enumeration ended early — timeout, dropped connection, a candidate
/// cap — must return an error rather than a shorter fleet: a prefix here is a
/// guardian silently not restored, with no later attempt to correct it.
#[async_trait::async_trait]
pub trait BackupArchive: Send + Sync + 'static {
    /// Fetch and decode everything this mnemonic ever published.
    async fn recover(&self, identity: &RootMnemonic) -> Result<RecoveredFleet, RecoverError>;
}

#[cfg(test)]
#[path = "../tests/backup.rs"]
mod tests;
