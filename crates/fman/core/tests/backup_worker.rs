use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use fedi_decentralized_service_fleet_manager::{
    FederationSize, FiId, InviteCode, Plan, QuoteId, SeatId,
};
use tempfile::TempDir;

use super::*;
use crate::db::NewSeat;
use crate::facts::SeatFacts;
use crate::seat_process::BitcoindConfig;

/// A sink that can be switched off or held mid-call, so a test can hold the
/// relay down (or slow) and watch what the worker does anyway.
struct SwitchableSink {
    published: Mutex<Vec<SeatId>>,
    archives: AtomicUsize,
    attempts: AtomicUsize,
    fails: AtomicBool,
    holds: AtomicBool,
}

impl SwitchableSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            published: Mutex::new(Vec::new()),
            archives: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            fails: AtomicBool::new(false),
            holds: AtomicBool::new(false),
        })
    }

    fn published(&self) -> Vec<SeatId> {
        self.published.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl BackupSink for SwitchableSink {
    async fn publish(&self, publication: &crate::backup::SeatPublication) -> anyhow::Result<()> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        while self.holds.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if self.fails.load(Ordering::SeqCst) {
            anyhow::bail!("relay is down");
        }
        if publication.archive.is_some() {
            self.archives.fetch_add(1, Ordering::SeqCst);
        }
        self.published
            .lock()
            .unwrap()
            .push(publication.document.seat_id.clone());
        Ok(())
    }

    fn format_version(&self) -> u32 {
        1
    }
}

fn test_facts(seat: u8) -> SeatFacts {
    let mut seed = [0_u8; 32];
    seed[0] = 7;
    let fi_id = FiId(
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
            .unwrap()
            .x_only_public_key()
            .0,
    );
    SeatFacts {
        seat_id: SeatId::from(QuoteId([seat; 32])),
        seat_no: crate::facts::SeatNo(u32::from(seat)),
        fi_id,
        plan: Plan::InfiniteBestEffort { price_msats: 0 },
        federation_size: FederationSize(7),
        created_at_ms: 1_700_000_000_000,
    }
}

async fn test_db(temp: &TempDir) -> Db {
    Db::open(temp.path()).await.unwrap()
}

/// Insert a durable seat; the worker only publishes what the database holds.
async fn create_seat(db: &Db, seat: u8) -> SeatFacts {
    let facts = test_facts(seat);
    crate::test_support::insert_test_seat(
        db,
        NewSeat {
            seat_id: facts.seat_id.clone(),
            fi_id: facts.fi_id,
            plan: facts.plan.clone(),
            federation_size: facts.federation_size,
            payment: None,
        },
    )
    .await
}

fn test_process(temp: &TempDir) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: temp.path().join("fedimintd"),
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(BitcoindConfig {
            url: "http://127.0.0.1:18443".to_owned(),
            username: "user".to_owned(),
            password: "pass".to_owned(),
        }),
    }
}

async fn write_test_archive(process: &SeatProcessConfig, facts: &SeatFacts) {
    let dir = crate::seat_process::seat_data_dir(process, facts.seat_no);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    for (name, contents) in [
        ("private.encrypt", "deadbeef"),
        ("private.salt", "c2FsdA"),
        ("local.json", r#"{"api_bind":"127.0.0.1:1"}"#),
        ("consensus.json", r#"{"api_endpoints":{}}"#),
    ] {
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }
}

const FAST_SCAN: Duration = Duration::from_millis(10);

/// Await a condition without pinning a wall-clock budget into the assertion.
async fn eventually(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

/// [`eventually`] for conditions that read the database.
async fn eventually_async<F: Future<Output = bool>>(mut condition: impl FnMut() -> F) {
    for _ in 0..500 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

/// A confirmed publication is durable: once the document on the relay matches
/// the document the seat's state assembles to, scan after scan finds nothing
/// to do.
#[tokio::test]
async fn an_unchanged_seat_is_published_once() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let facts = create_seat(&db, 1).await;
    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db, test_process(&temp));

    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;
    // Dozens more scans pass; the confirmed record keeps every one a no-op.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sink.published(), vec![facts.seat_id.clone()]);
}

/// The point of derived dirtiness: marking is a wakeup, and a wedged relay
/// costs the marking path nothing.
#[tokio::test]
async fn marking_never_waits_on_the_relay() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    sink.fails.store(true, Ordering::SeqCst);
    let db = test_db(&temp).await;
    create_seat(&db, 2).await;
    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db, test_process(&temp));

    let started = std::time::Instant::now();
    for _ in 0..100 {
        worker.mark();
    }
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "marking blocked on the publisher"
    );
}

