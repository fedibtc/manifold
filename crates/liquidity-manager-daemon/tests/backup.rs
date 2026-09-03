use super::*;

/// A pass that never finishes refuses the backup instead of stalling the
/// daemon.
///
/// This bound protects the workers rather than the backup. Tokio's `RwLock`
/// is write-preferring, so a queued backup stops every *other* pass from
/// starting; waiting forever would turn one stuck pass into a stalled daemon
/// the moment an operator asked for a backup. Giving up drops the queued
/// write request and the workers behind it proceed — which is the second
/// assertion here, and the one that makes this a fix rather than a message.
#[tokio::test]
async fn a_stuck_worker_pass_refuses_a_backup_rather_than_stalling_the_daemon() -> anyhow::Result<()>
{
    let context = crate::test_support::production_test_context(
        "backup-quiescence-timeout",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;

    let stuck_pass = context.work_quiescence.pass().await;
    let error = create_backup_within(&context, std::time::Duration::from_millis(50))
        .await
        .expect_err("a backup must not wait on a stuck pass forever");
    assert_eq!(
        error.code(),
        fedi_decentralized_service_liquidity_manager::ServiceErrorCode::Unavailable
    );
    assert!(
        error.message().contains("quiescence window"),
        "the refusal must name its cause: {error}"
    );

    // The refusal released the queued write request, so ordinary passes are
    // not blocked behind a backup that gave up.
    drop(stuck_pass);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            context.work_quiescence.pass()
        )
        .await
        .is_ok(),
        "a worker pass must still be able to start after a refused backup"
    );
    Ok(())
}

/// A backup waits for worker passes, then captures both stores at once.
///
/// This enters `create_backup` itself rather than `create_archive`, because the
/// property is about *when* the payload is read and no test of the archive
/// writer can see that.
///
/// The barrier assertion is the load-bearing half: a worker pass is held in
/// flight, `create_backup` is started, and it must not finish. Without the
/// quiescence barrier it finishes immediately and the archive walks live stores
/// while workers run.
#[tokio::test]
async fn a_backup_waits_for_worker_passes_before_capturing() -> anyhow::Result<()> {
    let context = crate::test_support::production_test_context(
        "backup-quiescence",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;
    // A durable row, so the staged snapshot has something to be a snapshot of.
    sqlx::query("INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, NULL, 1)")
        .bind("backup-quiescence-probe")
        .execute(context.database.pool())
        .await?;

    let pass = context.work_quiescence.pass().await;
    let backup_context = context.clone();
    let mut backup = tokio::spawn(async move {
        create_backup(&backup_context)
            .await
            .map_err(anyhow_from_service_error)
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut backup)
            .await
            .is_err(),
        "a backup must not capture while a worker pass is in flight"
    );

    drop(pass);
    let response = backup.await??;

    // The recovery point is recorded in the manifest rather than implied, so a
    // reader of the archive can tell what it covers.
    assert_eq!(
        response.manifest.recovery_point.stores,
        vec![BackupStore::Sqlite, BackupStore::DataDirectory]
    );
    assert!(response.manifest.recovery_point.quiesced_at.0 > 0);
    assert!(response.manifest.recovery_point.quiesced_at.0 <= response.manifest.created_at.0);

    // The staged database is a complete copy, so the archive carries no
    // write-ahead sidecars that could disagree with it.
    let archive_path = PathBuf::from(&response.archive.0);
    let archive_bytes = decompressed_archive_bytes(&archive_path)?;
    assert!(contains_bytes(&archive_bytes, b"backup-quiescence-probe"));
    assert!(
        !contains_bytes(&archive_bytes, b"flip.sqlite-wal"),
        "VACUUM INTO makes the write-ahead log unnecessary, so it must not be archived"
    );

    // The staging copy carries the same secrets the archive does and must
    // not outlive the backup.
    let leftovers: Vec<_> = fs::read_dir(
        context
            .paths
            .data_dir
            .parent()
            .expect("the test data dir has a parent"),
    )?
    .filter_map(Result::ok)
    .filter(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .contains(".backup-staging-")
    })
    .collect();
    assert!(
        leftovers.is_empty(),
        "backup staging directories were left behind: {leftovers:?}"
    );
    Ok(())
}

