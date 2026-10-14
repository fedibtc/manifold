//! QA harness for the schema 11 → 12 storage migration: replays database
//! states recorded from schema 11 releases.

use std::path::{Path, PathBuf};

use fedimint_core::db::mem_impl::MemTransaction;
use fedimint_core::db::{
    Database, DatabaseError, DatabaseResult, IDatabaseTransactionOps, IDatabaseTransactionOpsCore,
    IRawDatabase, IRawDatabaseTransaction, PrefixStream,
};
use futures::StreamExt as _;

use super::*;

/// A shared in-memory database whose `fail_commit`th commit fails, as a
/// crash just before that commit would.
#[derive(Debug)]
pub(super) struct CrashingDatabase {
    inner: Arc<MemDatabase>,
    fail_commit: usize,
    commits: AtomicUsize,
}

impl CrashingDatabase {
    pub(super) fn open(inner: &Arc<MemDatabase>, fail_commit: Option<usize>) -> Database {
        Self {
            inner: inner.clone(),
            fail_commit: fail_commit.unwrap_or(usize::MAX),
            commits: AtomicUsize::new(0),
        }
        .into_database()
    }
}

#[async_trait::async_trait]
impl IRawDatabase for CrashingDatabase {
    type Transaction<'a> = CrashingTransaction<'a>;

    async fn begin_transaction<'a>(&'a self) -> CrashingTransaction<'a> {
        CrashingTransaction {
            inner: self.inner.begin_transaction().await,
            db: self,
        }
    }

    fn checkpoint(&self, _backup_path: &Path) -> DatabaseResult<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct CrashingTransaction<'a> {
    inner: MemTransaction<'a>,
    db: &'a CrashingDatabase,
}

#[async_trait::async_trait]
impl IDatabaseTransactionOpsCore for CrashingTransaction<'_> {
    async fn raw_insert_bytes(
        &mut self,
        key: &[u8],
        value: &[u8],
    ) -> DatabaseResult<Option<Vec<u8>>> {
        self.inner.raw_insert_bytes(key, value).await
    }

    async fn raw_get_bytes(&mut self, key: &[u8]) -> DatabaseResult<Option<Vec<u8>>> {
        self.inner.raw_get_bytes(key).await
    }

    async fn raw_remove_entry(&mut self, key: &[u8]) -> DatabaseResult<Option<Vec<u8>>> {
        self.inner.raw_remove_entry(key).await
    }

    async fn raw_remove_by_prefix(&mut self, key_prefix: &[u8]) -> DatabaseResult<()> {
        self.inner.raw_remove_by_prefix(key_prefix).await
    }

    async fn raw_find_by_prefix(&mut self, key_prefix: &[u8]) -> DatabaseResult<PrefixStream<'_>> {
        self.inner.raw_find_by_prefix(key_prefix).await
    }

    async fn raw_find_by_prefix_sorted_descending(
        &mut self,
        key_prefix: &[u8],
    ) -> DatabaseResult<PrefixStream<'_>> {
        self.inner
            .raw_find_by_prefix_sorted_descending(key_prefix)
            .await
    }

    async fn raw_find_by_range(
        &mut self,
        range: std::ops::Range<&[u8]>,
    ) -> DatabaseResult<PrefixStream<'_>> {
        self.inner.raw_find_by_range(range).await
    }
}

impl IDatabaseTransactionOps for CrashingTransaction<'_> {}

#[async_trait::async_trait]
impl IRawDatabaseTransaction for CrashingTransaction<'_> {
    async fn commit_tx(self) -> DatabaseResult<()> {
        let commit = self.db.commits.fetch_add(1, Ordering::SeqCst) + 1;
        if commit == self.db.fail_commit {
            return Err(DatabaseError::WriteConflict);
        }
        self.inner.commit_tx().await
    }
}

pub(super) type Rows = Vec<(Vec<u8>, Vec<u8>)>;

pub(super) async fn install_rows(database: &Database, rows: &Rows) {
    let mut dbtx = database.begin_transaction().await;
    for (key, value) in rows {
        dbtx.raw_insert_bytes(key, value).await.unwrap();
    }
    dbtx.commit_tx().await;
}

