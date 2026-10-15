//! Backup restore through the run loop, against real local Nostr relays.
//!
//! A formed FA publishes its backup with the production relay workers, or a
//! test publishes hand-made events under the same backup key. A second FA with
//! the same identity opens an empty database with a restored-mnemonic hint and
//! only runs [`FaClient::run`].

use std::process::{Child, Command, Stdio};

use fedi_decentralized_nostr_clients::NostrRelayClient;
use nostr_sdk::{RelayUrl, Timestamp as NostrTimestamp};

use super::crash_db::{CrashPlan, CrashPoint, CrashStorage};
use super::*;
use crate::backup::{FA_BACKUP_D_TAG, FA_BACKUP_EVENT_KIND, FaBackupKeys, FaBackupPayload};

const RELAY_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A `nostr-rs-relay` process listening on a loopback port.
struct LocalRelay {
    child: Child,
    url: RelayUrl,
    _dir: tempfile::TempDir,
}

impl LocalRelay {
    async fn start() -> Self {
        // A relay whose port another process took panics at once, while
        // that process answers our readiness probe. Detect it and move on.
        for _ in 0..5 {
            if let Some(relay) = Self::try_start_on(free_port()).await {
                return relay;
            }
        }
        panic!("nostr-rs-relay never got a free port");
    }

    /// Start a relay on `port`, or `None` when another process holds it.
    async fn try_start_on(port: u16) -> Option<Self> {
        let dir = tempfile::tempdir().expect("relay directory");
        let data = dir.path().join("db");
        std::fs::create_dir_all(&data).expect("relay data directory");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "[database]\ndata_directory = \"{}\"\n\n[network]\naddress = \"127.0.0.1\"\nport = {port}\n",
                data.display()
            ),
        )
        .expect("relay config");
        let relay_bin = std::env::var_os("DEV_DEFE_NOSTR_RS_RELAY_BIN")
            .unwrap_or_else(|| "nostr-rs-relay".into());
        let mut child = Command::new(relay_bin)
            .arg("--config")
            .arg(&config)
            .current_dir(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start nostr-rs-relay from the dev shell");
        let ready_by = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            if child.try_wait().expect("relay status").is_some() {
                return None;
            }
            if listens_on(child.id(), port) {
                break;
            }
            assert!(std::time::Instant::now() < ready_by, "relay never listened");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Some(Self {
            child,
            url: relay_url(port),
            _dir: dir,
        })
    }
}

/// Whether process `pid` holds the TCP listener on `127.0.0.1:port`. A
/// connect probe cannot tell: it also succeeds against another process that
/// took the port.
fn listens_on(pid: u32, port: u16) -> bool {
    let local = format!("0100007F:{port:04X}");
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return false;
    };
    // Columns: sl, local_address, rem_address, st, ..., inode (index 9).
    let Some(inode) = table.lines().skip(1).find_map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        (fields.get(1) == Some(&local.as_str()) && fields.get(3) == Some(&"0A"))
            .then(|| fields.get(9).map(|inode| (*inode).to_owned()))
            .flatten()
    }) else {
        return false;
    };
    let socket = format!("socket:[{inode}]");
    std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| {
        fds.flatten().any(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|target| target.as_os_str() == socket.as_str())
        })
    })
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    defe_portalloc::port_alloc(1).expect("allocate a loopback port")
}

fn relay_url(port: u16) -> RelayUrl {
    RelayUrl::parse(&format!("ws://127.0.0.1:{port}")).expect("relay URL")
}

/// A relay URL nothing listens on.
fn down_relay() -> RelayUrl {
    relay_url(free_port())
}

fn relay_options() -> FormationRunOptions {
    FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_secs(30),
        request_timeout: Duration::from_secs(5),
    })
    .unwrap()
}

fn backup_keys() -> FaBackupKeys {
    FaBackupKeys::derive(&TestIdentity.scoped_root())
}

/// A backup event as the coordinator builds it, with explicit content.
fn backup_event(content: &str, created_at: u64) -> Event {
    let keys = backup_keys();
    EventBuilder::new(Kind::Custom(FA_BACKUP_EVENT_KIND), content)
        .tags([Tag::identifier(FA_BACKUP_D_TAG)])
        .custom_created_at(NostrTimestamp::from_secs(created_at))
        .sign_with_keys(keys.author())
        .expect("sign backup event")
}

fn sealed_event(payload: &FaBackupPayload, created_at: u64) -> Event {
    backup_event(backup_keys().seal(payload).unwrap().content(), created_at)
}