#[test]
fn archive_round_trip_excludes_transient_runtime_files() -> anyhow::Result<()> {
    let temp = TestDir::new("backup-round-trip")?;
    fs::write(temp.path.join("flip.sqlite"), b"sqlite")?;
    fs::write(temp.path.join("secret-store.key"), b"secret-key")?;
    fs::write(temp.path.join(LOCK_FILE_NAME), b"pid=1")?;
    fs::write(
        temp.path.join(PUBLIC_ENDPOINT_ADDR_TEMP_FILE),
        b"partial-endpoint-address",
    )?;
    fs::create_dir_all(temp.path.join(BACKUP_DIR_NAME))?;
    fs::write(temp.path.join(BACKUP_DIR_NAME).join("old.tar.gz"), b"old")?;

    let paths = test_paths(&temp.path);
    let manifest = manifest(Timestamp(1_700_000_000), Timestamp(1_699_999_990));
    let archive_path = create_archive(&paths, &paths.data_dir, &manifest)?;
    assert!(
        archive_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".tar.gz"))
    );
    let inspected = inspect_archive(&archive_path)?;
    assert_eq!(inspected, manifest);

    let archive_bytes = decompressed_archive_bytes(&archive_path)?;
    assert!(!contains_bytes(&archive_bytes, b"old"));
    assert!(!contains_bytes(&archive_bytes, b"pid=1"));
    assert!(!contains_bytes(&archive_bytes, b"partial-endpoint-address"));

    Ok(())
}

#[test]
fn endpoint_publication_temp_can_disappear_after_backup_collection() -> anyhow::Result<()> {
    let temp = TestDir::new("backup-endpoint-publication")?;
    let temporary_endpoint = temp.path.join(PUBLIC_ENDPOINT_ADDR_TEMP_FILE);
    fs::write(&temporary_endpoint, b"partial-endpoint-address")?;

    let entries = collect_payload_entries(&temp.path)?;
    fs::remove_file(temporary_endpoint)?;

    assert!(entries.iter().all(|entry| entry.source_path.exists()));
    Ok(())
}

/// A live restore rebinds nothing: the public node id is derived from the
/// provider identity, so an archive carrying a different one would leave the
/// daemon reachable at an address its advertisements do not name.
#[test]
fn restoring_a_foreign_provider_identity_is_rejected() {
    let running = Pubkey("aaaa".to_owned());
    let foreign = Pubkey("bbbb".to_owned());

    let error = ensure_restorable_identity(Some(&running), Some(&foreign))
        .expect_err("a foreign identity should be rejected");
    assert!(
        error.to_string().contains("public node id"),
        "the error should explain why, got: {error}"
    );

    let error = ensure_restorable_identity(Some(&running), None)
        .expect_err("an archive with no identity should be rejected");
    assert!(error.to_string().contains("unable to rebind"));
}

/// The two cases that are safe: same identity, and a daemon that has none
/// yet and so has nothing bound or published to contradict.
#[test]
fn restoring_a_matching_or_absent_identity_is_allowed() -> anyhow::Result<()> {
    let running = Pubkey("aaaa".to_owned());
    ensure_restorable_identity(Some(&running), Some(&Pubkey("aaaa".to_owned())))?;
    ensure_restorable_identity(None, Some(&Pubkey("cccc".to_owned())))?;
    ensure_restorable_identity(None, None)?;
    Ok(())
}

