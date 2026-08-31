//! Data-directory backup, archive staging and validation, and restore.
//!
//! FLIP's durable state spans SQLite and the target-Fedimint client
//! directories, so a backup captures both at one instant behind a quiescence
//! barrier. A restore stages and validates the archive before it touches the
//! live data directory, keeps the previous state aside, and can roll back to it.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::timeout;

use anyhow::{Context, bail, ensure};
use fedi_decentralized_service_liquidity_manager::{
    BackupArchive, BackupManifest, BackupRecoveryPoint, BackupStateGroup, BackupStore,
    CreateBackupResponse, InspectBackupRequest, InspectBackupResponse, ProtocolVersion, Pubkey,
    RestoreBackupRequest, RestoreBackupResponse, ServiceError, ServiceResult, SetupStatus,
    SetupValidationSummary, Timestamp, ValidationStatus,
};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tar::{Archive, Builder, EntryType, Header};

use crate::config::{DaemonArgs, LOCK_FILE_NAME};
use crate::public::PUBLIC_ENDPOINT_ADDR_TEMP_FILE;
use crate::{DaemonContext, DaemonPaths, Database, SecretStore, setup_store};
use crate::{failed_precondition, internal_error, invalid_argument, now_timestamp, unavailable};

const BACKUP_FORMAT_VERSION: ProtocolVersion = ProtocolVersion(3);

/// How long a backup waits for in-flight worker passes before giving up.
///
/// A pass is bounded by its own budget — `STABILITY_ITEM_BUDGET` is 30 seconds,
/// and the gateway path's is the same order — so a barrier that cannot be taken
/// inside this window means a pass is stuck rather than merely busy, and each of
/// the four periodic workers may have one.
///
/// **The bound exists to protect the workers, not the backup.** Tokio's
/// `RwLock` is write-preferring, so a queued backup stops every *other* pass
/// from starting. Waiting forever would turn one stuck pass into a stalled
/// daemon the moment an operator asked for a backup. Giving up drops the queued
/// write request, and the workers behind it proceed.
const BACKUP_QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(120);
const BACKUP_DIR_NAME: &str = "backups";
const MANIFEST_ENTRY: &str = "backup-manifest.json";

/// Per-file SHA-256 digests of the archived payload, written last so each
/// digest covers exactly the bytes the tar writer consumed.
///
/// This detects corruption, and only corruption: a truncated copy, a bad
/// sector, an interrupted transfer, a partially-written archive. It is **not**
/// integrity protection. The digests travel inside the archive they describe,
/// so anyone who can modify the archive can recompute them; nothing here
/// authenticates the writer. Authenticating an archive needs a signature or a
/// MAC over a key the archive does not carry, which is separate work — see
/// `docs/liquidity-manager/liquidity-manager-open-items.md`.
///
/// It also cannot detect a *self-consistently wrong* archive. The payload is
/// walked while SQLite, its WAL, and the Fedimint client files are live, so a
/// torn read produces a digest that matches the torn bytes. Preventing the torn
/// read is the quiescence barrier's job, not this checksum's.
const CHECKSUM_ENTRY: &str = "backup-checksums.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveEntryKind {
    Directory,
    File,
}

#[derive(Debug)]
struct PayloadEntry {
    source_path: PathBuf,
    relative_path: PathBuf,
    kind: ArchiveEntryKind,
}

/// Captures both durable stores at one instant, then compresses off the barrier.
///
/// FLIP's durable state spans SQLite and the target-Fedimint client
/// directories. Reading two mutable stores without a shared snapshot or a
/// quiescence barrier may observe different instants, so checkpointing the WAL
/// and then walking the live data directory with every worker running would
/// produce an archive holding SQLite from one moment and a client database from
/// another, with nothing in it recording that it might.
///
/// **The barrier covers the copy, not the compression.** Holding worker passes
/// still for the whole `tar` + `gzip` would stall the funding path for as long
/// as the archive takes to write, which on a target-client database is the
/// wrong trade for a maintenance operation. Copying is what has to be
/// consistent; compressing a copy that already exists does not.
///
/// **The cost this accepts, stated rather than hidden:** peak disk during a
/// backup is one extra copy of the payload, in a `0700` staging directory
/// beside the data directory, removed on both the success and the failure path.
/// The staging directory holds the same secret material the archive does, for
/// the same reason.
pub(crate) async fn create_backup(context: &DaemonContext) -> ServiceResult<CreateBackupResponse> {
    create_backup_within(context, BACKUP_QUIESCENCE_TIMEOUT).await
}