pub(super) async fn dump_rows(database: &Database) -> Rows {
    database
        .begin_transaction_nc()
        .await
        .raw_find_by_prefix(&[])
        .await
        .unwrap()
        .collect()
        .await
}

fn hex_rows(rows: &Rows) -> serde_json::Value {
    rows.iter()
        .map(|(k, v)| serde_json::json!([hex::encode(k), hex::encode(v)]))
        .collect()
}

fn quote_id(hex_id: &serde_json::Value) -> QuoteId {
    QuoteId(
        hex::decode(hex_id.as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    )
}

fn quote_set(value: &serde_json::Value) -> HashSet<QuoteId> {
    value
        .as_array()
        .map(|quotes| quotes.iter().map(quote_id).collect())
        .unwrap_or_default()
}

/// The test wallet as the old release left it.
fn seeded_payments(probe: &serde_json::Value) -> (TestPayments, Arc<PaymentState>) {
    let (payments, state) = TestPayments::new();
    if probe.is_object() {
        *state.funded_quotes.lock().unwrap() = quote_set(&probe["funded_quotes"]);
        *state.settled_quotes.lock().unwrap() = quote_set(&probe["settled_quotes"]);
        *state.rejected_quotes.lock().unwrap() = quote_set(&probe["rejected_quotes"]);
        *state.released_quotes.lock().unwrap() = quote_set(&probe["released_quotes"]);
        if let Some(reservations) = probe["reservations"].as_object() {
            let mut stored = state.reservations.lock().unwrap();
            for (id, reservation) in reservations {
                stored.insert(
                    id.clone(),
                    TestReservation {
                        quote_ids: reservation["quote_ids"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(quote_id)
                            .collect(),
                        started: quote_set(&reservation["started"]),
                        terminal: quote_set(&reservation["terminal"]),
                        released: quote_set(&reservation["released"]),
                    },
                );
            }
        }
    }
    (payments, state)
}

/// The test MGs and federation as the old release left them.
fn seeded_mgs(probe: &serde_json::Value) -> Arc<MgState> {
    let state = Arc::new(MgState::default());
    if let Some(raw) = probe["meta_consensus_raw"].as_str() {
        *state.meta_consensus_raw.lock().unwrap() = Some(hex::decode(raw).unwrap());
    }
    if let Some(revision) = probe["meta_consensus_revision"].as_u64() {
        state
            .meta_consensus_revision
            .store(revision, Ordering::SeqCst);
    }
    state
}

pub(super) struct Snapshot {
    pub(super) path: PathBuf,
    pub(super) rows: Rows,
    pub(super) probes: serde_json::Value,
}

pub(super) fn read_snapshot(path: &Path) -> Snapshot {
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let rows = snapshot["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                hex::decode(row[0].as_str().unwrap()).unwrap(),
                hex::decode(row[1].as_str().unwrap()).unwrap(),
            )
        })
        .collect();
    Snapshot {
        path: path.to_owned(),
        rows,
        probes: snapshot["probes"].clone(),
    }
}

fn row_json(rows: &Rows, key: &[u8]) -> Option<serde_json::Value> {
    rows.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| serde_json::from_slice(v).ok())
}

fn owner(rows: &Rows) -> Option<FaId> {
    [[0x00u8], [0x07u8]].iter().find_map(|key| {
        row_json(rows, key).and_then(|row| serde_json::from_value(row["fi_id"].clone()).ok())
    })
}

