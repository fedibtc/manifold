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
    }
    (payments, state)
}

/// The test MGs and federation as the old release left them.
fn seeded_mgs(probe: &serde_json::Value) -> Arc<MgState> {
    let state = Arc::new(MgState::default());
    state.quote_calls.store(1000, Ordering::SeqCst);
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
            "id": c.formation_id.0,
            "name": c.federation_name.0,
            "size": c.federation_size.0,
            "stage": format!("{:?}", c.stage),
            "slots": c.slots.iter().map(|s| serde_json::json!({
                "index": s.index,
                "mg": s.mg_id.to_hex(),
                "seat_id": s.seat_id.as_ref().map(ToString::to_string),
                "phase": format!("{:?}", s.phase),
                "code": s.guardian_code.is_some(),
            })).collect::<Vec<_>>(),
        })),
        "federation": status.federation.as_ref().map(|f| serde_json::json!({
            "id": f.formation_id.0,
            "invite": f.federation_invite.0.clone(),
            "name": f.federation_name.as_ref().map(|n| n.0.clone()),
            "seats": f.seats.iter().map(|s| s.seat_id.to_string()).collect::<Vec<_>>(),
        })),
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
    if snapshot.path.to_string_lossy().contains("name_sensitive") {
        mgs.name_sensitive_codes.store(true, Ordering::SeqCst);
    }
    let old_funded = wallet.funded_quotes.lock().unwrap().clone();
    let mut runs = Vec::new();
    for _ in 0..6 {
        let result = client.drive_all_for_test(options()).await;
        runs.push(result.as_ref().err().map(|e| format!("{e:?}")));
        let status = client.status();
        let needs_seats = status.creation.is_some() && !is_buying(&status) && !is_forming(&status);
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
        "payment_calls": wallet.created_quotes.lock().unwrap().iter().map(|q| hex::encode(q.0)).collect::<Vec<_>>(),
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
    let request = store.missing_seats_request(fa_id).await?;
    let bought = request.bought.clone();
    let missing = usize::from(request.seat_count());
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
const R269_NAMED_SEAT_NOT_LOWEST: &str =
    include_str!("../db/fixtures/qa/r269_dkg_preparation_named_seat_not_lowest.json");
const R269_NAMED_CODE_CACHED: &str =
    include_str!("../db/fixtures/qa/r269_dkg_preparation_named_code_cached.json");
const R269_UNVERIFIED_RESTORE: &str =
    include_str!("../db/fixtures/qa/r269_unverified_restored_backup.json");
const R269_PUBLISHING_SEAT_BINDINGS: &str =
    include_str!("../db/fixtures/qa/r269_publishing_seat_bindings.json");
const R269_BOUND_REPLACEMENT_PAID: &str =
    include_str!("../db/fixtures/qa/r269_bound_replacement_payment_started.json");
const R269_MIXED_REPLACEMENT_WAVE: &str =
    include_str!("../db/fixtures/qa/r269_mixed_replacement_wave_live_reservation.json");

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
/// its six siblings are bound to quotes whose payments never started. The
/// run pays the six seats, they move to the pool, and the creation needs one
/// seat.
#[tokio::test]
async fn r269_pending_replacement_buys_its_bound_siblings() {
    let (client, mem, wallet, _) = open_fixture(R269_PENDING_REPLACEMENT).await;
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(!is_buying(&status));
    assert_eq!(accepted_seats(&status).len(), 6);
    let bound = kept_bound_quotes(R269_PENDING_REPLACEMENT);
    assert_eq!(bound.len(), 6);
    // No bound payment started, so each seat pays a fresh quote.
    let funded = wallet.funded_quotes.lock().unwrap().clone();
    assert_eq!(funded.len(), 6);
    assert!(funded.is_disjoint(&bound));
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
    assert!(!is_buying(&status));
    assert_eq!(accepted_seats(&status).len(), 6);
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
    assert!(!is_buying(&status));
    assert_eq!(accepted_seats(&status).len(), 6);
    reload(&mem).await.expect("the FA still loads after a run");
}

/// Two applied replacements wait after payment outputs started, and their
/// previews have long expired. The payer wallet released their hold before
/// the FA opened. Both replacements leave without a payment, the five paid
/// seats move to the pool, and the creation needs two seats.
#[tokio::test]
async fn r269_mixed_replacement_wave_drops_both_replacements() {
    let (client, mem, wallet, _) = open_fixture(R269_MIXED_REPLACEMENT_WAVE).await;
    let funded = wallet.funded_quotes.lock().unwrap().clone();
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(!is_buying(&status));
    assert_eq!(accepted_seats(&status).len(), 5);
    assert_eq!(*wallet.funded_quotes.lock().unwrap(), funded);
    reload(&mem).await.expect("the FA still loads after a run");
}

/// All seven seats exist and no guardian code is cached. Seat index 0 is on
/// MG 0, but MG 3 holds the lowest seat id. MGs whose code depends on the
/// federation name must be asked for the named code at MG 0, as schema 11
/// would have, and the federation forms.
#[tokio::test]
async fn r269_dkg_preparation_asks_the_index_zero_seat_for_the_named_code() {
    let (client, _, _, mgs) = open_fixture(R269_NAMED_SEAT_NOT_LOWEST).await;
    mgs.name_sensitive_codes.store(true, Ordering::SeqCst);
    client.drive_all_for_test(options()).await.unwrap();
    assert!(client.status().federation.is_some());
    assert_eq!(*mgs.named_dkg_code_indices.lock().unwrap(), vec![0]);
}

/// Schema 11 cached the named code of MG 0 and the plain code of MG 1. The
/// cached codes still match, so the run asks no MG for a named code again
/// and the federation forms.
#[tokio::test]
async fn r269_dkg_preparation_keeps_a_cached_named_code() {
    let (client, _, _, mgs) = open_fixture(R269_NAMED_CODE_CACHED).await;
    mgs.name_sensitive_codes.store(true, Ordering::SeqCst);
    client.drive_all_for_test(options()).await.unwrap();
    assert!(client.status().federation.is_some());
    assert!(
        mgs.named_dkg_code_indices
            .lock()
            .unwrap()
            .iter()
            .all(|index| *index == 0)
    );
}

/// Schema 11 kept a restored backup it had not verified yet. The migration
/// keeps it, and the run verifies and imports it with no relay lookup,
/// through the other saved MGs while the peers of its saved invite are down.
#[tokio::test]
async fn r269_unverified_restore_is_verified_and_imported_by_the_run() {
    let snapshot = fixture(R269_UNVERIFIED_RESTORE);
    let restored = row_json(&snapshot.rows, &[0x07]).unwrap();
    assert_eq!(restored["backup_eligible"], false);
    let (client, mem, _, _) = open_fixture(R269_UNVERIFIED_RESTORE).await;
    assert!(client.status().recovery.is_some());
    assert!(client.status().federation.is_none());
    client
        .inner
        .ports
        .consensus_reader
        .offline_peers
        .lock()
        .unwrap()
        .extend((0..MIN_FEDERATION_SIZE - 1).map(PeerId::from));

    let run = tokio::time::timeout(Duration::from_secs(2), client.run_with(options())).await;
    assert!(run.is_err());
    let status = client.status();
    assert!(status.recovery.is_none(), "{status:?}");
    assert!(status.federation.is_some());
    let fa_id = LegacyIdentity.public_key().unwrap();
    assert!(
        client
            .inner
            .store
            .unverified_restored_backup(fa_id)
            .await
            .is_none()
    );
    assert!(reload(&mem).await.unwrap().federation.is_some());
}

/// While consensus cannot verify the kept backup, it stays, and the FA
/// stays in recovery.
#[tokio::test]
async fn r269_unverified_restore_stays_while_consensus_is_unreachable() {
    let (client, _, _, _) = open_fixture(R269_UNVERIFIED_RESTORE).await;
    client.inner.ports.consensus_reader.fail_next(usize::MAX);

    let run = tokio::time::timeout(Duration::from_secs(1), client.run_with(options())).await;
    assert!(run.is_err());
    let status = client.status();
    assert!(status.recovery.is_some(), "{status:?}");
    assert!(status.federation.is_none());
    let fa_id = LegacyIdentity.public_key().unwrap();
    assert!(
        client
            .inner
            .store
            .unverified_restored_backup(fa_id)
            .await
            .is_some()
    );
}

/// The fixture's metadata target under the field name it had before Manifold
/// renamed the guardian-fee field.
fn with_old_fee_field_name(rows: &mut Rows) {
    let (_, header) = rows.iter_mut().find(|(key, _)| key == &[0x00]).unwrap();
    let mut formation: serde_json::Value = serde_json::from_slice(header).unwrap();
    let target = formation["formation_meta_target"].as_object_mut().unwrap();
    let account = target.remove("guardian_verification_fee_account").unwrap();
    target.insert("fedi_fee_account".to_owned(), account);
    *header = serde_json::to_vec(&formation).unwrap();
}

/// The Fedi bridge's `migrate_fi_guardian_fee_field`, which runs before the
/// FA client opens.
fn bridge_fee_field_rewrite(rows: &mut Rows) {
    let (_, header) = rows.iter_mut().find(|(key, _)| key == &[0x00]).unwrap();
    let mut formation: serde_json::Value = serde_json::from_slice(header).unwrap();
    let target = formation["formation_meta_target"].as_object_mut().unwrap();
    if !target.contains_key("guardian_verification_fee_account")
        && let Some(account) = target.remove("fedi_fee_account")
    {
        target.insert("guardian_verification_fee_account".to_owned(), account);
    }
    *header = serde_json::to_vec(&formation).unwrap();
}

/// A metadata target under the old guardian-fee field name migrates like the
/// current one once the bridge has rewritten it, and the formation finishes.
/// Without the rewrite the FA does not open and its rows stay as they were.
#[tokio::test]
async fn r269_old_guardian_fee_field_name_migrates_after_the_bridge_rewrite() {
    let original = fixture(R269_PUBLISHING_SEAT_BINDINGS);
    let expected = Arc::new(MemDatabase::new());
    install_rows(&CrashingDatabase::open(&expected, None), &original.rows).await;
    migrate_and_load(
        &CrashingDatabase::open(&expected, None),
        owner(&original.rows).unwrap(),
    )
    .await
    .unwrap();

    let mut renamed = original.rows.clone();
    with_old_fee_field_name(&mut renamed);
    let unrewritten = Arc::new(MemDatabase::new());
    install_rows(&CrashingDatabase::open(&unrewritten, None), &renamed).await;
    let error = open_snapshot(&original, &unrewritten, LegacyIdentity)
        .await
        .err()
        .expect("the old field name does not migrate");
    assert!(matches!(error, FaError::Storage(_)), "{error:?}");
    assert_eq!(
        dump_rows(&CrashingDatabase::open(&unrewritten, None)).await,
        {
            let mut rows = renamed.clone();
            rows.sort();
            rows
        }
    );

    bridge_fee_field_rewrite(&mut renamed);
    let rewritten = Arc::new(MemDatabase::new());
    install_rows(&CrashingDatabase::open(&rewritten, None), &renamed).await;
    let (client, _, _) = open_snapshot(&original, &rewritten, LegacyIdentity)
        .await
        .unwrap();
    assert_eq!(
        dump_rows(&CrashingDatabase::open(&rewritten, None)).await,
        dump_rows(&CrashingDatabase::open(&expected, None)).await
    );
    client.drive_all_for_test(options()).await.unwrap();
    assert!(client.status().federation.is_some());
}

/// The quote that the fixture's replacement seat 0 was bound to when its
/// payment started. The binding cleared `replacement_for`, so the row is an
/// ordinary bound paid seat; the other six seats exist.
fn bound_replacement_quote() -> QuoteId {
    let fixture: serde_json::Value = serde_json::from_str(R269_BOUND_REPLACEMENT_PAID).unwrap();
    let seat = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0].as_str().unwrap().starts_with("01") && row[1]["index"] == 0)
        .unwrap();
    assert!(seat[1]["replacement_for"].is_null());
    assert!(seat[1]["seat_id"].is_null());
    serde_json::from_value(
        seat[1]["admission"]["peer_badge"]["state"]["effect_authorized"]["quote_id"].clone(),
    )
    .unwrap()
}