/// [`create_backup`] with the quiescence window as a parameter, so a test can
/// reach the refusal without waiting two minutes for it. Production has exactly
/// one caller and it passes [`BACKUP_QUIESCENCE_TIMEOUT`].
async fn create_backup_within(
    context: &DaemonContext,
    quiescence_timeout: Duration,
) -> ServiceResult<CreateBackupResponse> {
    let sqlite_relative = sqlite_relative_path(&context.paths)?;
    let staging = backup_staging_dir_for(&context.paths.data_dir);

    let quiesced_at = {
        // Every periodic worker pass is held still from here to the end of this
        // block. Tokio's `RwLock` is write-preferring, so no further pass
        // starts once this queues, and the guard arrives only when the passes
        // already running have finished. See `WorkQuiescence`.
        let Ok(_barrier) = timeout(quiescence_timeout, context.work_quiescence.quiesce()).await
        else {
            tracing::warn!(
                quiescence_timeout_secs = quiescence_timeout.as_secs(),
                "backup could not quiesce the periodic workers in time; no archive was written"
            );
            return Err(unavailable(
                "a periodic worker pass did not finish within the backup's quiescence \
                 window, so both stores could not be captured at one instant. The daemon \
                 is still serving; retry the backup, and if it keeps failing check worker \
                 health for a pass that is stuck",
            ));
        };
        let quiesced_at = now_timestamp();
        match stage_payload(context, &staging, &sqlite_relative).await {
            Ok(()) => quiesced_at,
            Err(error) => {
                remove_staging(&staging);
                return Err(internal_error(error));
            }
        }
    };

    let manifest = manifest(now_timestamp(), quiesced_at);
    let archived = create_archive(&context.paths, &staging, &manifest);
    remove_staging(&staging);
    let archive_path = archived.map_err(internal_error)?;
    tracing::info!(
        archive = %archive_path.display(),
        quiesced_at = quiesced_at.0,
        "wrote a backup archive"
    );

    Ok(CreateBackupResponse {
        archive: BackupArchive(archive_path.to_string_lossy().into_owned()),
        manifest,
    })
}