fn status_summary(status: &FaStatus) -> serde_json::Value {
    serde_json::json!({
        "creation": status.creation.as_ref().map(|c| serde_json::json!({
            "name": c.federation_name.0,
            "size": c.federation_size.0,
        })),
        "purchase": status.purchase.as_ref().map(|p| serde_json::json!({
            "id": p.purchase_id.0,
            "phase": format!("{:?}", p.phase),
            "outputs_started": p.payment_outputs_started,
            "seats": p.seats.iter().map(|s| serde_json::json!({
                "index": s.index,
                "mg": s.mg_id.to_hex(),
                "seat_id": s.seat_id.as_ref().map(ToString::to_string),
                "phase": format!("{:?}", s.phase),
            })).collect::<Vec<_>>(),
        })),
        "pool": status.pool.iter().map(|s| s.seat_id.to_string()).collect::<Vec<_>>(),
        "formation": status.formation.as_ref().map(|f| serde_json::json!({
            "id": f.formation_id.0,
            "phase": format!("{:?}", f.phase),
            "invite": f.invite_code.as_ref().map(|i| i.0.clone()),
            "codes": f.seats.iter().filter(|s| s.guardian_code.is_some()).count(),
        })),
        "federation": status.federation.as_ref().map(|f| serde_json::json!({
            "id": f.formation_id.0,
            "invite": f.federation_invite.0.clone(),
            "name": f.federation_name.as_ref().map(|n| n.0.clone()),
            "seats": f.seats.iter().map(|s| s.seat_id.to_string()).collect::<Vec<_>>(),
            "freshness": format!("{:?}", f.freshness),
        })),
        "reauthorization": status.reauthorization.map(|r| format!("{r:?}")),
        "recovery": status.recovery.is_some(),
    })
}

async fn migrate_and_load(database: &Database, fa_id: FaId) -> FaResult<FaStatus> {
    let store = crate::db::FaStore::new(database.clone());
    store.migrate_storage().await?;
    store.load_status(fa_id).await
}

/// Migrate one recorded state, interrupt the migration at every commit, and
/// drive the result with the test harness.
pub(super) async fn check_snapshot(snapshot: &Snapshot, drive: bool) -> serde_json::Value {
    let fa_id = owner(&snapshot.rows).unwrap_or_else(TestIdentity::fa_id);
    let mem = Arc::new(MemDatabase::new());
    let database = CrashingDatabase::open(&mem, None);
    install_rows(&database, &snapshot.rows).await;
    let first = migrate_and_load(&database, fa_id).await;
    let migrated = dump_rows(&database).await;
    let second = migrate_and_load(&database, fa_id).await;
    let idempotent = dump_rows(&database).await == migrated
        && first
            .as_ref()
            .map(status_summary)
            .map_err(ToString::to_string)
            == second
                .as_ref()
                .map(status_summary)
                .map_err(ToString::to_string);

    let mut crashes = Vec::new();
    for fail_commit in 1..=3 {
        let mem = Arc::new(MemDatabase::new());
        install_rows(&CrashingDatabase::open(&mem, None), &snapshot.rows).await;
        let interrupted = crate::db::FaStore::new(CrashingDatabase::open(&mem, Some(fail_commit)))
            .migrate_storage()
            .await;
        let healthy = CrashingDatabase::open(&mem, None);
        let resumed = migrate_and_load(&healthy, fa_id).await;
        crashes.push(serde_json::json!({
            "fail_commit": fail_commit,
            "interrupted": interrupted.as_ref().err().map(ToString::to_string),
            "resumed": resumed.as_ref().err().map(ToString::to_string),
            "same": dump_rows(&healthy).await == migrated
                && resumed.as_ref().map(status_summary).map_err(ToString::to_string)
                    == first.as_ref().map(status_summary).map_err(ToString::to_string),
        }));
        if interrupted.is_ok() {
            break;
        }
    }

    let mut report = serde_json::json!({
        "file": snapshot.path.to_string_lossy(),
        "open": first.as_ref().err().map(ToString::to_string),
        "status": first.as_ref().ok().map(status_summary),
        "rows_after": hex_rows(&migrated),
        "idempotent": idempotent,
        "crashes": crashes,
    });
    if drive && first.is_ok() {
        if fa_id == TestIdentity::fa_id() {
            report["drive"] = drive_migrated(snapshot, &mem, TestIdentity).await;
        } else if fa_id == LegacyIdentity.public_key().unwrap() {
            report["drive"] = drive_migrated(snapshot, &mem, LegacyIdentity).await;
        }
    }
    report
}

/// The test identity of the `fi-client` releases.
#[derive(Clone, Copy)]
pub(super) struct LegacyIdentity;

