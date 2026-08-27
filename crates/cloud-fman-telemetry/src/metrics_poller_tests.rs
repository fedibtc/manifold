//! Production authenticated Iroh metrics wire test.

use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GuardianMetricsResponse,
    GuardianTelemetryApi, GuardianTelemetryApiServer, GuardianTelemetrySeat,
    ListGuardianTelemetrySeatsRequest, ListGuardianTelemetrySeatsResponse,
    ListSafeEventJournalsRequest, ListSafeEventJournalsResponse, ScrapeGuardianMetricsRequest,
    SeatId, TelemetryResult,
};
use fedi_iroh_rpc::{
    IrohProtocol,
    iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router},
};

use super::*;

const VALID_INVITE: &str = "fed11qgqpu8rhwden5te0vejkg6tdd9h8gepwd4cxcumxv4jzuen0duhsqqfqh6nl7sgk72caxfx8khtfnn8y436q3nhyrkev3qp8ugdhdllnh86qmp42pm";

#[test]
fn federation_attribution_requires_a_parseable_invite() {
    assert!(federation_id_from_invite(None).is_none());
    assert!(
        federation_id_from_invite(Some(&fedi_decentralized_service_fleet_manager::InviteCode(
            "not-an-invite".to_owned()
        )))
        .is_none()
    );
    let federation_id = federation_id_from_invite(Some(
        &fedi_decentralized_service_fleet_manager::InviteCode(VALID_INVITE.to_owned()),
    ))
    .unwrap();
    assert_eq!(federation_id.len(), 64);
    assert!(
        federation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
}

fn secure_state_path(directory: &tempfile::TempDir) -> std::path::PathBuf {
    let data_dir = directory.path().join("collector");
    drop(crate::data_root_lock::DataRootLock::acquire(&data_dir).unwrap());
    data_dir.join("state.sqlite")
}

#[derive(Clone)]
struct TestService {
    good_seat: SeatId,
    bad_seat: SeatId,
    bad_delay: Duration,
}

impl GuardianTelemetryApi for TestService {
    async fn list_guardian_telemetry_seats(
        &self,
        request: ListGuardianTelemetrySeatsRequest,
    ) -> TelemetryResult<ListGuardianTelemetrySeatsResponse> {
        assert_eq!(request.capability.as_bytes(), &[7; 32]);
        Ok(ListGuardianTelemetrySeatsResponse {
            seats: vec![
                GuardianTelemetrySeat {
                    seat_id: self.good_seat.clone(),
                    invite_code: Some(fedi_decentralized_service_fleet_manager::InviteCode(
                        VALID_INVITE.to_owned(),
                    )),
                },
                GuardianTelemetrySeat {
                    seat_id: self.bad_seat.clone(),
                    invite_code: Some(fedi_decentralized_service_fleet_manager::InviteCode(
                        VALID_INVITE.to_owned(),
                    )),
                },
            ],
        })
    }

    async fn scrape_guardian_metrics(
        &self,
        request: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse> {
        assert_eq!(request.capability.as_bytes(), &[7; 32]);
        let good = request.seat_id == self.good_seat;
        if !good {
            tokio::time::sleep(self.bad_delay).await;
        }
        Ok(GuardianMetricsResponse {
            status_code: if good { 200 } else { 503 },
            content_type: Some("text/plain; version=0.0.4".to_owned()),
            content_encoding: None,
            body: b"fm_app_start_ts{version=\"test\",version_hash=\"hash\"} 1\nfm_consensus_session_count 2\n".to_vec(),
        })
    }

    async fn list_safe_event_journals(
        &self,
        _: ListSafeEventJournalsRequest,
    ) -> TelemetryResult<ListSafeEventJournalsResponse> {
        unreachable!()
    }

    async fn fetch_safe_event_journal(
        &self,
        _: FetchSafeEventJournalRequest,
    ) -> TelemetryResult<FetchSafeEventJournalResponse> {
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_production_client_discovers_and_scrapes_a_seat() {
    let good_seat = SeatId::new("22".repeat(32)).unwrap();
    let bad_seat = SeatId::new("33".repeat(32)).unwrap();
    let server_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let router = Router::builder(server_endpoint)
        .accept(
            GUARDIAN_TELEMETRY_ALPN,
            IrohProtocol::new(GuardianTelemetryApiServer::new(TestService {
                good_seat,
                bad_seat,
                bad_delay: Duration::from_secs(2),
            })),
        )
        .spawn();
    let client_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(
        &secure_state_path(&directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    let signer = "11".repeat(32);
    let auth = crate::auth::VerifiedHttpAuth {
        signer: signer.clone(),
        event_id: "metrics-deadline".into(),
        created_at: 100,
    };
    store.reserve_auth(&auth, 100).await.unwrap();
    let endpoint_id = router.endpoint().id().to_string();
    store
        .admit(
            &auth,
            crate::store::TargetMaterial {
                fman_pubkey: &signer,
                fman_name: "calm-tern",
                endpoint_id: &endpoint_id,
                capability: &[7; 32],
                generation: 1,
            },
            100,
        )
        .await
        .unwrap();
    let scheduled = store.due_metric_targets(100).await.unwrap().remove(0);
    assert!(
        store
            .reserve_metric_attempt(&scheduled, 100, 1800)
            .await
            .unwrap()
    );
    let target = store
        .begin_collection_work(&scheduled, 100)
        .await
        .unwrap()
        .unwrap();
    let mut poller = MetricsPoller::new(
        store.clone(),
        client_endpoint,
        "test".to_owned(),
        "hash".to_owned(),
        false,
        std::num::NonZeroUsize::new(1).unwrap(),
        Duration::from_secs(1800),
    );
    poller.connect_address = Some(router.endpoint().addr());
    let started = tokio::time::Instant::now();
    let commit = poller
        .collect_target(&target, started + Duration::from_secs(1))
        .await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!commit.complete);
    assert_eq!(commit.snapshots.len(), 1);
    let expected_federation = fedimint_core::invite_code::InviteCode::from_str(VALID_INVITE)
        .unwrap()
        .federation_id()
        .to_string();
    assert_eq!(
        commit.snapshots[0].federation_id, expected_federation,
        "the invite-derived binding stays with the exact scraped seat"
    );
    assert!(
        commit
            .listed_seats
            .as_ref()
            .unwrap()
            .contains(&commit.snapshots[0].guardian_seat_id)
    );
    store.commit_metrics(&target, commit, 101).await.unwrap();
    let policy = MetricsPolicy {
        version: "test",
        version_hash: "hash",
        canonical_method_labels: false,
    };
    assert_eq!(
        store
            .metric_snapshots(&policy, 101, i64::MAX)
            .await
            .unwrap()
            .snapshots
            .len(),
        1
    );
    router.shutdown().await.unwrap();
}

#[test]
fn hostile_target_deadlines_leave_every_due_target_a_slot_within_cadence() {
    for targets in [1_usize, 4, 5, 100, 4096] {
        let concurrency = 4_usize;
        let cadence = Duration::from_secs(900);
        let waves = targets.div_ceil(concurrency);
        let budget = fair_target_budget(cadence, concurrency, targets);
        assert!(budget <= MAX_TARGET_BUDGET);
        assert!(budget * u32::try_from(waves).unwrap() <= cadence);
    }
}

#[test]
fn wake_uses_the_durable_deadline_after_a_long_cycle() {
    let cadence = Duration::from_secs(900);
    assert_eq!(wake_delay(1_000, Some(1_000), cadence), Duration::ZERO);
    assert_eq!(
        wake_delay(1_000, Some(1_450), cadence),
        Duration::from_secs(450)
    );
    assert_eq!(wake_delay(1_000, None, cadence), cadence);
}

fn durability_hook() -> std::sync::Arc<crate::store::TestCommitHook> {
    std::sync::Arc::new(crate::store::TestCommitHook {
        entered_once: std::sync::atomic::AtomicBool::new(false),
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    })
}

async fn registered_test_store(
    directory: &tempfile::TempDir,
) -> (Store, crate::auth::VerifiedHttpAuth, i64) {
    let store = Store::open(
        &secure_state_path(directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    let now = unix_seconds().unwrap();
    let auth = crate::auth::VerifiedHttpAuth {
        signer: "11".repeat(32),
        event_id: "metrics-drain".into(),
        created_at: now,
    };
    store.reserve_auth(&auth, now).await.unwrap();
    store
        .admit(
            &auth,
            crate::store::TargetMaterial {
                fman_pubkey: &auth.signer,
                fman_name: "calm-tern",
                endpoint_id: "not-an-endpoint",
                capability: &[7; 32],
                generation: 1,
            },
            now,
        )
        .await
        .unwrap();
    (store, auth, now)
}

async fn test_poller(store: Store) -> MetricsPoller {
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    MetricsPoller::new(
        store,
        endpoint,
        "test".into(),
        "hash".into(),
        false,
        std::num::NonZeroUsize::new(1).unwrap(),
        Duration::from_secs(1800),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drains_reservation_commit_before_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _, now) = registered_test_store(&directory).await;
    let hook = durability_hook();
    let poller = test_poller(store.with_metric_reservation_hook(hook.clone())).await;
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(poller.run(receiver));
    let entered = hook.clone();
    tokio::task::spawn_blocking(move || entered.entered.wait())
        .await
        .unwrap();
    shutdown.send_replace(true);
    assert!(!task.is_finished());
    let release = hook.clone();
    tokio::task::spawn_blocking(move || release.release.wait())
        .await
        .unwrap();
    task.await.unwrap().unwrap();

    let reopened = Store::open(
        &secure_state_path(&directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    let next_due = reopened.next_metric_due_at(now).await.unwrap().unwrap();
    assert!((now + 1800..=now + 1802).contains(&next_due));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drains_snapshot_commit_before_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _, now) = registered_test_store(&directory).await;
    let hook = durability_hook();
    let initial_revision = store.metric_exposition_version(now).await.unwrap().revision;
    let poller = test_poller(store.with_metric_commit_hook(hook.clone())).await;
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(poller.run(receiver));
    let entered = hook.clone();
    tokio::task::spawn_blocking(move || entered.entered.wait())
        .await
        .unwrap();
    shutdown.send_replace(true);
    assert!(!task.is_finished());
    let release = hook.clone();
    tokio::task::spawn_blocking(move || release.release.wait())
        .await
        .unwrap();
    task.await.unwrap().unwrap();

    let reopened = Store::open(
        &secure_state_path(&directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    assert!(
        reopened
            .metric_exposition_version(now)
            .await
            .unwrap()
            .revision
            > initial_revision
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_sibling_does_not_cancel_snapshot_commit() {
    let directory = tempfile::tempdir().unwrap();
    let (store, auth, now) = registered_test_store(&directory).await;
    let second_auth = crate::auth::VerifiedHttpAuth {
        event_id: "metrics-drain-second".into(),
        ..auth
    };
    store.reserve_auth(&second_auth, now).await.unwrap();
    store
        .admit(
            &second_auth,
            crate::store::TargetMaterial {
                fman_pubkey: "22",
                fman_name: "other-display",
                endpoint_id: "also-not-an-endpoint",
                capability: &[8; 32],
                generation: 1,
            },
            now,
        )
        .await
        .unwrap();
    let initial_revision = store.metric_exposition_version(now).await.unwrap().revision;
    let hook = durability_hook();
    let store = store
        .with_metric_commit_hook(hook.clone())
        .with_metric_reservation_failure_after(1);
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let poller = MetricsPoller::new(
        store,
        endpoint,
        "test".into(),
        "hash".into(),
        false,
        std::num::NonZeroUsize::new(2).unwrap(),
        Duration::from_secs(1800),
    );
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(poller.run(receiver));
    let entered = hook.clone();
    tokio::task::spawn_blocking(move || entered.entered.wait())
        .await
        .unwrap();
    shutdown.send_replace(true);
    assert!(
        !task.is_finished(),
        "fatal sibling must join the active commit"
    );
    let release = hook.clone();
    tokio::task::spawn_blocking(move || release.release.wait())
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(MetricsPollError::ReserveAttempt)
    ));

    let reopened = Store::open(
        &secure_state_path(&directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    assert!(
        reopened
            .metric_exposition_version(now)
            .await
            .unwrap()
            .revision
            > initial_revision
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_sibling_does_not_cancel_attempt_reservation() {
    let directory = tempfile::tempdir().unwrap();
    let (store, auth, now) = registered_test_store(&directory).await;
    let second_auth = crate::auth::VerifiedHttpAuth {
        event_id: "metrics-reservation-second".into(),
        ..auth
    };
    store.reserve_auth(&second_auth, now).await.unwrap();
    store
        .admit(
            &second_auth,
            crate::store::TargetMaterial {
                fman_pubkey: "22",
                fman_name: "other-display",
                endpoint_id: "also-not-an-endpoint",
                capability: &[8; 32],
                generation: 1,
            },
            now,
        )
        .await
        .unwrap();
    let hook = durability_hook();
    let store = store
        .with_metric_reservation_hook(hook.clone())
        .with_metric_reservation_failure_after(1);
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let poller = MetricsPoller::new(
        store,
        endpoint,
        "test".into(),
        "hash".into(),
        false,
        std::num::NonZeroUsize::new(2).unwrap(),
        Duration::from_secs(1800),
    );
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(poller.run(receiver));
    let entered = hook.clone();
    tokio::task::spawn_blocking(move || entered.entered.wait())
        .await
        .unwrap();
    shutdown.send_replace(true);
    assert!(
        !task.is_finished(),
        "fatal sibling must join the active reservation"
    );
    let release = hook.clone();
    tokio::task::spawn_blocking(move || release.release.wait())
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(MetricsPollError::ReserveAttempt)
    ));

    let reopened = Store::open(
        &secure_state_path(&directory),
        "test-profile",
        crate::cipher::SecretCipher::new(&[3; 32]),
        "test-key".to_owned(),
        3600,
    )
    .await
    .unwrap();
    assert_eq!(reopened.due_metric_targets(now).await.unwrap().len(), 1);
    assert_eq!(
        reopened
            .due_metric_targets(crate::journal_types::unix_seconds().unwrap() + 1802)
            .await
            .unwrap()
            .len(),
        2
    );
}