/// Copies both stores into `staging`, with the barrier already held.
///
/// The SQLite copy is `VACUUM INTO` rather than a file copy of the live
/// database. It writes a complete, self-contained database containing every
/// committed transaction, so the archive needs no `-wal` or `-shm` beside it and
/// cannot carry a half-checkpointed pair. Those two sidecars are therefore
/// skipped in the payload walk below; every other file is copied as it stands,
/// which is sound because the barrier means nothing is writing them.
async fn stage_payload(
    context: &DaemonContext,
    staging: &Path,
    sqlite_relative: &Path,
) -> anyhow::Result<()> {
    create_private_dir(staging)?;

    let staged_sqlite = staging.join(sqlite_relative);
    if let Some(parent) = staged_sqlite.parent() {
        create_private_dir(parent)?;
    }
    // `VACUUM INTO` refuses an existing file, which is what we want: a name
    // collision here would mean the staging directory was not fresh.
    sqlx::query("VACUUM INTO ?")
        .bind(staged_sqlite.to_string_lossy().as_ref())
        .execute(context.database.pool())
        .await
        .with_context(|| format!("failed to snapshot SQLite into {}", staged_sqlite.display()))?;

    for entry in collect_payload_entries(&context.paths.data_dir)? {
        if is_sqlite_payload_path(&entry.relative_path, sqlite_relative) {
            continue;
        }
        let destination = staging.join(&entry.relative_path);
        match entry.kind {
            ArchiveEntryKind::Directory => create_private_dir(&destination)?,
            ArchiveEntryKind::File => {
                if let Some(parent) = destination.parent() {
                    create_private_dir(parent)?;
                }
                fs::copy(&entry.source_path, &destination).with_context(|| {
                    format!(
                        "failed to stage backup payload {}",
                        entry.relative_path.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// Whether this payload path is the live database or one of its sidecars.
///
/// `VACUUM INTO` has already written a self-contained copy, so all three are
/// skipped. The sidecars are matched by suffix on the database's own file name
/// rather than by extension, so an unrelated file elsewhere that happens to end
/// `-wal` is still archived.
fn is_sqlite_payload_path(relative_path: &Path, sqlite_relative: &Path) -> bool {
    if relative_path == sqlite_relative {
        return true;
    }
    let Some(sqlite_name) = sqlite_relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    relative_path.parent() == sqlite_relative.parent()
        && relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == format!("{sqlite_name}-wal") || name == format!("{sqlite_name}-shm")
            })
}

fn backup_staging_dir_for(data_dir: &Path) -> PathBuf {
    let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let name = data_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("flip");
    parent.join(format!(
        ".{name}.backup-staging-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn create_private_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create backup staging dir {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Removes the staging copy. It carries the same secret material the archive
/// does, so leaving it behind on a failure path is not an option.
fn remove_staging(staging: &Path) {
    if let Err(error) = fs::remove_dir_all(staging)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(
            path = %staging.display(),
            ?error,
            "failed to remove backup staging directory; it holds archive payload and must be removed by hand"
        );
    }
}

pub(crate) fn inspect_backup(
    request: InspectBackupRequest,
) -> ServiceResult<InspectBackupResponse> {
    Ok(InspectBackupResponse {
        manifest: inspect_archive(Path::new(&request.archive.0)).map_err(invalid_argument)?,
    })
}

pub(crate) fn restore_requires_restore_mode() -> ServiceError {
    failed_precondition(
        "restore_backup requires --restore-mode or FLIP_RESTORE_MODE=1, then a normal daemon restart",
    )
}

/// A backup archive extracted and fully validated into a staging directory,
/// ready to be swapped into the data dir.
///
/// Staging is deliberately separated from committing so that every way a
/// restore can be rejected — a corrupt archive, a failed setup validation, a
/// foreign provider identity — is discovered while the daemon is still serving
/// from its current state. Only an archive that has already passed everything
/// reaches the point where live state is moved aside.
pub(crate) struct StagedRestore {
    staging_dir: PathBuf,
    response: RestoreBackupResponse,
    allocations: BTreeMap<String, AllocationIdentity>,
}

impl StagedRestore {
    /// The outcome to report for this restore, computed against the staged
    /// state before it was committed.
    pub(crate) fn response(&self) -> RestoreBackupResponse {
        self.response.clone()
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        staging_dir: PathBuf,
        restored_state_groups: Vec<BackupStateGroup>,
    ) -> Self {
        Self {
            staging_dir,
            response: RestoreBackupResponse {
                status: SetupStatus::NotConfigured,
                validation: SetupValidationSummary {
                    status: ValidationStatus::NotRun,
                    checks: Vec::new(),
                },
                restored_state_groups,
            },
            allocations: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AllocationIdentity {
    requester_pubkey: String,
    provider_pubkey: String,
    network: String,
    details_payload_hash: Vec<u8>,
}

/// Refuses a live restore that would forget or replace a durably accepted
/// allocation from the running generation.
///
/// Takes the generation's allocation-admission write guard rather than asking
/// the caller in prose to hold one. The comparison means nothing unless
/// nothing else can commit an allocation while it runs, and `&mut` is
/// reachable only through the write guard, so the compiler checks that now
/// instead of a reader. It also puts the two gate conditions in one place: a
/// generation already closing for another restore fails here rather than in a
/// separate call the next caller could forget.
///
/// What stays with the caller is the part this function cannot see — holding
/// the guard onward until the restore is either abandoned or the generation is
/// closed and torn down. Restore-only disaster recovery has no running
/// generation and does not use this comparison at all.
pub(crate) async fn ensure_preserves_live_allocations(
    staged: &StagedRestore,
    running: &Database,
    admission: &mut crate::daemon::AllocationAdmission,
) -> ServiceResult<()> {
    admission.ensure_accepts_live_restore()?;

    let running_allocations = load_allocation_identities(running)
        .await
        .map_err(internal_error)?;

    for (federation_id, running_identity) in running_allocations {
        match staged.allocations.get(&federation_id) {
            Some(restored_identity) if restored_identity == &running_identity => {}
            Some(_) => {
                return Err(failed_precondition(format!(
                    "live restore refused: archive replaces the accepted allocation for \
                     federation {federation_id}; use an archive that contains the current \
                     allocation history or keep the running generation"
                )));
            }
            None => {
                return Err(failed_precondition(format!(
                    "live restore refused: archive predates the accepted allocation for \
                     federation {federation_id}; use an archive that contains the current \
                     allocation history or keep the running generation"
                )));
            }
        }
    }

    Ok(())
}

impl Drop for StagedRestore {
    fn drop(&mut self) {
        // Committing consumes the staging dir by moving its contents out, so
        // this only removes the leftovers of a restore that was abandoned.
        let _ = fs::remove_dir_all(&self.staging_dir);
    }
}

/// Extracts and validates an archive into a staging dir without touching the
/// data dir.
///
/// `running_provider` is the provider identity of the daemon requesting the
/// restore, or `None` in restore mode and on a daemon that has never had one.
pub(crate) async fn stage_restore(
    args: &DaemonArgs,
    paths: &DaemonPaths,
    request: RestoreBackupRequest,
    running_provider: Option<&Pubkey>,
) -> ServiceResult<StagedRestore> {
    let sqlite_relative = sqlite_relative_path(paths)?;
    let archive_path = Path::new(&request.archive.0);
    let manifest = inspect_archive(archive_path).map_err(invalid_argument)?;

    let staging_dir = staging_dir_for(&paths.data_dir).map_err(internal_error)?;
    fs::create_dir_all(&staging_dir)
        .with_context(|| {
            format!(
                "failed to create restore staging dir {}",
                staging_dir.display()
            )
        })
        .map_err(internal_error)?;

    let staged = async {
        extract_archive(archive_path, &staging_dir)?;
        let restored = validate_restored_state(args, &staging_dir, &sqlite_relative).await?;
        ensure_restorable_identity(running_provider, restored.provider.as_ref())?;
        let response = RestoreBackupResponse {
            status: restored_status(restored.configured, &restored.validation),
            validation: restored.validation.clone(),
            restored_state_groups: manifest.state_groups,
        };
        Ok::<_, anyhow::Error>((response, restored.allocations))
    }
    .await;

    match staged {
        Ok((response, allocations)) => Ok(StagedRestore {
            staging_dir,
            response,
            allocations,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_dir);
            Err(invalid_argument(error))
        }
    }
}

/// Rejects an archive whose provider identity does not match the running
/// daemon's.
///
/// The public Iroh node id is derived from the provider signing identity, so a
/// live restore that changed it would move the daemon to a different network
/// address than the one its published advertisements name — while peers keep
/// dialing the old one. A daemon that has no identity yet can adopt whatever
/// the archive carries, since it has nothing bound and nothing published.
/// Restoring a foreign identity remains available through restore mode, which
/// starts from an empty data dir and binds afterwards.
fn ensure_restorable_identity(
    running: Option<&Pubkey>,
    restored: Option<&Pubkey>,
) -> anyhow::Result<()> {
    let Some(running) = running else {
        return Ok(());
    };
    match restored {
        Some(restored) if restored.0 == running.0 => Ok(()),
        Some(restored) => bail!(
            "backup carries provider identity {} but this daemon is running as {}; \
             restoring another provider's state would change the daemon's public node id \
             out from under its published advertisements. Use restore mode on an empty \
             data dir instead.",
            restored.0,
            running.0
        ),
        None => bail!(
            "backup carries no provider identity but this daemon is running as {}; \
             restoring it live would leave the public transport unable to rebind. \
             Use restore mode on an empty data dir instead.",
            running.0
        ),
    }
}

/// Serializes restore-mode restores of one data directory.
///
/// [`restore_backup`] checks the data dir is empty, stages an archive, checks
/// again, and then moves the staged contents in. The check and the move are
/// separate steps over a shared directory, and [`move_staged_contents`] renames
/// entries in without a predicate, so neither move fails on the other's output.
/// Two callers that interleave those steps each land a different archive in the
/// same root. Nothing downstream reports it: checksums are verified per archive
/// during staging, so both archives verify, and what remains is two verified
/// archives interleaved. The daemon then starts against a data root that no
/// backup describes.
///
/// The exclusion therefore has to span the whole operation. It cannot live
/// inside the move: a move that refused partway would leave the second caller a
/// data root it can neither use nor clean up.
///
/// One process is the right scope. [`crate::daemon::DaemonLock`] holds an
/// exclusive lock on the data dir for the process lifetime, so a second process
/// cannot reach this directory. Two concurrent calls into one restore-mode
/// process are what remain, and they share this target.
#[derive(Clone, Default)]
pub(crate) struct RestoreTarget {
    busy: Arc<tokio::sync::Mutex<()>>,
}

impl RestoreTarget {
    /// Takes the target for one restore, or reports that one is in flight.
    ///
    /// This rejects rather than queues. A queued caller would wait out the
    /// first extraction, extract its own archive, and then fail the
    /// empty-directory check because the first restore filled the directory —
    /// an expensive route to a worse error message. The live-restore path
    /// rejects a second restore for the same reason.
    pub(crate) fn begin(&self) -> ServiceResult<RestoreTargetGuard> {
        self.busy
            .clone()
            .try_lock_owned()
            .map(|guard| RestoreTargetGuard { _guard: guard })
            .map_err(|_| unavailable("a restore is already in progress"))
    }

    /// Restores an archive into the data dir, holding the target throughout.
    pub(crate) async fn restore(
        &self,
        args: &DaemonArgs,
        paths: &DaemonPaths,
        request: RestoreBackupRequest,
    ) -> ServiceResult<RestoreBackupResponse> {
        let _guard = self.begin()?;
        restore_backup(args, paths, request).await
    }
}

/// Evidence that one caller holds a [`RestoreTarget`]. Releases on drop.
pub(crate) struct RestoreTargetGuard {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Applies a staged archive to an empty data dir.
///
/// Private because the empty-check/move sequence below is only safe under a
/// [`RestoreTarget`]; [`RestoreTarget::restore`] is the way in.
async fn restore_backup(
    args: &DaemonArgs,
    paths: &DaemonPaths,
    request: RestoreBackupRequest,
) -> ServiceResult<RestoreBackupResponse> {
    if args.mode != crate::config::DaemonMode::Restore {
        return Err(restore_requires_restore_mode());
    }

    ensure_restore_target_empty(&paths.data_dir)
        .map_err(|error| failed_precondition(error.to_string()))?;
    let staged = stage_restore(args, paths, request, None).await?;
    tracing::info!(
        data_dir = %paths.data_dir.display(),
        "staged and validated a backup archive; applying it to the data dir"
    );
    ensure_restore_target_empty(&paths.data_dir)
        .map_err(|error| failed_precondition(error.to_string()))?;
    move_staged_contents(&staged.staging_dir, &paths.data_dir).map_err(internal_error)?;
    tracing::info!(
        data_dir = %paths.data_dir.display(),
        "restored a backup into the data dir"
    );

    Ok(staged.response())
}

fn create_archive(
    paths: &DaemonPaths,
    source_root: &Path,
    manifest: &BackupManifest,
) -> anyhow::Result<PathBuf> {
    let backups_dir = paths.data_dir.join(BACKUP_DIR_NAME);
    fs::create_dir_all(&backups_dir)
        .with_context(|| format!("failed to create backup dir {}", backups_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&backups_dir, fs::Permissions::from_mode(0o700));
    }

    let archive_name = format!("flip-backup-{}-{}", manifest.created_at.0, unique_suffix());
    let archive_path = backups_dir.join(format!("{archive_name}.tar.gz"));
    let tmp_path = backups_dir.join(format!("{archive_name}.tar.gz.tmp"));
    let archive_file = create_private_file(&tmp_path)?;
    let result = write_archive(archive_file, source_root, manifest);

    match result {
        Ok(()) => {
            fs::rename(&tmp_path, &archive_path).with_context(|| {
                format!(
                    "failed to move backup archive {} to {}",
                    tmp_path.display(),
                    archive_path.display()
                )
            })?;
            Ok(archive_path)
        }
        Err(error) => {
            let _ = fs::remove_file(&tmp_path);
            Err(error)
        }
    }
}

fn write_archive(
    archive_file: File,
    source_root: &Path,
    manifest: &BackupManifest,
) -> anyhow::Result<()> {
    let encoder = GzEncoder::new(archive_file, Compression::default());
    let mut builder = Builder::new(encoder);
    append_manifest(&mut builder, manifest)?;

    let mut checksums = BTreeMap::new();
    for entry in collect_payload_entries(source_root)? {
        match entry.kind {
            ArchiveEntryKind::Directory => {
                builder
                    .append_dir(&entry.relative_path, &entry.source_path)
                    .with_context(|| {
                        format!(
                            "failed to append backup directory {}",
                            entry.relative_path.display()
                        )
                    })?;
            }
            ArchiveEntryKind::File => {
                let digest = append_digested_file(&mut builder, &entry)?;
                checksums.insert(checksum_key(&entry.relative_path), digest);
            }
        }
    }
    append_checksums(&mut builder, manifest, &checksums)?;

    let encoder = builder
        .into_inner()
        .context("failed to finalize backup tar stream")?;
    encoder
        .finish()
        .context("failed to finalize gzip backup archive")?;
    Ok(())
}

/// Reads exactly what it is asked for and digests it on the way past.
///
/// The digest has to cover the bytes the tar writer actually consumed, not the
/// bytes a separate pass would read afterwards. The payload is walked while the
/// daemon is running, so a second pass over a live SQLite or RocksDB file would
/// routinely disagree with the archived copy and report corruption that is not
/// there.
struct DigestingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for DigestingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.update(&buf[..read]);
        Ok(read)
    }
}

/// Archive key for a payload path.
///
/// Backups are produced and restored on the same host family, and
/// `collect_payload_entries` rejects anything that is not a plain directory or
/// file, so the lossy conversion cannot silently merge two distinct entries
/// here — `extract_archive` keys the verification the same way and would report
/// a mismatch rather than skip a file.
fn checksum_key(relative_path: &Path) -> String {
    relative_path.to_string_lossy().into_owned()
}

fn append_digested_file<W: Write>(
    builder: &mut Builder<W>,
    entry: &PayloadEntry,
) -> anyhow::Result<String> {
    let file = File::open(&entry.source_path).with_context(|| {
        format!(
            "failed to open backup file {}",
            entry.relative_path.display()
        )
    })?;
    let metadata = file.metadata().with_context(|| {
        format!(
            "failed to inspect backup file {}",
            entry.relative_path.display()
        )
    })?;

    let mut header = Header::new_gnu();
    header.set_metadata(&metadata);
    header.set_cksum();

    let mut reader = DigestingReader {
        inner: file,
        hasher: Sha256::new(),
    };
    builder
        .append_data(&mut header, &entry.relative_path, &mut reader)
        .with_context(|| {
            format!(
                "failed to append backup file {}",
                entry.relative_path.display()
            )
        })?;

    Ok(hex::encode(reader.hasher.finalize()))
}

fn append_checksums<W: Write>(
    builder: &mut Builder<W>,
    manifest: &BackupManifest,
    checksums: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(checksums).context("serialize backup checksums")?;
    let mut header = Header::new_gnu();
    header.set_size(json.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(manifest.created_at.0);
    header.set_cksum();
    builder
        .append_data(&mut header, CHECKSUM_ENTRY, json.as_slice())
        .context("append backup checksums")
}

fn digest_file(path: &Path) -> anyhow::Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to reopen {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn append_manifest<W: Write>(
    builder: &mut Builder<W>,
    manifest: &BackupManifest,
) -> anyhow::Result<()> {
    let manifest_json = serde_json::to_vec_pretty(manifest).context("serialize backup manifest")?;
    let mut header = Header::new_gnu();
    header.set_size(manifest_json.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(manifest.created_at.0);
    header.set_cksum();
    builder
        .append_data(&mut header, MANIFEST_ENTRY, manifest_json.as_slice())
        .context("append backup manifest")
}

fn collect_payload_entries(data_dir: &Path) -> anyhow::Result<Vec<PayloadEntry>> {
    let mut entries = Vec::new();
    collect_payload_entries_inner(data_dir, data_dir, &mut entries)?;
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(entries)
}

fn collect_payload_entries_inner(
    root: &Path,
    current: &Path,
    entries: &mut Vec<PayloadEntry>,
) -> anyhow::Result<()> {
    let mut children = fs::read_dir(current)
        .with_context(|| format!("failed to read backup source dir {}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read backup source dir {}", current.display()))?;
    children.sort_by_key(|entry| entry.path());

    for child in children {
        let path = child.path();
        let relative_path = path
            .strip_prefix(root)
            .context("backup source path escaped data dir")?
            .to_path_buf();
        if is_excluded_payload_path(&relative_path) {
            continue;
        }
        ensure!(
            relative_path != Path::new(MANIFEST_ENTRY)
                && relative_path != Path::new(CHECKSUM_ENTRY),
            "{} and {MANIFEST_ENTRY} are reserved for backup archive metadata",
            CHECKSUM_ENTRY
        );

        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect backup source {}", path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            bail!(
                "backup source contains unsupported symlink {}",
                path.display()
            );
        }
        if metadata.is_dir() {
            entries.push(PayloadEntry {
                source_path: path.clone(),
                relative_path,
                kind: ArchiveEntryKind::Directory,
            });
            collect_payload_entries_inner(root, &path, entries)?;
        } else if metadata.is_file() {
            entries.push(PayloadEntry {
                source_path: path,
                relative_path,
                kind: ArchiveEntryKind::File,
            });
        } else {
            bail!("backup source contains unsupported file {}", path.display());
        }
    }

    Ok(())
}

fn inspect_archive(path: &Path) -> anyhow::Result<BackupManifest> {
    let file = File::open(path)
        .with_context(|| format!("failed to open backup archive {}", path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut manifest = None;
    let mut seen_paths = HashSet::new();

    for entry in archive.entries().context("failed to read backup archive")? {
        let mut entry = entry.context("failed to read backup archive entry")?;
        let relative_path = archive_entry_path(&entry)?;
        validate_archive_payload_path(&relative_path)?;
        ensure!(
            seen_paths.insert(relative_path.clone()),
            "backup archive contains duplicate path {}",
            relative_path.display()
        );

        let kind = archive_entry_kind(entry.header().entry_type())?;
        if relative_path == Path::new(MANIFEST_ENTRY) {
            ensure!(
                kind == ArchiveEntryKind::File,
                "backup manifest entry must be a regular file"
            );
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("failed to read backup manifest")?;
            manifest = Some(
                serde_json::from_slice::<BackupManifest>(&bytes)
                    .context("failed to parse backup manifest")?,
            );
        }
    }

    let manifest = manifest.context("backup archive missing manifest")?;
    ensure!(
        manifest.version == BACKUP_FORMAT_VERSION,
        "unsupported backup version {}",
        manifest.version.0
    );
    Ok(manifest)
}

fn extract_archive(archive_path: &Path, staging_dir: &Path) -> anyhow::Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("failed to open backup archive {}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut seen_paths = HashSet::new();
    let mut declared: Option<BTreeMap<String, String>> = None;
    let mut extracted = BTreeMap::new();

    for entry in archive.entries().context("failed to read backup archive")? {
        let mut entry = entry.context("failed to read backup archive entry")?;
        let relative_path = archive_entry_path(&entry)?;
        validate_archive_payload_path(&relative_path)?;
        ensure!(
            seen_paths.insert(relative_path.clone()),
            "backup archive contains duplicate path {}",
            relative_path.display()
        );

        let kind = archive_entry_kind(entry.header().entry_type())?;
        if relative_path == Path::new(MANIFEST_ENTRY) {
            ensure!(
                kind == ArchiveEntryKind::File,
                "backup manifest entry must be a regular file"
            );
            continue;
        }
        if relative_path == Path::new(CHECKSUM_ENTRY) {
            ensure!(
                kind == ArchiveEntryKind::File,
                "backup checksum entry must be a regular file"
            );
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("failed to read backup checksums")?;
            declared =
                Some(serde_json::from_slice(&bytes).context("failed to parse backup checksums")?);
            continue;
        }

        let destination = staging_dir.join(&relative_path);
        match kind {
            ArchiveEntryKind::Directory => fs::create_dir_all(&destination).with_context(|| {
                format!("failed to create restore dir {}", destination.display())
            })?,
            ArchiveEntryKind::File => {
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create restore dir {}", parent.display())
                    })?;
                }
                entry
                    .unpack(&destination)
                    .with_context(|| format!("failed to restore {}", relative_path.display()))?;
                extracted.insert(checksum_key(&relative_path), digest_file(&destination)?);
            }
        }
    }

    let declared = declared
        .context("backup archive missing checksums; it was written by an older format version")?;
    verify_checksums(&declared, &extracted)
}

/// Compares what the archive said it holds with what came out of it.
///
/// Both directions matter. A digest mismatch is a corrupted file; a declared
/// path that never arrived is a truncated archive, which is the more likely
/// accident and the one a per-file digest alone would miss; an extracted file
/// nobody declared means the archive and its checksum list disagree about what
/// the archive is.
fn verify_checksums(
    declared: &BTreeMap<String, String>,
    extracted: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for (path, expected) in declared {
        match extracted.get(path) {
            Some(actual) => ensure!(
                actual == expected,
                "backup archive is corrupt: {path} does not match its recorded checksum"
            ),
            None => bail!("backup archive is incomplete: {path} is recorded but missing"),
        }
    }
    for path in extracted.keys() {
        ensure!(
            declared.contains_key(path),
            "backup archive is inconsistent: {path} is present but not recorded"
        );
    }
    Ok(())
}

/// What validating a staged archive established about the state it holds.
struct RestoredState {
    validation: SetupValidationSummary,
    configured: bool,
    provider: Option<Pubkey>,
    allocations: BTreeMap<String, AllocationIdentity>,
}

async fn load_allocation_identities(
    database: &Database,
) -> anyhow::Result<BTreeMap<String, AllocationIdentity>> {
    let rows = sqlx::query(
        "SELECT federation_id, requester_pubkey, provider_pubkey, network, \
         details_payload_hash FROM allocations ORDER BY federation_id",
    )
    .fetch_all(database.pool())
    .await
    .context("failed to read accepted allocation identities")?;

    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("federation_id")?,
                AllocationIdentity {
                    requester_pubkey: row.try_get("requester_pubkey")?,
                    provider_pubkey: row.try_get("provider_pubkey")?,
                    network: row.try_get("network")?,
                    details_payload_hash: row.try_get("details_payload_hash")?,
                },
            ))
        })
        .collect::<Result<_, sqlx::Error>>()
        .context("failed to decode accepted allocation identities")
}

async fn validate_restored_state(
    args: &DaemonArgs,
    staging_dir: &Path,
    sqlite_relative: &Path,
) -> anyhow::Result<RestoredState> {
    let restored_sqlite = staging_dir.join(sqlite_relative);
    ensure!(
        restored_sqlite.is_file(),
        "backup archive is missing restored SQLite database at {}",
        sqlite_relative.display()
    );

    let restored_paths = DaemonPaths {
        data_dir: staging_dir.to_path_buf(),
        sqlite_path: restored_sqlite,
        secret_store_key: staging_dir.join("secret-store.key"),
        federations_dir: staging_dir.join("federations"),
        lock_file: staging_dir.join(LOCK_FILE_NAME),
    };
    let database = Database::connect(&restored_paths.sqlite_path)
        .await
        .context("failed to open restored SQLite database")?;
    database
        .ping()
        .await
        .context("restored SQLite health check failed")?;
    let secret_store = SecretStore::load_or_create(
        &restored_paths.secret_store_key,
        args.secret_store_key.as_deref(),
    )
    .context("failed to load restored secret-store key")?;
    // A precondition, not a check: an archive this daemon cannot decrypt must
    // not reach the data dir, because the daemon would come up healthy and fail
    // every secret-backed operation, admin authentication included.
    setup_store::ensure_secret_records_decryptable(&database, &secret_store)
        .await
        .map_err(anyhow_from_service_error)?;
    // Local checks only. A restore-mode process must not dial hosts named by the
    // archive it was handed: recovery would depend on a gateway that is often the
    // thing that is down, and `gateway_api_view_check` would send the archive's
    // own gateway admin credential to the archive's own URL with no endpoint
    // policy applied. The summary is informational — `stage_restore` does not
    // gate on it, and the daemon revalidates in full on normal boot.
    let validation = setup_store::validate_restored_setup(&database, &secret_store)
        .await
        .map_err(anyhow_from_service_error)?;
    let configured = setup_store::load_setup_state(&database)
        .await
        .map_err(anyhow_from_service_error)?
        .config
        .is_some();
    // Absent is a legitimate state (a backup taken before identity install),
    // so this distinguishes "no identity" from "could not be read".
    let provider = crate::identity::load_provider_identity(&database)
        .await
        .ok();
    let allocations = load_allocation_identities(&database).await?;

    // The staged database is only inspected here; the generation that will
    // serve from it opens its own pool after the swap.
    database.close().await;

    Ok(RestoredState {
        validation,
        configured,
        provider,
        allocations,
    })
}

fn move_staged_contents(staging_dir: &Path, data_dir: &Path) -> anyhow::Result<()> {
    move_dir_contents(staging_dir, data_dir, |_| false)
        .context("failed to move restored state into the data dir")?;
    let _ = fs::remove_dir(staging_dir);
    Ok(())
}

/// Swaps staged state into a live data dir, moving what is there aside first.
///
/// Returns the directory the previous state was moved to. It is deliberately
/// left on disk: a restore replaces every durable thing the daemon owns, and
/// deleting the only copy of the state it replaced is not this function's call
/// to make.
///
/// The lock file stays put — the daemon still holds it, and it is not restorable
/// state — and so does `backups/`, which is excluded from archives for the same
/// reason. Relocating an operator's archive collection as a side effect of using
/// one of them would be a surprise.
pub(crate) fn commit_live_restore(
    paths: &DaemonPaths,
    staged: &StagedRestore,
) -> anyhow::Result<PathBuf> {
    let aside_dir = aside_dir_for(&paths.data_dir);
    fs::create_dir_all(&aside_dir)
        .with_context(|| format!("failed to create pre-restore dir {}", aside_dir.display()))?;

    move_dir_contents(&paths.data_dir, &aside_dir, is_retained_across_restore)
        .context("failed to move live state aside; data dir is unchanged")?;

    match move_staged_contents(&staged.staging_dir, &paths.data_dir) {
        Ok(()) => Ok(aside_dir),
        Err(error) => {
            // The data dir is now part-restored. Put back what was moved aside
            // so the caller can rebuild the generation it already tore down.
            match rollback_live_restore(paths, &aside_dir) {
                Ok(()) => {
                    Err(error.context("restore rolled back; previous state is back in place"))
                }
                Err(rollback_error) => Err(error.context(format!(
                    "restore failed and rollback also failed ({rollback_error}); \
                     previous state is in {}",
                    aside_dir.display()
                ))),
            }
        }
    }
}

/// Puts state moved aside by [`commit_live_restore`] back into the data dir,
/// discarding whatever a failed restore left there.
pub(crate) fn rollback_live_restore(paths: &DaemonPaths, aside_dir: &Path) -> anyhow::Result<()> {
    for entry in read_dir_sorted(&paths.data_dir)? {
        let path = entry.path();
        if is_retained_across_restore(&entry.file_name()) {
            continue;
        }
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed
            .with_context(|| format!("failed to clear partly restored state {}", path.display()))?;
    }

    move_dir_contents(aside_dir, &paths.data_dir, |_| false)
        .context("failed to move previous state back into the data dir")?;
    let _ = fs::remove_dir(aside_dir);
    Ok(())
}

/// Entries that belong to the running process rather than to restorable state.
fn is_retained_across_restore(file_name: &std::ffi::OsString) -> bool {
    file_name == LOCK_FILE_NAME || file_name == BACKUP_DIR_NAME
}

/// A SQLite write-ahead-log sidecar, which the engine creates and removes on its
/// own schedule alongside the database it belongs to.
///
/// These are the only names this process does not fully control the lifetime of
/// inside a directory it just populated: closing a pool unlinks them, and the
/// unlink does not always land before the next `read_dir`. Nothing else in a data
/// or staging directory is removed by anyone but this process.
fn is_sqlite_sidecar(file_name: &std::ffi::OsStr) -> bool {
    let Some(name) = file_name.to_str() else {
        return false;
    };
    name.ends_with("-wal") || name.ends_with("-shm")
}

fn move_dir_contents(
    source: &Path,
    destination: &Path,
    skip: impl Fn(&std::ffi::OsString) -> bool,
) -> anyhow::Result<()> {
    let listed = read_dir_sorted(source)?
        .into_iter()
        .map(|entry| (entry.file_name(), entry.path()))
        .collect::<Vec<_>>();
    move_listed_entries(listed, destination, skip)
}

/// The move itself, split from the listing so a test can drive the one condition
/// that matters here: an entry that was listed and is gone by the time it is
/// renamed. That cannot be staged through `move_dir_contents`, because the
/// listing happens inside it.
fn move_listed_entries(
    listed: Vec<(std::ffi::OsString, PathBuf)>,
    destination: &Path,
    skip: impl Fn(&std::ffi::OsString) -> bool,
) -> anyhow::Result<()> {
    for (file_name, from) in listed {
        if skip(&file_name) {
            continue;
        }
        let to = destination.join(&file_name);
        match fs::rename(&from, &to) {
            Ok(()) => {}
            // The listing above is a snapshot, and SQLite unlinks its own
            // sidecars when the pool that owns them closes. That unlink can land
            // between the `read_dir` and this rename; without this arm the whole
            // restore fails there — *after* the running generation has already
            // been torn down.
            //
            // The tolerance is deliberately narrow. It is not "ignore missing
            // files": it is `NotFound` on the two suffixes whose deleter is
            // known, in a directory this process just populated. Anything else
            // vanishing is still an error, because nothing else has a legitimate
            // deleter here.
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && is_sqlite_sidecar(&file_name) =>
            {
                tracing::debug!(
                    path = %from.display(),
                    "restore: sqlite sidecar removed itself between listing and move"
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to move {} to {}", from.display(), to.display())
                });
            }
        }
    }
    Ok(())
}

fn read_dir_sorted(path: &Path) -> anyhow::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("failed to read dir {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read dir {}", path.display()))?;
    entries.sort_by_key(std::fs::DirEntry::path);
    Ok(entries)
}

fn aside_dir_for(data_dir: &Path) -> PathBuf {
    let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let name = data_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("flip");
    parent.join(format!(".{name}.pre-restore-{}", unique_suffix()))
}

fn ensure_restore_target_empty(data_dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create restore target {}", data_dir.display()))?;
    for entry in fs::read_dir(data_dir)
        .with_context(|| format!("failed to read restore target {}", data_dir.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read restore target entry in {}",
                data_dir.display()
            )
        })?;
        if entry.file_name() == LOCK_FILE_NAME {
            continue;
        }

        bail!(
            "restore target data dir must be empty before restore; found {}",
            entry.path().display()
        );
    }

    Ok(())
}