/// The swap replaces restorable state, keeps what belongs to the running
/// process, and can be undone.
#[test]
fn committing_a_live_restore_moves_state_aside_and_rolls_back() -> anyhow::Result<()> {
    let temp = TestDir::new("live-restore-commit")?;
    let data_dir = temp.path.join("data");
    fs::create_dir_all(&data_dir)?;
    fs::write(data_dir.join("flip.sqlite"), b"live-db")?;
    fs::write(data_dir.join("secret-store.key"), b"live-key")?;
    fs::write(data_dir.join(LOCK_FILE_NAME), b"pid=1")?;
    fs::create_dir_all(data_dir.join(BACKUP_DIR_NAME))?;
    fs::write(
        data_dir.join(BACKUP_DIR_NAME).join("old.tar.gz"),
        b"archive",
    )?;

    let staging_dir = temp.path.join("staging");
    fs::create_dir_all(&staging_dir)?;
    fs::write(staging_dir.join("flip.sqlite"), b"restored-db")?;
    fs::write(staging_dir.join("secret-store.key"), b"restored-key")?;

    let paths = test_paths(&data_dir);
    let staged = StagedRestore {
        staging_dir,
        response: RestoreBackupResponse {
            status: SetupStatus::NotConfigured,
            validation: SetupValidationSummary {
                status: ValidationStatus::NotRun,
                checks: Vec::new(),
            },
            restored_state_groups: Vec::new(),
        },
        allocations: BTreeMap::new(),
    };

    let aside_dir = commit_live_restore(&paths, &staged)?;

    assert_eq!(fs::read(data_dir.join("flip.sqlite"))?, b"restored-db");
    assert_eq!(
        fs::read(data_dir.join("secret-store.key"))?,
        b"restored-key"
    );
    // The lock belongs to the process, and archives are not restorable state.
    assert_eq!(fs::read(data_dir.join(LOCK_FILE_NAME))?, b"pid=1");
    assert_eq!(
        fs::read(data_dir.join(BACKUP_DIR_NAME).join("old.tar.gz"))?,
        b"archive"
    );
    // The displaced state is kept, not deleted.
    assert_eq!(fs::read(aside_dir.join("flip.sqlite"))?, b"live-db");

    rollback_live_restore(&paths, &aside_dir)?;

    assert_eq!(fs::read(data_dir.join("flip.sqlite"))?, b"live-db");
    assert_eq!(fs::read(data_dir.join("secret-store.key"))?, b"live-key");
    assert_eq!(fs::read(data_dir.join(LOCK_FILE_NAME))?, b"pid=1");
    assert!(!aside_dir.exists(), "rollback should consume the aside dir");

    Ok(())
}

#[test]
fn inspect_rejects_path_traversal() -> anyhow::Result<()> {
    let error = normalize_archive_path(Path::new("../evil")).expect_err("path should be rejected");
    assert!(error.to_string().contains("relative"));
    Ok(())
}

#[test]
fn inspect_rejects_symlink_entries() -> anyhow::Result<()> {
    let temp = TestDir::new("backup-symlink")?;
    let archive_path = temp.path.join("bad.tar.gz");
    let file = File::create(&archive_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);
    append_manifest(
        &mut builder,
        &manifest(Timestamp(1_700_000_000), Timestamp(1_699_999_990)),
    )?;
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_cksum();
    header.set_link_name("target")?;
    builder.append_data(&mut header, "link", std::io::empty())?;
    let encoder = builder.into_inner()?;
    encoder.finish()?;

    let error = inspect_archive(&archive_path).expect_err("archive should be rejected");
    assert!(error.to_string().contains("unsupported entry type"));
    Ok(())
}