async fn publish(relay: &RelayUrl, event: &Event) {
    let client = NostrRelayClient::connect_without_signer(relay, RELAY_IO_TIMEOUT)
        .await
        .expect("connect to local relay");
    assert_eq!(client.publish_signed_event(event).await.unwrap(), event.id);
}

/// Publish the formed FA's backup with the production relay workers, and
/// wait until every relay confirmed it.
async fn publish_with_workers(source: &TestClient, relays: &[RelayUrl]) -> TaskGroup {
    let tasks = TaskGroup::new();
    crate::backup_worker::spawn_workers(
        &tasks,
        source.inner.store.clone(),
        source.inner.ports.identity.scoped_root(),
        relays.to_vec(),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        for relay in relays {
            while source
                .inner
                .store
                .backup_confirmation(&relay.to_string())
                .await
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    })
    .await
    .expect("every relay confirms the backup");
    tasks
}

/// An FA opened from the restored mnemonic of [`TestIdentity`].
async fn open_restored(
    database: fedimint_core::db::Database,
    state: Arc<MgState>,
    relays: Vec<RelayUrl>,
    required_reads: usize,
) -> TestClient {
    let (payments, _) = TestPayments::new();
    FaClient::open_inner(
        database,
        FaClientPorts {
            identity: TestIdentity,
            payments,
            registry: TestRegistry::default(),
            mg_connector: TestConnector {
                state: state.clone(),
                config: MgConfig::given_away(),
            },
            consensus_reader: TestConsensusReader::new(state),
            fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
        },
        test_peer_badge_verifier(),
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
        Some(crate::backup_worker::BackupSource {
            relays,
            required_reads,
            completed: staging_recovery(),
        }),
    )
    .await
    .expect("open restored FA")
}

/// Run the loop until `done` holds for the state, and return that state.
async fn run_until(
    client: &TestClient,
    within: Duration,
    done: impl FnMut(&FaState) -> bool,
) -> FaState {
    let mut observed = client.observe();
    tokio::select! {
        () = client.run(relay_options()) => unreachable!("the run loop does not return"),
        state = tokio::time::timeout(within, observed.wait_for(done)) => {
            state
                .unwrap_or_else(|_| panic!("state stayed {:?}", client.state()))
                .unwrap()
                .clone()
        }
    }
}

fn is_active(state: &FaState) -> bool {
    matches!(state, FaState::Active(_))
}

fn failed_lookup(state: &FaState) -> bool {
    matches!(
        state,
        FaState::Restoring(RecoveryCheck {
            last_error: Some(_)
        })
    )
}

fn lookup_error(client: &TestClient) -> FaErrorCode {
    match client.state() {
        FaState::Restoring(RecoveryCheck {
            last_error: Some(error),
        }) => error,
        state => panic!("expected a failed lookup, got {state:?}"),
    }
}

/// The restored FA governs exactly the source's federation and wrote nothing
/// else.
async fn assert_restored(restored: &TestClient, source: &TestClient, generation: u64) {
    let restored_federation = federation(&restored.status()).clone();
    let source_federation = federation(&source.status()).clone();
    assert_eq!(restored_federation.freshness, FormationFreshness::Fresh);
    assert_eq!(
        restored_federation.federation_invite,
        source_federation.federation_invite
    );
    assert_eq!(restored_federation.seats, source_federation.seats);
    assert_eq!(
        restored_federation.federation_name,
        source_federation.federation_name
    );
    let status = restored.status();
    assert!(status.creation.is_none() && status.purchase.is_none() && status.formation.is_none());
    assert!(
        restored
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await
    );
    // The imported payload becomes the last published generation, so the
    // restored FA never republishes an older one.
    let republished = restored.inner.store.backup_payload().await.unwrap();
    assert_eq!(republished.payload.snapshot_generation, generation);
}