/// A failed publication is retried until it lands, and only the confirmed
/// publish writes the record: no relay, no record, no false "backed up".
#[tokio::test]
async fn a_failed_publication_is_retried_until_it_lands() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    sink.fails.store(true, Ordering::SeqCst);
    let db = test_db(&temp).await;
    let facts = create_seat(&db, 3).await;
    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db.clone(), test_process(&temp));

    eventually(|| sink.attempts.load(Ordering::SeqCst) > 1).await;
    assert!(sink.published().is_empty());
    assert!(
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_none(),
        "a failed publish must not be recorded as confirmed"
    );

    sink.fails.store(false, Ordering::SeqCst);
    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;
    let mut recorded = None;
    for _ in 0..500 {
        recorded = db.backup_publication(&facts.seat_id, 1).await.unwrap();
        if recorded.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let recorded = recorded.expect("a confirmed publish is recorded");
    assert!(recorded.published_at_ms > 0);
}

/// A durable state change republishes without anyone enumerating what
/// changed: the scan rederives the document and sees the hash move. The mark
/// only makes it prompt.
#[tokio::test]
async fn a_decommission_reaches_the_relay_before_the_next_scan_tick() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let facts = create_seat(&db, 4).await;
    // A scan interval far beyond the test budget: only the mark can explain
    // the second publication arriving in time.
    let worker = BackupWorker::new(sink.clone() as _, Duration::from_secs(600));
    worker.spawn(db.clone(), test_process(&temp));

    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;

    db.decommission_seat(&facts.seat_id).await.unwrap();
    worker.mark();
    eventually(|| sink.published().len() == 2).await;
}

/// A seat that has run consensus holds key shares that exist nowhere else.
/// Until the document carries them the seat stays pending — publishing
/// without them would leave a seat that looks backed up while the only copy
/// of its shares is on one disk.
#[tokio::test]
async fn a_seat_running_consensus_is_not_published_without_its_guardian_config() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let process = test_process(&temp);
    let facts = create_seat(&db, 6).await;
    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();

    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db.clone(), test_process(&temp));

    // fedimintd has not written its config out yet. The worker does not even
    // reach the relay: there is nothing worth publishing, so the seat pends.
    eventually(|| {
        worker
            .last_scan()
            .is_some_and(|scan| scan.pending_seats == 1)
    })
    .await;
    assert_eq!(sink.attempts.load(Ordering::SeqCst), 0);

    write_test_archive(&process, &facts).await;
    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;
    assert!(sink.archives.load(Ordering::SeqCst) >= 1);
}

/// The regression the old in-memory queue had: the archive requirement is a
/// durable formed record, so a restart with the archive
/// files missing still refuses to publish a share-less document over the one
/// the relay holds.
#[tokio::test]
async fn a_restart_cannot_downgrade_a_consensus_seat_to_a_shareless_document() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let facts = create_seat(&db, 7).await;
    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();

    // A fresh worker over the same database is a daemon restart. No archive
    // files exist (a lost data directory); the durable observation must still
    // hold the line.
    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db, test_process(&temp));

    eventually(|| {
        worker
            .last_scan()
            .is_some_and(|scan| scan.pending_seats == 1)
    })
    .await;
    assert_eq!(sink.attempts.load(Ordering::SeqCst), 0);
}

/// Before the consensus observation is durable, `RestartDKG` can still wipe
/// the seat's directory and `fedimintd` may still be mid-write, so config
/// files sitting on disk are not yet the seat's archive. The worker publishes
/// the share-less document and withholds the archive until the observation
/// lands — then the mark makes the archive follow promptly.
#[tokio::test]
async fn an_archive_is_withheld_until_the_observation_is_durable() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let process = test_process(&temp);
    let facts = create_seat(&db, 9).await;
    // The files exist — a DKG mid-flight has written them — but no probe has
    // observed consensus yet.
    write_test_archive(&process, &facts).await;

    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db.clone(), test_process(&temp));

    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;
    eventually_async(|| async {
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_some()
    })
    .await;
    assert_eq!(sink.archives.load(Ordering::SeqCst), 0);
    let record = db
        .backup_publication(&facts.seat_id, 1)
        .await
        .unwrap()
        .expect("the share-less document is still confirmed and recorded");
    assert!(record.archive_digest.is_none());

    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();
    worker.mark();
    eventually(|| sink.archives.load(Ordering::SeqCst) >= 1).await;
    eventually_async(|| async {
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_some_and(|record| record.archive_digest.is_some())
    })
    .await;
}