/// The restore move must survive SQLite unlinking its own sidecar.
///
/// `move_dir_contents` lists the directory and then renames each entry, and
/// closing the staged pool unlinks `-wal` on SQLite's own schedule. When that
/// landed in between, the whole restore failed — *after* the running
/// generation had been torn down, so the daemon was left serving nothing. It
/// reproduced at about two runs in eight.
///
/// The second half is the control that keeps the tolerance narrow. Without
/// it, "skip anything missing" would pass this test just as well, and a
/// genuinely lost database file would move a broken restore forward in
/// silence.
#[test]
fn a_sidecar_that_vanishes_between_listing_and_move_does_not_fail_the_restore() -> anyhow::Result<()>
{
    let temp = TestDir::new("restore-sidecar-race")?;
    let source = temp.path.join("staging");
    let destination = temp.path.join("data");
    fs::create_dir_all(&source)?;
    fs::create_dir_all(&destination)?;

    fs::write(source.join("flip.sqlite"), b"sqlite")?;
    // Listed, then unlinked by SQLite before the rename reaches it. Never
    // written, which is exactly the state the race produces.
    let vanished_wal = (
        std::ffi::OsString::from("flip.sqlite-wal"),
        source.join("flip.sqlite-wal"),
    );
    let listed = vec![
        (
            std::ffi::OsString::from("flip.sqlite"),
            source.join("flip.sqlite"),
        ),
        vanished_wal,
    ];

    move_listed_entries(listed, &destination, |_| false)?;

    assert!(
        destination.join("flip.sqlite").exists(),
        "the database itself must still be moved"
    );
    assert!(
        !destination.join("flip.sqlite-wal").exists(),
        "a sidecar that no longer exists cannot arrive"
    );

    // The control: the same missing-file condition on a name SQLite does not
    // own must still fail the restore.
    let listed = vec![(
        std::ffi::OsString::from("secret-store.key"),
        source.join("secret-store.key"),
    )];
    let error = move_listed_entries(listed, &destination, |_| false)
        .expect_err("a missing non-sidecar entry must fail the move");
    assert!(
        error.to_string().contains("failed to move"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
fn only_sqlite_sidecar_suffixes_are_tolerated() {
    assert!(is_sqlite_sidecar(std::ffi::OsStr::new("flip.sqlite-wal")));
    assert!(is_sqlite_sidecar(std::ffi::OsStr::new("flip.sqlite-shm")));
    assert!(!is_sqlite_sidecar(std::ffi::OsStr::new("flip.sqlite")));
    assert!(!is_sqlite_sidecar(std::ffi::OsStr::new("secret-store.key")));
    assert!(!is_sqlite_sidecar(std::ffi::OsStr::new("wal")));
}

fn test_paths(data_dir: &Path) -> DaemonPaths {
    DaemonPaths {
        data_dir: data_dir.to_path_buf(),
        sqlite_path: data_dir.join("flip.sqlite"),
        secret_store_key: data_dir.join("secret-store.key"),
        federations_dir: data_dir.join("federations"),
        lock_file: data_dir.join(LOCK_FILE_NAME),
    }
}

/// A backup whose rot you cannot see is a backup you cannot rely on.
///
/// The corruption is deliberately well-formed: the tar stays valid, the
/// entry keeps its recorded size, and only the file's bytes change — which
/// is what a bad sector or a mangled transfer looks like. Nothing else on
/// the restore path inspects payload bytes, so before the digests this
/// archive restored silently and the daemon came up on it.
#[test]
fn a_corrupted_payload_is_refused_rather_than_restored() -> anyhow::Result<()> {
    let temp = TestDir::new("backup-corruption")?;
    fs::write(temp.path.join("flip.sqlite"), b"sqlite-payload")?;
    fs::write(temp.path.join("secret-store.key"), b"secret-key")?;

    let paths = test_paths(&temp.path);
    let archive_path = create_archive(
        &paths,
        &paths.data_dir,
        &manifest(Timestamp(1_700_000_000), Timestamp(1_699_999_990)),
    )?;

    // Control: the untouched archive restores, so the refusal below is
    // about the corruption and not about the archive or the harness.
    let good = TestDir::new("backup-corruption-good")?;
    extract_archive(&archive_path, &good.path)?;
    assert_eq!(fs::read(good.path.join("flip.sqlite"))?, b"sqlite-payload");

    let corrupted = rewrite_payload(&archive_path, b"sqlite-payload", b"sqlite-p4yload")?;
    let bad = TestDir::new("backup-corruption-bad")?;
    let error = extract_archive(&corrupted.path.join("corrupt.tar.gz"), &bad.path)
        .expect_err("a corrupted payload must not restore");
    assert!(
        error
            .to_string()
            .contains("does not match its recorded checksum"),
        "unexpected error: {error}"
    );

    Ok(())
}

/// The digests have to answer "is this the whole archive", not only "is
/// each file it happens to contain intact". A truncated copy loses whole
/// entries, and per-file digests over what survived would all pass.
#[test]
fn a_missing_or_unrecorded_file_is_refused() {
    let declared = BTreeMap::from([
        ("flip.sqlite".to_owned(), "aa".to_owned()),
        ("secret-store.key".to_owned(), "bb".to_owned()),
    ]);

    let truncated = BTreeMap::from([("flip.sqlite".to_owned(), "aa".to_owned())]);
    let error = verify_checksums(&declared, &truncated)
        .expect_err("a recorded file that never arrived must be refused");
    assert!(error.to_string().contains("is recorded but missing"));

    let mut extra = declared.clone();
    extra.insert("unexpected".to_owned(), "cc".to_owned());
    let error =
        verify_checksums(&declared, &extra).expect_err("a file nobody recorded must be refused");
    assert!(error.to_string().contains("is present but not recorded"));

    assert!(verify_checksums(&declared, &declared).is_ok());
}

/// A held restore target keeps a second archive out of the data dir.
///
/// `restore_backup` checks the data dir is empty, stages an archive, checks
/// again, and moves the staged contents in. Two callers that interleave those
/// steps each land a different archive in one root, and per-archive checksums
/// verify both, so nothing downstream reports the mixture.
///
/// The load-bearing assertions are the two about the data dir. The refused
/// caller must leave nothing behind, and the caller that follows it must land
/// its own archive alone. A `restore` that stopped consulting the target would
/// put `unwanted` in the data dir next to `wanted`, which is the merge this
/// pins.
#[tokio::test]
async fn a_restore_cannot_land_while_another_holds_the_target() -> anyhow::Result<()> {
    let (_wanted_source, wanted) = restorable_archive("restore-target-wanted", "wanted").await?;
    let (_unwanted_source, unwanted) =
        restorable_archive("restore-target-unwanted", "unwanted").await?;

    let temp = TestDir::new("restore-target")?;
    let paths = test_paths(&temp.path);
    let args = restore_mode_args(&paths);
    let target = RestoreTarget::default();

    let held = target.begin().expect("the target starts free");

    let error = target
        .restore(&args, &paths, restore_request(&unwanted))
        .await
        .expect_err("a restore must be refused while another holds the target");
    assert_eq!(
        error.code(),
        fedi_decentralized_service_liquidity_manager::ServiceErrorCode::Unavailable
    );
    assert!(
        error.message().contains("restore is already in progress"),
        "the refusal must name its cause: {error}"
    );
    assert!(
        !temp.path.join("unwanted").exists(),
        "a refused restore must not reach the data dir"
    );

    // The target is taken for one restore, not for the process: the next
    // caller gets it, and lands its own archive by itself.
    drop(held);
    target
        .restore(&args, &paths, restore_request(&wanted))
        .await
        .map_err(anyhow_from_service_error)?;
    assert!(temp.path.join("wanted").exists());
    assert!(
        !temp.path.join("unwanted").exists(),
        "the data dir must carry one archive, not two"
    );

    Ok(())
}

/// Builds an archive of a data dir carrying a real database and one marker
/// file, so a restore of it both validates and is identifiable afterwards.
///
/// The returned [`TestDir`] owns the archive; dropping it removes the file.
async fn restorable_archive(name: &str, marker: &str) -> anyhow::Result<(TestDir, PathBuf)> {
    let source = TestDir::new(name)?;
    let paths = test_paths(&source.path);
    // A restore opens and pings the database it staged, so a placeholder file
    // would be rejected before the move this test is about.
    Database::connect(&paths.sqlite_path).await?;
    fs::write(source.path.join(marker), marker.as_bytes())?;

    let archive = create_archive(
        &paths,
        &paths.data_dir,
        &manifest(Timestamp(1_700_000_000), Timestamp(1_699_999_990)),
    )?;
    Ok((source, archive))
}

fn restore_request(archive: &Path) -> RestoreBackupRequest {
    RestoreBackupRequest {
        archive: BackupArchive(archive.display().to_string()),
    }
}

fn restore_mode_args(paths: &DaemonPaths) -> DaemonArgs {
    DaemonArgs {
        manifold_environment:
            fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
        data_dir: paths.data_dir.clone(),
        sqlite_path: paths.sqlite_path.clone(),
        admin_bind_address: "127.0.0.1:0".parse().expect("a valid bind address"),
        public_bind_address: "127.0.0.1:0".parse().expect("a valid bind address"),
        bootstrap_admin_token: Some("test-admin-token".to_owned()),
        // None, so the staged archive's own key file is what the restore reads.
        secret_store_key: None,
        allow_bootstrap_token_fallback: false,
        mode: crate::config::DaemonMode::Restore,
        provider_nostr_secret_key: None,
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    }
}

/// Rewrites an archive with one payload substring replaced by another of
/// the same length, so every tar header stays correct.
fn rewrite_payload(path: &Path, from: &[u8], to: &[u8]) -> anyhow::Result<TestDir> {
    assert_eq!(
        from.len(),
        to.len(),
        "replacement must not resize the entry"
    );
    let mut bytes = decompressed_archive_bytes(path)?;
    let at = bytes
        .windows(from.len())
        .position(|window| window == from)
        .context("payload to corrupt is not in the archive")?;
    bytes[at..at + to.len()].copy_from_slice(to);

    let dir = TestDir::new("backup-corruption-rewrite")?;
    let mut encoder = GzEncoder::new(
        File::create(dir.path.join("corrupt.tar.gz"))?,
        Compression::default(),
    );
    encoder.write_all(&bytes)?;
    encoder.finish()?;
    Ok(dir)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn decompressed_archive_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;
    Ok(bytes)
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join("fedi-flip-tests").join(format!(
            "{name}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