async fn assert_nothing_written(client: &TestClient) {
    assert!(
        !client
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await
    );
    assert!(
        client
            .inner
            .store
            .load_federation(TestIdentity::fa_id())
            .await
            .unwrap()
            .is_none()
    );
    assert!(client.inner.store.backup_payload().await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_mnemonic_finds_its_published_backup_through_the_run_loop() {
    let relays = [
        LocalRelay::start().await,
        LocalRelay::start().await,
        LocalRelay::start().await,
    ];
    let urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let workers = publish_with_workers(&source, &urls).await;

    let database = MemDatabase::new().into_database();
    let restored = open_restored(database.clone(), state.clone(), urls.clone(), 2).await;
    assert_eq!(
        restored.state(),
        FaState::Restoring(RecoveryCheck::default())
    );
    run_until(&restored, Duration::from_secs(30), is_active).await;
    assert_restored(&restored, &source, 1).await;
    drop(restored);

    // Reopening skips the finished lookup; the stored federation is unsynced
    // until the loop rechecks it against consensus.
    let reopened = open_restored(database, state, urls, 2).await;
    let FaState::Active(federation) = reopened.state() else {
        panic!("a reopened restore is active, not {:?}", reopened.state());
    };
    assert_eq!(federation.freshness, FormationFreshness::Unsynced);
    run_until(&reopened, Duration::from_secs(30), |state| {
        matches!(state, FaState::Active(federation) if federation.freshness == FormationFreshness::Fresh)
    })
    .await;
    assert_restored(&reopened, &source, 1).await;
    workers.shutdown_join_all(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn no_backup_on_a_read_quorum_ends_the_lookup_idle() {
    // Two of three relays answer, which is the quorum.
    let relays = [LocalRelay::start().await, LocalRelay::start().await];
    let mut urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    urls.push(down_relay());
    let database = MemDatabase::new().into_database();
    let state = Arc::new(MgState::default());
    let restored = open_restored(database.clone(), state.clone(), urls.clone(), 2).await;
    run_until(&restored, Duration::from_secs(30), |state| {
        *state == FaState::Idle
    })
    .await;
    assert!(
        restored
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await
    );
    assert!(restored.claim_run_guard().await.is_ok());
    drop(restored);

    let reopened = open_restored(database, state, urls, 2).await;
    assert_eq!(reopened.state(), FaState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn too_few_relays_keep_restoring_and_the_loop_retries() {
    let answering = LocalRelay::start().await;
    let (first_down, second_down) = (free_port(), free_port());
    let urls = vec![
        answering.url.clone(),
        relay_url(first_down),
        relay_url(second_down),
    ];
    let restored = open_restored(
        MemDatabase::new().into_database(),
        Arc::new(MgState::default()),
        urls,
        2,
    )
    .await;
    let runner = restored.clone();
    let run = tokio::spawn(async move { runner.run(relay_options()).await });
    let mut observed = restored.observe();
    tokio::time::timeout(Duration::from_secs(30), observed.wait_for(failed_lookup))
        .await
        .expect("the lookup fails without a quorum")
        .unwrap();
    assert_eq!(lookup_error(&restored), FaErrorCode::Registry);
    assert_nothing_written(&restored).await;
    assert!(matches!(
        restored.claim_run_guard().await,
        Err(FaError::Busy)
    ));

    // The relays come back; the next retry completes the lookup.
    let _back = LocalRelay::try_start_on(first_down)
        .await
        .expect("the down relay's reserved port is free");
    tokio::time::timeout(
        Duration::from_secs(60),
        observed.wait_for(|state| *state == FaState::Idle),
    )
    .await
    .expect("a retry completes the lookup")
    .unwrap();
    assert!(
        restored
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await
    );
    run.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn one_answering_relay_with_a_backup_restores_without_a_quorum() {
    let answering = LocalRelay::start().await;
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let workers = publish_with_workers(&source, std::slice::from_ref(&answering.url)).await;
    let restored = open_restored(
        MemDatabase::new().into_database(),
        state,
        vec![answering.url.clone(), down_relay(), down_relay()],
        2,
    )
    .await;
    run_until(&restored, Duration::from_secs(30), is_active).await;
    assert_restored(&restored, &source, 1).await;
    workers.shutdown_join_all(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_newest_generation_across_relays_wins() {
    let relays = [
        LocalRelay::start().await,
        LocalRelay::start().await,
        LocalRelay::start().await,
    ];
    let urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let current = source.inner.store.backup_payload().await.unwrap().payload;
    let now = test_now_secs();
    // An older generation names a seat consensus does not hold. Were it
    // chosen, verification would fail and the lookup would never finish.
    let mut stale = current.clone();
    stale.snapshot_generation = 1;
    stale.seats[0].mg_identity = mg_keys(20).public_key();
    let mut newest = current.clone();
    newest.snapshot_generation = 3;
    publish(&urls[0], &sealed_event(&stale, now - 100)).await;
    publish(&urls[1], &sealed_event(&newest, now - 50)).await;
    // A newer event cannot outrank a higher generation on another relay.
    let mut middle = current;
    middle.snapshot_generation = 2;
    middle.seats[1].mg_identity = mg_keys(21).public_key();
    publish(&urls[2], &sealed_event(&middle, now)).await;

    let restored = open_restored(MemDatabase::new().into_database(), state, urls, 2).await;
    run_until(&restored, Duration::from_secs(30), is_active).await;
    assert_restored(&restored, &source, 3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_and_corrupt_events_are_ignored() {
    let relays = [
        LocalRelay::start().await,
        LocalRelay::start().await,
        LocalRelay::start().await,
    ];
    let urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let mut payload = source.inner.store.backup_payload().await.unwrap().payload;
    let now = test_now_secs();
    publish(&urls[0], &sealed_event(&payload, now - 100)).await;

    // Our own author with content we cannot open: garbage, a document sealed
    // for another FA, and a document from an unsupported schema.
    payload.snapshot_generation = 9;
    let other_keys = FaBackupKeys::derive(&fedimint_derive_secret::DerivableSecret::new_root(
        &[42; 64],
        b"another-fa",
    ));
    publish(&urls[1], &backup_event("not a sealed document", now)).await;
    publish(
        &urls[2],
        &backup_event(other_keys.seal(&payload).unwrap().content(), now),
    )
    .await;
    // Another author's well-formed event at the same coordinate.
    let foreign = EventBuilder::new(
        Kind::Custom(FA_BACKUP_EVENT_KIND),
        other_keys.seal(&payload).unwrap().content(),
    )
    .tags([Tag::identifier(FA_BACKUP_D_TAG)])
    .sign_with_keys(other_keys.author())
    .unwrap();
    publish(&urls[2], &foreign).await;

    let restored = open_restored(MemDatabase::new().into_database(), state, urls, 2).await;
    run_until(&restored, Duration::from_secs(30), is_active).await;
    assert_restored(&restored, &source, 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_that_consensus_rejects_writes_nothing_and_is_retried() {
    let relays = [
        LocalRelay::start().await,
        LocalRelay::start().await,
        LocalRelay::start().await,
    ];
    let urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let current = source.inner.store.backup_payload().await.unwrap().payload;
    let now = test_now_secs();
    let mut changed = current.clone();
    changed.snapshot_generation = 2;
    changed.seats[0].mg_identity = mg_keys(20).public_key();
    for url in &urls {
        publish(url, &sealed_event(&changed, now - 100)).await;
    }

    let restored = open_restored(MemDatabase::new().into_database(), state, urls.clone(), 2).await;
    let runner = restored.clone();
    let run = tokio::spawn(async move { runner.run(relay_options()).await });
    let mut observed = restored.observe();
    tokio::time::timeout(Duration::from_secs(30), observed.wait_for(failed_lookup))
        .await
        .expect("verification rejects the backup")
        .unwrap();
    assert_eq!(
        lookup_error(&restored),
        FaErrorCode::InvalidManifoldGuardians
    );
    assert_nothing_written(&restored).await;
    assert!(matches!(
        restored.cancel_creation().await,
        Err(FaError::Busy)
    ));

    // A corrected, newer generation reaches one relay; a retry imports it.
    let mut corrected = current;
    corrected.snapshot_generation = 3;
    publish(&urls[1], &sealed_event(&corrected, now)).await;
    tokio::time::timeout(Duration::from_secs(60), observed.wait_for(is_active))
        .await
        .expect("a retry imports the corrected backup")
        .unwrap();
    run.abort();
    assert_restored(&restored, &source, 3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn user_actions_are_busy_while_restoring() {
    // A relay that accepts connections and never answers holds the lookup.
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let silent_url = relay_url(silent.local_addr().unwrap().port());
    let (source, formation_id, state, connector) = formed_client_for_liquidity().await;
    let restored = open_restored(
        MemDatabase::new().into_database(),
        state,
        vec![silent_url],
        1,
    )
    .await;
    let runner = restored.clone();
    let run = tokio::spawn(async move { runner.run(relay_options()).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        restored.state(),
        FaState::Restoring(RecoveryCheck::default())
    );

    let preview = restored.preview_at(&intent(), locators());
    let maintenance = MaintenanceRunOptions::default();
    let results = [
        (
            "create_federation",
            restored
                .create_federation(None, None, &preview, None)
                .await
                .err(),
        ),
        ("cancel_creation", restored.cancel_creation().await.err()),
        (
            "propose_guardian_fees",
            restored
                .propose_guardian_fees(GuardianFeePpm::MANIFOLD_DEFAULT, options())
                .await
                .err(),
        ),
        ("restart_dkg", restored.restart_dkg(options()).await.err()),
        (
            "update_federation_metadata",
            restored
                .update_federation_metadata(
                    FederationMetadataUpdate::name("Renamed").unwrap(),
                    maintenance,
                )
                .await
                .err(),
        ),
        (
            "register_gateway",
            restored
                .register_gateway(
                    GatewayApiUrl::try_from("https://gateway.example/").unwrap(),
                    maintenance,
                )
                .await
                .err(),
        ),
        (
            "admitted_setup_payment_federations",
            restored
                .admitted_setup_payment_federations(options())
                .await
                .err(),
        ),
        (
            "start_liquidity",
            restored
                .start_liquidity(
                    &formation_id,
                    &liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string()),
                    LiquidityRequestIntent::gateway(250_000, Some(260_000)),
                    &connector,
                )
                .await
                .err(),
        ),
    ];
    for (action, error) in results {
        assert!(
            matches!(error, Some(FaError::Busy)),
            "{action} during a restore returned {error:?}"
        );
    }
    // Buying missing seats takes the run guard first, like every action.
    assert!(matches!(
        restored.buy_seats(&preview, None).await,
        Err(FaError::Busy)
    ));
    assert_eq!(
        restored.state(),
        FaState::Restoring(RecoveryCheck::default())
    );
    run.abort();
    drop(source);
}

/// A process stop at every write commit of a restore, before and after it
/// applies, and a stop while relays are still being read. Each reopened
/// process finishes the restore from what was stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_restore_stopped_at_any_commit_finishes_after_reopen() {
    let relays = [
        LocalRelay::start().await,
        LocalRelay::start().await,
        LocalRelay::start().await,
    ];
    let urls = relays
        .iter()
        .map(|relay| relay.url.clone())
        .collect::<Vec<_>>();
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let workers = publish_with_workers(&source, &urls).await;

    // Count the commits of one uninterrupted restore.
    let storage = CrashStorage::default();
    let plan = CrashPlan::never();
    let restored = open_restored(storage.open(plan.clone()), state.clone(), urls.clone(), 2).await;
    run_until(&restored, Duration::from_secs(30), is_active).await;
    drop(restored);
    let commits = plan.commits();
    assert!(commits >= 1, "a restore commits its import");

    for commit in 1..=commits {
        for point in [CrashPoint::BeforeCommit, CrashPoint::AfterCommit] {
            let storage = CrashStorage::default();
            let plan = CrashPlan::at(commit, point);
            let crashed =
                open_restored(storage.open(plan.clone()), state.clone(), urls.clone(), 2).await;
            tokio::select! {
                () = crashed.run(relay_options()) => unreachable!(),
                () = plan.wait() => {}
                () = tokio::time::sleep(Duration::from_secs(30)) => {
                    panic!("commit {commit} {point:?} was never reached");
                }
            }
            drop(crashed);

            let reopened = open_restored(
                storage.open(CrashPlan::never()),
                state.clone(),
                urls.clone(),
                2,
            )
            .await;
            run_until(&reopened, Duration::from_secs(30), |state| {
                matches!(state, FaState::Active(federation) if federation.freshness == FormationFreshness::Fresh)
            })
            .await;
            assert_restored(&reopened, &source, 1).await;
        }
    }

    // A stop while one relay still has not answered stores nothing.
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut with_silent = urls.clone();
    with_silent.push(relay_url(silent.local_addr().unwrap().port()));
    let database = MemDatabase::new().into_database();
    let stopped = open_restored(database.clone(), state.clone(), with_silent.clone(), 3).await;
    tokio::select! {
        () = stopped.run(relay_options()) => unreachable!(),
        () = tokio::time::sleep(Duration::from_secs(1)) => {}
    }
    assert_eq!(
        stopped.state(),
        FaState::Restoring(RecoveryCheck::default())
    );
    drop(stopped);
    drop(silent);
    let reopened = open_restored(database, state, with_silent, 3).await;
    assert_eq!(
        reopened.state(),
        FaState::Restoring(RecoveryCheck::default())
    );
    assert_nothing_written(&reopened).await;
    run_until(&reopened, Duration::from_secs(30), is_active).await;
    assert_restored(&reopened, &source, 1).await;
    workers.shutdown_join_all(None).await.unwrap();
}

/// A relay whose port another process holds is started on another port,
/// rather than handing out the other process's listener.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_never_reuses_a_port_another_process_holds() {
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    assert!(LocalRelay::try_start_on(port).await.is_none());
    let relay = LocalRelay::start().await;
    assert!(listens_on(
        relay.child.id(),
        relay
            .url
            .as_str()
            .rsplit(':')
            .next()
            .unwrap()
            .trim_end_matches('/')
            .parse()
            .unwrap()
    ));
}