fn archive_entry_path<R: Read>(entry: &tar::Entry<'_, R>) -> anyhow::Result<PathBuf> {
    let path = entry.path().context("failed to read archive entry path")?;
    normalize_archive_path(path.as_ref())
}

fn normalize_archive_path(path: &Path) -> anyhow::Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty(),
        "backup archive contains empty path"
    );

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("backup archive path must be relative: {}", path.display())
            }
        }
    }

    ensure!(
        !normalized.as_os_str().is_empty(),
        "backup archive contains empty path"
    );
    Ok(normalized)
}

fn validate_archive_payload_path(relative_path: &Path) -> anyhow::Result<()> {
    if relative_path == Path::new(MANIFEST_ENTRY) {
        return Ok(());
    }
    ensure!(
        !is_excluded_payload_path(relative_path),
        "backup archive contains excluded path {}",
        relative_path.display()
    );
    Ok(())
}

fn archive_entry_kind(entry_type: EntryType) -> anyhow::Result<ArchiveEntryKind> {
    if entry_type.is_dir() {
        Ok(ArchiveEntryKind::Directory)
    } else if entry_type.is_file() {
        Ok(ArchiveEntryKind::File)
    } else {
        bail!("backup archive contains unsupported entry type")
    }
}

fn is_excluded_payload_path(relative_path: &Path) -> bool {
    relative_path == Path::new(LOCK_FILE_NAME)
        || relative_path == Path::new(PUBLIC_ENDPOINT_ADDR_TEMP_FILE)
        || relative_path
            .components()
            .next()
            .is_some_and(|component| component == Component::Normal(BACKUP_DIR_NAME.as_ref()))
}