/// The wallet prepared the replacement's payment before the crash. The run
/// presents that payment once, pays nothing new, and all seven seats reach
/// the pool.
#[tokio::test]
async fn r269_bound_replacement_with_a_prepared_payment_presents_it_once() {
    let quote = bound_replacement_quote();
    let (client, mem, wallet, mgs) = open_fixture(R269_BOUND_REPLACEMENT_PAID).await;
    assert!(wallet.funded_quotes.lock().unwrap().contains(&quote));
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(!is_buying(&status));
    assert_eq!(federation(&status).seats.len(), 7);
    assert!(wallet.created_quotes.lock().unwrap().is_empty());
    let presented = mgs
        .create_records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| record.quote_id == quote)
        .count();
    assert_eq!(presented, 1);
    reload(&mem).await.expect("the FA still loads after a run");
}

/// The wallet rejected the replacement's payment. The seat leaves without a
/// payment, the six other seats reach the pool, and the creation needs one
/// seat.
#[tokio::test]
async fn r269_bound_replacement_with_a_rejected_payment_leaves() {
    let quote = bound_replacement_quote();
    let (client, mem, wallet, mgs) = open_fixture(R269_BOUND_REPLACEMENT_PAID).await;
    wallet.funded_quotes.lock().unwrap().remove(&quote);
    wallet.rejected_quotes.lock().unwrap().insert(quote);
    let result = client.drive_all_for_test(options()).await;
    assert!(
        matches!(
            result,
            Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
                ..
            })
        ),
        "{result:?}"
    );
    let status = client.status();
    assert!(!is_buying(&status));
    assert_eq!(accepted_seats(&status).len(), 6);
    assert!(status.creation.is_some());
    assert!(wallet.created_quotes.lock().unwrap().is_empty());
    assert!(
        mgs.create_records
            .lock()
            .unwrap()
            .iter()
            .all(|record| record.quote_id != quote)
    );
    reload(&mem).await.expect("the FA still loads after a run");

    rebuy_missing_seats(&client).await.unwrap();
    client.drive_all_for_test(options()).await.unwrap();
    assert_eq!(federation(&client.status()).seats.len(), 7);
    assert_eq!(wallet.created_quotes.lock().unwrap().len(), 1);
}