impl FaIdentity for LegacyIdentity {
    fn scoped_root(&self) -> fedimint_derive_secret::DerivableSecret {
        fedimint_derive_secret::DerivableSecret::new_root(&[7; 64], b"fi-client-test-root")
    }
}

pub(super) async fn open_as<I: FaIdentity>(
    identity: I,
    database: Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
) -> FaResult<FaClient<I, TestPayments, TestRegistry, TestConnector, TestConsensusReader>> {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    FaClient::open_inner(
        database,
        FaClientPorts {
            identity,
            payments,
            registry,
            mg_connector: TestConnector {
                state: state.clone(),
                config,
            },
            consensus_reader: TestConsensusReader::new(state),
            fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
        },
        test_peer_badge_verifier(),
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
        None,
    )
    .await
}

pub(super) type SnapshotClient<I> =
    FaClient<I, TestPayments, TestRegistry, TestConnector, TestConsensusReader>;

/// Open a migrated state with test MGs and the old release's wallet and
/// federation.
pub(super) async fn open_snapshot<I: FaIdentity>(
    snapshot: &Snapshot,
    mem: &Arc<MemDatabase>,
    identity: I,
) -> FaResult<(SnapshotClient<I>, Arc<PaymentState>, Arc<MgState>)> {
    let header = row_json(&snapshot.rows, &[0x00]);
    let paid = header.as_ref().is_some_and(|header| {
        !header["creation_mode"]["selected"]["payment_federation_id"].is_null()
    });
    let mut config = if paid {
        MgConfig::paid()
    } else {
        MgConfig::given_away()
    };
    if let Some(size) = header
        .as_ref()
        .and_then(|header| header["intent"]["federation_size"].as_u64())
    {
        config.federation_size = FederationSize(u16::try_from(size).unwrap());
    }
    let (payments, wallet) = seeded_payments(&snapshot.probes["wallet0"]);
    let mgs = seeded_mgs(&snapshot.probes["mg0"]);
    let client = open_as(
        identity,
        CrashingDatabase::open(mem, None),
        payments,
        mgs.clone(),
        config,
    )
    .await?;
    Ok((client, wallet, mgs))
}

/// Run the migrated state with test MGs and the old release's wallet.
async fn drive_migrated<I: FaIdentity>(
    snapshot: &Snapshot,
    mem: &Arc<MemDatabase>,
    identity: I,
) -> serde_json::Value {
    // A crashed driver's lease only delays the resume.
    {
        let database = CrashingDatabase::open(mem, None);
        let mut dbtx = database.begin_transaction().await;
        dbtx.raw_remove_entry(&[0x02]).await.unwrap();
        dbtx.commit_tx().await;
    }
    let (client, wallet, mgs) = match open_snapshot(snapshot, mem, identity).await {
        Ok(opened) => opened,
        Err(error) => return serde_json::json!({"open": error.to_string()}),
    };
    let old_funded = wallet.funded_quotes.lock().unwrap().clone();
    let mut runs = Vec::new();
    for _ in 0..6 {
        let result = client.drive_all_for_test(options()).await;
        runs.push(result.as_ref().err().map(|e| format!("{e:?}")));
        let status = client.status();
        let needs_seats = status.creation.is_some()
            && status.purchase.is_none()
            && status.formation.is_none();
        if needs_seats {
            let rebuy = rebuy_missing_seats(&client).await;
            runs.push(Some(format!("rebuy: {:?}", rebuy.as_ref().err())));
            continue;
        }
        if result.is_ok() {
            break;
        }
    }
    let status = client.status();
    let funded = wallet.funded_quotes.lock().unwrap().clone();
    serde_json::json!({
        "runs": runs,
        "status": status_summary(&status),
        "old_funded": qa_hex_quotes(&old_funded),
        "new_funded": qa_hex_quotes(&funded.difference(&old_funded).copied().collect()),
        "settled": qa_hex_quotes(&wallet.settled_quotes.lock().unwrap()),
        "released": qa_hex_quotes(&wallet.released_quotes.lock().unwrap()),
        "create_records": mgs.create_records.lock().unwrap().iter().map(|r| serde_json::json!({
            "quote_id": hex::encode(r.quote_id.0),
            "seat_id": r.seat_id.to_string(),
        })).collect::<Vec<_>>(),
        "start_callbacks": mgs.start_callbacks.lock().unwrap().iter().map(|c| c.is_some()).collect::<Vec<_>>(),
        "restart_calls": mgs.restart_calls.load(Ordering::SeqCst),
        "rows_final": hex_rows(&dump_rows(&CrashingDatabase::open(mem, None)).await),
    })
}