/// The plan's consensus check and the publish are not one atomic step. If the
/// first observation commits while a share-less document is in flight, the
/// worker must not record that publication: the record would suppress the
/// republish that adds the archive.
#[tokio::test]
async fn an_observation_landing_mid_publish_keeps_the_seat_pending() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let process = test_process(&temp);
    let facts = create_seat(&db, 10).await;

    // Hold the relay call open, and land the observation while it is held.
    sink.holds.store(true, Ordering::SeqCst);
    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db.clone(), test_process(&temp));
    eventually(|| sink.attempts.load(Ordering::SeqCst) == 1).await;
    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();
    sink.holds.store(false, Ordering::SeqCst);

    // The publish itself succeeded, but the recheck refuses the record and
    // the seat stays pending until the archive can be published too.
    eventually(|| {
        worker
            .last_scan()
            .is_some_and(|scan| scan.pending_seats == 1)
    })
    .await;
    assert_eq!(sink.published(), vec![facts.seat_id.clone()]);
    assert!(
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_none(),
        "a document confirmed share-less after the observation must not be recorded"
    );

    write_test_archive(&process, &facts).await;
    eventually(|| sink.archives.load(Ordering::SeqCst) >= 1).await;
    eventually_async(|| async {
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_some_and(|record| record.archive_digest.is_some())
    })
    .await;
}

/// Once the archive is confirmed on the relay, later scans neither reread the
/// seat's files nor republish its bytes: the recorded digest stands in for
/// both, so a steady-state scan is a few point queries.
#[tokio::test]
async fn a_confirmed_archive_is_never_reread_or_republished() {
    let temp = TempDir::new().unwrap();
    let sink = SwitchableSink::new();
    let db = test_db(&temp).await;
    let process = test_process(&temp);
    let facts = create_seat(&db, 8).await;
    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();
    write_test_archive(&process, &facts).await;

    let worker = BackupWorker::new(sink.clone() as _, FAST_SCAN);
    worker.spawn(db.clone(), test_process(&temp));
    eventually(|| sink.published() == vec![facts.seat_id.clone()]).await;
    let archives = sink.archives.load(Ordering::SeqCst);
    assert!(archives >= 1);

    // Deleting the files proves later scans run off the confirmed digest: a
    // scan that reread the directory would refuse (consensus ran, no archive)
    // and show up as a pending seat.
    tokio::fs::remove_dir_all(crate::seat_process::seat_data_dir(&process, facts.seat_no))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sink.archives.load(Ordering::SeqCst), archives);
    assert_eq!(sink.published(), vec![facts.seat_id.clone()]);
    assert!(
        worker
            .last_scan()
            .is_some_and(|scan| scan.pending_seats == 0)
    );
}

/// The envelope schema version scopes a confirmed publication: the version is
/// outside the hashed plaintext, so after an upgrade an unchanged document
/// still needs republishing — the relay holds events the new build's own
/// restore would refuse. The archive digest's never-regress rule holds only
/// within one version for the same reason.
#[tokio::test]
async fn a_publication_record_counts_only_for_the_version_that_wrote_it() {
    let temp = TempDir::new().unwrap();
    let db = test_db(&temp).await;
    let facts = create_seat(&db, 11).await;

    db.record_backup_publication(&facts.seat_id, "doc-hash", Some("archive-digest"), 1)
        .await
        .unwrap();
    assert!(
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        db.backup_publication(&facts.seat_id, 2)
            .await
            .unwrap()
            .is_none(),
        "another version's record is no confirmation"
    );

    // Same version: a document-only republish must not forget the archive.
    db.record_backup_publication(&facts.seat_id, "doc-hash-2", None, 1)
        .await
        .unwrap();
    assert_eq!(
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .unwrap()
            .archive_digest
            .as_deref(),
        Some("archive-digest")
    );

    // New version: the confirmed archive is laid out under rules the new
    // version's restore does not read, so the digest is taken as given — absent.
    db.record_backup_publication(&facts.seat_id, "doc-hash-2", None, 2)
        .await
        .unwrap();
    let record = db
        .backup_publication(&facts.seat_id, 2)
        .await
        .unwrap()
        .unwrap();
    assert!(record.archive_digest.is_none());
    assert!(
        db.backup_publication(&facts.seat_id, 1)
            .await
            .unwrap()
            .is_none(),
        "one row per seat: the new version's record replaces the old"
    );
}