/// The wallet never started the replacement's payment. The run pays one new
/// quote from the same MG at the previewed plan, and all seven seats reach
/// the pool.
#[tokio::test]
async fn r269_bound_replacement_without_a_started_payment_pays_one_new_quote() {
    let quote = bound_replacement_quote();
    let (client, mem, wallet, mgs) = open_fixture(R269_BOUND_REPLACEMENT_PAID).await;
    wallet.funded_quotes.lock().unwrap().remove(&quote);
    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    assert!(!is_buying(&status));
    assert_eq!(federation(&status).seats.len(), 7);
    let paid = wallet.created_quotes.lock().unwrap().clone();
    assert_eq!(paid.len(), 1);
    assert_ne!(paid[0], quote);
    let fixture: serde_json::Value = serde_json::from_str(R269_BOUND_REPLACEMENT_PAID).unwrap();
    let old_seat = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0].as_str().unwrap().starts_with("01") && row[1]["index"] == 0)
        .unwrap()[1]
        .clone();
    let old_quote: SignedResponse<GetQuoteResponse> =
        serde_json::from_value(old_seat["signed_quote"].clone()).unwrap();
    let records = mgs.quote_records.lock().unwrap();
    let new_quote = records
        .iter()
        .find(|record| record.quote_id == paid[0])
        .expect("the new quote came from a test MG");
    let locator: Locator = serde_json::from_value(old_seat["locator"].clone()).unwrap();
    let old_quote = old_quote.verify(&locator.service_pubkey).unwrap();
    assert_eq!(new_quote.plan, old_quote.terms.request.plan);
    drop(records);
    let presented = mgs.create_records.lock().unwrap();
    assert_eq!(
        presented.iter().filter(|r| r.quote_id == paid[0]).count(),
        1
    );
    assert!(presented.iter().all(|r| r.quote_id != quote));
    drop(presented);
    reload(&mem).await.expect("the FA still loads after a run");
}