/// Buy the creation's missing seats from test MGs it does not use yet, as a
/// user answering a reauthorization would.
async fn rebuy_missing_seats<I: FaIdentity>(client: &SnapshotClient<I>) -> FaResult<()> {
    let fa_id = client.fa_id()?;
    let store = &client.inner.store;
    let creation = store
        .load_creation(fa_id)
        .await?
        .ok_or(FaError::NoActiveFormation)?;
    let bought = store.bought_seats().await;
    let bought_count = u16::try_from(store.pool_seats().await.len()).unwrap();
    let missing = usize::from(creation.federation_size.0 - bought_count);
    let request = MgSelectionRequest::new(
        creation.federation_size,
        creation.fedimintd_versions,
        creation.plan,
    )?
    .missing_seats(bought.clone(), bought_count);
    let price_msats = client.inner.ports.mg_connector.config.price_msats;
    let seats = (0..usize::from(MAX_FEDERATION_SIZE))
        .filter(|index| !bought.mg_ids.contains(&test_mg_id(*index)))
        .take(missing)
        .map(|index| (index, price_msats))
        .collect::<Vec<_>>();
    client
        .buy_seats(
            &preview_of(request, seats),
            (price_msats > 0).then(payment_federation_id),
        )
        .await
}

fn qa_hex_quotes(quotes: &HashSet<QuoteId>) -> Vec<String> {
    let mut quotes = quotes.iter().map(|q| hex::encode(q.0)).collect::<Vec<_>>();
    quotes.sort();
    quotes
}

/// Replays every snapshot under `QA_CORPUS` into `QA_REPORT` (JSON lines).
#[tokio::test]
async fn qa_corpus() {
    let (Ok(corpus), Ok(report)) = (std::env::var("QA_CORPUS"), std::env::var("QA_REPORT")) else {
        return;
    };
    let drive = std::env::var("QA_DRIVE").is_ok();
    let mut files = Vec::new();
    for dir in corpus.split(',') {
        for test_dir in std::fs::read_dir(dir).unwrap() {
            for file in std::fs::read_dir(test_dir.unwrap().path()).unwrap() {
                let path = file.unwrap().path();
                let name = path.to_string_lossy();
                if name.ends_with(".json")
                    && !name.ends_with(".oracle.json")
                    && !name.ends_with(".drive.json")
                {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    let reports = futures::stream::iter(files)
        .map(|path| async move {
            let snapshot = read_snapshot(&path);
            match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check_snapshot(
                &snapshot, drive,
            )))
            .await
            {
                Ok(report) => report,
                Err(panic) => serde_json::json!({
                    "file": path.to_string_lossy(),
                    "panic": panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned())),
                }),
            }
        })
        .buffer_unordered(32)
        .collect::<Vec<_>>()
        .await;
    let mut out = Vec::new();
    for line in reports {
        out.extend(serde_json::to_vec(&line).unwrap());
        out.push(b'\n');
    }
    std::fs::write(report, out).unwrap();
}

/// A recorded schema 11 state committed under `db/fixtures/qa`.
pub(super) fn fixture(json: &str) -> Snapshot {
    let fixture: serde_json::Value = serde_json::from_str(json).unwrap();
    let rows = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let key = hex::decode(row[0].as_str().unwrap()).unwrap();
            let value = match row[1].get("raw_hex") {
                Some(raw) => hex::decode(raw.as_str().unwrap()).unwrap(),
                None => serde_json::to_vec(&row[1]).unwrap(),
            };
            (key, value)
        })
        .collect();
    Snapshot {
        path: PathBuf::from(fixture["source"].as_str().unwrap()),
        rows,
        probes: serde_json::json!({"wallet0": fixture["wallet"], "mg0": fixture["mg"]}),
    }
}