fn sqlite_relative_path(paths: &DaemonPaths) -> ServiceResult<PathBuf> {
    paths
        .sqlite_path
        .strip_prefix(&paths.data_dir)
        .map(Path::to_path_buf)
        .map_err(|_| {
            failed_precondition(format!(
                "SQLite path {} must be under FLIP data dir {} for data-dir backup/restore",
                paths.sqlite_path.display(),
                paths.data_dir.display()
            ))
        })
}

fn create_private_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("failed to create file {}", path.display()))
}

fn staging_dir_for(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let name = data_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("flip");
    Ok(parent.join(format!(
        ".{name}.restore-staging-{}-{}",
        std::process::id(),
        unique_suffix()
    )))
}

fn manifest(created_at: Timestamp, quiesced_at: Timestamp) -> BackupManifest {
    BackupManifest {
        version: BACKUP_FORMAT_VERSION,
        created_at,
        state_groups: vec![
            BackupStateGroup::ProviderIdentity,
            BackupStateGroup::Attestations,
            BackupStateGroup::WalletClientState,
            BackupStateGroup::Database,
            BackupStateGroup::OperationHistory,
            BackupStateGroup::OperatorConfig,
            BackupStateGroup::ExternalDependencies,
        ],
        recovery_point: BackupRecoveryPoint {
            quiesced_at,
            stores: vec![BackupStore::Sqlite, BackupStore::DataDirectory],
        },
    }
}

fn restored_status(
    configured: bool,
    validation: &fedi_decentralized_service_liquidity_manager::SetupValidationSummary,
) -> SetupStatus {
    if !configured {
        return SetupStatus::NotConfigured;
    }

    if validation
        .checks
        .iter()
        .all(|check| check.status == ValidationStatus::Passed)
    {
        SetupStatus::Ready
    } else {
        SetupStatus::PendingValidation
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn anyhow_from_service_error(error: ServiceError) -> anyhow::Error {
    anyhow::anyhow!("{}: {}", error.code(), error)
}

#[cfg(test)]
#[path = "../tests/backup.rs"]
mod tests;