/// Install a fixture and open it as the `fi-client` test identity.
async fn open_fixture(
    json: &str,
) -> (
    SnapshotClient<LegacyIdentity>,
    Arc<MemDatabase>,
    Arc<PaymentState>,
    Arc<MgState>,
) {
    let snapshot = fixture(json);
    let mem = Arc::new(MemDatabase::new());
    install_rows(&CrashingDatabase::open(&mem, None), &snapshot.rows).await;
    let (client, wallet, mgs) = open_snapshot(&snapshot, &mem, LegacyIdentity)
        .await
        .expect("the migrated fixture opens");
    (client, mem, wallet, mgs)
}

async fn reload(mem: &Arc<MemDatabase>) -> FaResult<FaStatus> {
    crate::db::FaStore::new(CrashingDatabase::open(mem, None))
        .load_status(LegacyIdentity.public_key().unwrap())
        .await
}

const R269_PENDING_REPLACEMENT: &str =
    include_str!("../db/fixtures/qa/r269_pending_replacement_unauthorized_siblings.json");
const R269_INTERRUPTED_REPLACEMENT_RESTORE: &str =
    include_str!("../db/fixtures/qa/r269_interrupted_replacement_restore.json");
const R269_APPLIED_REPLACEMENT: &str =
    include_str!("../db/fixtures/qa/r269_applied_replacement_awaiting_quote.json");

/// The effect-bound quote ids of the fixture's seats that schema 11 kept,
/// that is, every seat that does not wait for a replacement.
fn kept_bound_quotes(json: &str) -> HashSet<QuoteId> {
    let fixture: serde_json::Value = serde_json::from_str(json).unwrap();
    fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row[0].as_str().unwrap().starts_with("01"))
        .filter(|row| row[1]["replacement_for"].is_null())
        .filter_map(|row| {
            let state = &row[1]["admission"]["peer_badge"]["state"]["effect_authorized"];
            (!state.is_null()).then(|| serde_json::from_value(state["quote_id"].clone()).unwrap())
        })
        .collect()
}

/// Seat 0 ended after payment outputs started and waits for a replacement;
/// its six siblings hold effect-authorized quotes the cleared aggregate no
/// longer names. The run pays exactly those quotes, the six seats move to
/// the pool, and the creation needs one seat.
#[tokio::test]
async fn r269_pending_replacement_buys_its_bound_siblings() {
    let (client, mem, wallet, _) = open_fixture(R269_PENDING_REPLACEMENT).await;
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(status.purchase.is_none());
    assert_eq!(status.pool.len(), 6);
    let bound = kept_bound_quotes(R269_PENDING_REPLACEMENT);
    assert_eq!(bound.len(), 6);
    assert_eq!(*wallet.funded_quotes.lock().unwrap(), bound);
    reload(&mem).await.expect("the FA still loads after a run");
}

/// A provisional replacement restore was interrupted after the wallet
/// released its reservation: the release intent remains beside six paid
/// seats. The run finishes the restore: the replacement leaves, the six
/// seats move to the pool, and the creation needs one seat.
#[tokio::test]
async fn r269_interrupted_replacement_restore_does_not_strand_paid_seats() {
    let (client, mem, _, _) = open_fixture(R269_INTERRUPTED_REPLACEMENT_RESTORE).await;
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(status.purchase.is_none());
    assert_eq!(status.pool.len(), 6);
    reload(&mem).await.expect("the FA still loads after a run");
}

/// An applied replacement MG still needs its quote after payment outputs
/// started, and its preview has long expired. The replacement leaves, the
/// six paid seats move to the pool, and the creation needs one seat.
#[tokio::test]
async fn r269_applied_replacement_with_an_expired_preview_can_still_finish() {
    let (client, mem, _, _) = open_fixture(R269_APPLIED_REPLACEMENT).await;
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(status.purchase.is_none());
    assert_eq!(status.pool.len(), 6);
    reload(&mem).await.expect("the FA still loads after a run");
}
