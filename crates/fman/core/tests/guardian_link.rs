use std::sync::Mutex;

use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallbackInput, FiId, InviteCode, SeatId,
};
use secp256k1::Keypair;
use tempfile::TempDir;

use super::*;
use crate::db::Db;
use crate::facts::PortBase;
use crate::fleet::FleetConfig;
use crate::push_callback::{
    CompletionCallbackInvoker, PushGatewayOrigin, PushGatewayOriginPolicy,
    ValidatedDkgCompletionCallback,
};
use crate::seat_process::fake::{block_forever, write_fake_fedimintd};
use crate::seat_process::{BitcoindConfig, RespawnPolicy, SeatProcessConfig, SeatProcessSpawner};
use crate::seat_readiness::{ReadinessOutcome, ReadinessReport};
use crate::wallet::NoWallet;

const ORIGIN: &str = "https://push.example.com";

/// Records every invocation and answers with whatever outcome is queued.
struct RecordingInvoker {
    calls: Mutex<Vec<(String, String)>>,
    outcome: Mutex<CallbackAttemptOutcome>,
}

impl RecordingInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            outcome: Mutex::new(CallbackAttemptOutcome::Delivered),
        })
    }

    fn answer(&self, outcome: CallbackAttemptOutcome) {
        *self.outcome.lock().unwrap() = outcome;
    }

    /// `(idempotency_key, reasons)` per call so far.
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for RecordingInvoker {
    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        let reasons = callback
            .data()
            .get("reasons")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned();
        self.calls
            .lock()
            .unwrap()
            .push((callback.idempotency_key().to_owned(), reasons));
        *self.outcome.lock().unwrap()
    }
}

struct Harness {
    fleet: Arc<Fleet>,
    invoker: Arc<RecordingInvoker>,
    presence_tx: watch::Sender<DirectoryPresence>,
    _temp: TempDir,
}

async fn harness(origin: Option<&str>) -> Harness {
    let temp = TempDir::new().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    crate::onboarding::onboard_as_new(&db).await.unwrap();
    db.complete_onboarding_for_test(1).await.unwrap();
    let invoker = RecordingInvoker::new();
    let fleet = Fleet::open(
        db,
        FleetConfig {
            process_spawner: SeatProcessSpawner::Fake(Arc::new(
                crate::seat_process::fake::FakeSeatProcessSpawner::default(),
            )),
            manifold_environment:
                fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
            first_port_base: PortBase::new(30_000).unwrap(),
            setup_payments_configured: true,
            guardian_verification_fee_account: None,
            respawn: RespawnPolicy::default(),
            backup_scan_interval: Duration::from_millis(10),
            push_gateway_origin: origin.map(|origin| {
                PushGatewayOrigin::parse(origin, PushGatewayOriginPolicy::HttpsOnly).unwrap()
            }),
            push_callback_retry_interval: Duration::from_millis(10),
            completion_callback_invoker: invoker.clone(),
            process: SeatProcessConfig {
                data_root: temp.path().to_owned(),
                fedimintd: write_fake_fedimintd(temp.path(), &block_forever()).await,
                bitcoin_network: bitcoin::Network::Regtest,
                iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
                bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind {
                    primary: BitcoindConfig {
                        url: "http://127.0.0.1:18443".to_owned(),
                        username: "user".to_owned(),
                        password: "pass".to_owned(),
                    },
                    esplora_fallback: None,
                },
            },
        },
        Arc::new(NoWallet),
    )
    .await
    .unwrap();
    let fleet = Arc::new(fleet);
    fleet.bind_iroh_endpoint_id("e".repeat(52));
    let (presence_tx, _) = watch::channel(presence(&fleet, OnboardingStatus::Checking));
    Harness {
        fleet,
        invoker,
        presence_tx,
        _temp: temp,
    }
}

fn presence(fleet: &Fleet, onboarding: OnboardingStatus) -> DirectoryPresence {
    DirectoryPresence {
        service_nostr_pubkey: fleet.identity().derive_service_nostr_keys().public_key(),
        onboarding,
        latest_fman_version: None,
    }
}

fn authorized(fleet: &Fleet) -> DirectoryPresence {
    presence(
        fleet,
        OnboardingStatus::AuthorizationObserved {
            authorizations: 1,
            holders: Vec::new(),
            checked_at: Some(1),
        },
    )
}

fn hook(path: &str) -> DkgCompletionCallback {
    DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{ORIGIN}/hooks/{path}"),
        idempotency_key: "unused".to_owned(),
    })
    .unwrap()
}

fn device() -> (Keypair, FiId) {
    let key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    (key, FiId(key.x_only_public_key().0))
}

fn far_future() -> Timestamp {
    Timestamp(now_secs() + 30 * 24 * 3600)
}

/// Link a fresh device through the RPC, the way the app does.
async fn link(harness: &Harness) -> (GuardianLinkRpc, Keypair, FiId) {
    let rpc = GuardianLinkRpc::new(harness.fleet.clone(), harness.presence_tx.subscribe());
    let invite = harness.fleet.create_guardian_link_offer().unwrap();
    let (key, device_id) = device();
    rpc.link_device(LinkDeviceRequest {
        version: ProtocolV1,
        secret: invite.secret,
        device_id,
        device_label: "Pixel".to_owned(),
        callback: hook("h1/s1"),
        callback_expires_at: far_future(),
    })
    .await
    .unwrap();
    (rpc, key, device_id)
}

fn seat(n: u8) -> SeatId {
    SeatId::from(fedi_decentralized_service_fleet_manager::QuoteId([n; 32]))
}

fn running(health: SeatHealth) -> SeatReport {
    SeatReport::Active {
        phase: SeatPhase::Running {
            invite_code: InviteCode("fed1invite".to_owned()),
        },
        health,
    }
}

#[tokio::test]
async fn evaluator_reads_only_formed_seats_and_waits_out_a_short_outage() {
    let h = harness(Some(ORIGIN)).await;
    let presence = authorized(&h.fleet);
    // An unformed seat is idle, not broken; a running one that just went
    // away gets the grace period.
    let seats = vec![
        (
            seat(1),
            SeatReport::Active {
                phase: SeatPhase::Created,
                health: SeatHealth::Unavailable,
            },
        ),
        (seat(2), running(SeatHealth::Unavailable)),
    ];
    let reasons = h
        .fleet
        .attention_reasons_with(seats.clone(), &presence)
        .await
        .unwrap();
    assert!(reasons.is_empty(), "{reasons:?}");
    h.fleet
        .guardian_link
        .backdate_unavailable_for_test(SEAT_UNAVAILABLE_GRACE);
    let reasons = h
        .fleet
        .attention_reasons_with(seats, &presence)
        .await
        .unwrap();
    assert_eq!(reasons, BTreeSet::from([AttentionReason::SeatUnavailable]));
    // Recovery clears the memory: a later outage starts its own grace.
    let reasons = h
        .fleet
        .attention_reasons_with(vec![(seat(2), running(SeatHealth::Healthy))], &presence)
        .await
        .unwrap();
    assert!(reasons.is_empty());
    let reasons = h
        .fleet
        .attention_reasons_with(vec![(seat(2), running(SeatHealth::Unavailable))], &presence)
        .await
        .unwrap();
    assert!(reasons.is_empty());
    // Lost data is a failure from the first look.
    let reasons = h
        .fleet
        .attention_reasons_with(
            vec![(
                seat(3),
                SeatReport::Active {
                    phase: SeatPhase::DataLoss {
                        invite_code: InviteCode("fed1invite".to_owned()),
                    },
                    health: SeatHealth::Unavailable,
                },
            )],
            &presence,
        )
        .await
        .unwrap();
    assert_eq!(reasons, BTreeSet::from([AttentionReason::SeatFailed]));
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn evaluator_reads_presence_support_and_readiness() {
    let h = harness(Some(ORIGIN)).await;
    // Before the first readiness run nothing is known, so nothing is wrong.
    let reasons = h
        .fleet
        .attention_reasons_with(Vec::new(), &presence(&h.fleet, OnboardingStatus::Checking))
        .await
        .unwrap();
    assert!(reasons.is_empty(), "{reasons:?}");
    let not_observed = presence(&h.fleet, OnboardingStatus::NotObserved { checked_at: 1 });
    h.fleet
        .set_seat_readiness(ReadinessReport {
            checked_at_ms: 1,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::DiscoveryRecordMissing,
            bitcoin: ReadinessOutcome::Pass,
        })
        .await
        .unwrap();
    h.fleet
        .db()
        .record_support_message(&crate::db::SupportRow {
            rumor_id: "1".repeat(64),
            from_fedi: true,
            body: "hello".to_owned(),
            created_at: 1,
            unread: true,
        })
        .await
        .unwrap();
    let reasons = h
        .fleet
        .attention_reasons_with(Vec::new(), &not_observed)
        .await
        .unwrap();
    assert_eq!(
        reasons,
        BTreeSet::from([
            AttentionReason::NotReadyForNewSeats,
            AttentionReason::NotApproved,
            AttentionReason::SupportMessage,
        ])
    );
    h.fleet
        .db()
        .mark_support_read(&["1".repeat(64)])
        .await
        .unwrap();
    h.fleet
        .set_seat_readiness(ReadinessReport {
            checked_at_ms: 2,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::Pass,
            bitcoin: ReadinessOutcome::Pass,
        })
        .await
        .unwrap();
    let reasons = h
        .fleet
        .attention_reasons_with(Vec::new(), &authorized(&h.fleet))
        .await
        .unwrap();
    assert!(reasons.is_empty(), "{reasons:?}");
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn notifier_sends_once_per_new_reason_and_again_after_it_clears() {
    let h = harness(Some(ORIGIN)).await;
    let (_, _, _) = link(&h).await;
    let mut backoff = None;
    let not_observed = presence(&h.fleet, OnboardingStatus::NotObserved { checked_at: 1 });
    assert!(tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    // Same reasons again: silence.
    assert!(!tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    // A second reason joins: one more send carries both.
    h.fleet
        .db()
        .record_support_message(&crate::db::SupportRow {
            rumor_id: "1".repeat(64),
            from_fedi: true,
            body: "hello".to_owned(),
            created_at: 1,
            unread: true,
        })
        .await
        .unwrap();
    assert!(tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    // Approval clears one reason; nothing new, so nothing is sent, but the
    // memory narrows so a relapse is told again.
    let approved = authorized(&h.fleet);
    assert!(!tick(&h.fleet, &approved, &mut backoff).await.unwrap());
    assert!(tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    let calls = h.invoker.calls();
    let linked = link.linked_at_ms;
    assert_eq!(
        calls,
        vec![
            (
                format!("guardian-link:{linked}:0"),
                "not_approved".to_owned()
            ),
            (
                format!("guardian-link:{linked}:1"),
                "not_approved,support_message".to_owned()
            ),
            (
                format!("guardian-link:{linked}:2"),
                "not_approved,support_message".to_owned()
            ),
        ]
    );
    assert_eq!(link.notification_seq, 3);
    assert!(link.last_notified_at_ms.is_some());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn notifier_retries_transient_failures_and_parks_terminal_ones() {
    let h = harness(Some(ORIGIN)).await;
    link(&h).await;
    let not_observed = presence(&h.fleet, OnboardingStatus::NotObserved { checked_at: 1 });
    let mut backoff = None;
    h.invoker.answer(CallbackAttemptOutcome::Retryable(
        CompletionCallbackReason::Network,
    ));
    assert!(!tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    // Inside the backoff window the hook is left alone.
    backoff.as_mut().unwrap().1 = Instant::now() + Duration::from_secs(60);
    assert!(!tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    assert_eq!(h.invoker.calls().len(), 1);
    backoff.as_mut().unwrap().1 = Instant::now();
    h.invoker.answer(CallbackAttemptOutcome::Delivered);
    assert!(tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    // The retry reuses the sequence number, so the gateway deduplicates.
    let calls = h.invoker.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, calls[1].0);
    assert!(backoff.is_none());

    // A terminal rejection parks the link until the phone relinks.
    h.fleet
        .db()
        .record_support_message(&crate::db::SupportRow {
            rumor_id: "1".repeat(64),
            from_fedi: true,
            body: "hello".to_owned(),
            created_at: 1,
            unread: true,
        })
        .await
        .unwrap();
    h.invoker.answer(CallbackAttemptOutcome::Terminal(
        CompletionCallbackReason::HookExpiredOrRevoked,
    ));
    assert!(!tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(
        link.delivery,
        GuardianLinkDelivery::Terminal(CompletionCallbackReason::HookExpiredOrRevoked)
    );
    assert!(link.callback.is_none());
    h.invoker.answer(CallbackAttemptOutcome::Delivered);
    assert!(!tick(&h.fleet, &not_observed, &mut backoff).await.unwrap());
    assert_eq!(h.invoker.calls().len(), 3);
    assert!(h.fleet.test_guardian_link_notification().await.is_err());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn link_needs_the_open_offer_and_a_hook_under_the_gateway() {
    let h = harness(Some(ORIGIN)).await;
    let rpc = GuardianLinkRpc::new(h.fleet.clone(), h.presence_tx.subscribe());
    let (_, device_id) = device();
    let request = |secret: LinkSecret, callback: DkgCompletionCallback| LinkDeviceRequest {
        version: ProtocolV1,
        secret,
        device_id,
        device_label: "Pixel".to_owned(),
        callback,
        callback_expires_at: far_future(),
    };
    // No offer open.
    assert_eq!(
        rpc.link_device(request(LinkSecret::from_bytes([7; 32]), hook("h/s")))
            .await
            .unwrap_err(),
        GuardianLinkError::InvalidInvite
    );
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    assert!(
        h.fleet
            .guardian_link_status()
            .await
            .unwrap()
            .offer
            .is_some()
    );
    // Wrong secret leaves the offer open.
    assert_eq!(
        rpc.link_device(request(LinkSecret::from_bytes([7; 32]), hook("h/s")))
            .await
            .unwrap_err(),
        GuardianLinkError::InvalidInvite
    );
    // A foreign hook is refused before the secret is spent.
    let foreign = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://elsewhere.example.com/hooks/h/s".to_owned(),
        idempotency_key: "x".to_owned(),
    })
    .unwrap();
    assert!(matches!(
        rpc.link_device(request(invite.secret.clone(), foreign))
            .await
            .unwrap_err(),
        GuardianLinkError::PushGatewayUnusable(_)
    ));
    assert_eq!(
        rpc.link_device(request(invite.secret.clone(), hook("h/s")))
            .await
            .unwrap()
            .fman_name,
        h.fleet.fman_name().to_string()
    );
    // The secret is single use and the offer is gone.
    assert_eq!(
        rpc.link_device(request(invite.secret, hook("h/s")))
            .await
            .unwrap_err(),
        GuardianLinkError::InvalidInvite
    );
    let status = h.fleet.guardian_link_status().await.unwrap();
    assert!(status.offer.is_none());
    assert_eq!(status.link.unwrap().device_label, "Pixel");
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn offers_need_a_gateway_and_expire() {
    let h = harness(None).await;
    assert!(h.fleet.create_guardian_link_offer().is_err());
    assert!(!h.fleet.guardian_link_status().await.unwrap().available);
    h.fleet.shutdown().await;

    let h = harness(Some(ORIGIN)).await;
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    h.fleet
        .guardian_link
        .offer
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .expires_at_ms = crate::db::now_ms() - 1;
    assert!(
        h.fleet
            .guardian_link_status()
            .await
            .unwrap()
            .offer
            .is_none()
    );
    assert!(!h.fleet.consume_guardian_link_offer(&invite.secret));
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn signed_verbs_accept_only_the_linked_device() {
    let h = harness(Some(ORIGIN)).await;
    let (rpc, key, device_id) = link(&h).await;
    let (other_key, other_id) = device();
    let signed = |key: &Keypair, device_id: FiId| {
        SignedRequest::create(
            &GetAttentionRequest {
                ts: Timestamp(now_secs()),
                device_id,
            },
            key,
        )
        .unwrap()
    };
    assert_eq!(
        rpc.get_attention(signed(&other_key, other_id))
            .await
            .unwrap_err(),
        GuardianLinkError::Unauthorized
    );
    // A stale timestamp is the same coarse refusal.
    let stale = SignedRequest::create(
        &GetAttentionRequest {
            ts: Timestamp(now_secs() - 2 * 3600),
            device_id,
        },
        &key,
    )
    .unwrap();
    assert_eq!(
        rpc.get_attention(stale).await.unwrap_err(),
        GuardianLinkError::Unauthorized
    );
    let attention = rpc.get_attention(signed(&key, device_id)).await.unwrap();
    assert_eq!(attention.fman_name, h.fleet.fman_name().to_string());
    assert!(attention.reasons.is_empty());

    // Renewal swaps the hook; the next notification uses it.
    rpc.renew_callback(
        SignedRequest::create(
            &RenewCallbackRequest {
                ts: Timestamp(now_secs()),
                device_id,
                callback: hook("h2/s2"),
                callback_expires_at: Timestamp(now_secs() + 60),
            },
            &key,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert!(
        link.callback
            .unwrap()
            .callback_url()
            .ends_with("/hooks/h2/s2")
    );
    assert_eq!(link.callback_expires_at, now_secs() + 60);

    rpc.unlink_device(
        SignedRequest::create(
            &UnlinkDeviceRequest {
                ts: Timestamp(now_secs()),
                device_id,
            },
            &key,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        rpc.get_attention(signed(&key, device_id))
            .await
            .unwrap_err(),
        GuardianLinkError::NotLinked
    );
    assert!(h.fleet.guardian_link_status().await.unwrap().link.is_none());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn relinking_replaces_the_device_and_test_notifies_it() {
    let h = harness(Some(ORIGIN)).await;
    let (rpc, _, first) = link(&h).await;
    let (_, second_id) = device();
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    rpc.link_device(LinkDeviceRequest {
        version: ProtocolV1,
        secret: invite.secret,
        device_id: second_id,
        device_label: "  iPhone  ".to_owned(),
        callback: hook("h3/s3"),
        callback_expires_at: far_future(),
    })
    .await
    .unwrap();
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(link.device_id, second_id);
    assert_ne!(link.device_id, first);
    assert_eq!(link.device_label, "iPhone");
    assert_eq!(
        h.fleet.test_guardian_link_notification().await.unwrap(),
        CallbackAttemptOutcome::Delivered
    );
    assert_eq!(h.invoker.calls()[0].1, "test");
    // A test is not a reason the phone was told about.
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert!(link.notified_reasons.is_empty());
    assert_eq!(link.notification_seq, 1);
    assert!(h.fleet.revoke_guardian_link().await.unwrap());
    assert!(!h.fleet.revoke_guardian_link().await.unwrap());
    h.fleet.shutdown().await;
}

#[test]
fn device_labels_are_short_and_printable() {
    assert!(validate_device_label("Pixel 8").is_ok());
    assert!(validate_device_label("   ").is_err());
    assert!(validate_device_label("a\nb").is_err());
    assert!(validate_device_label(&"x".repeat(DEVICE_LABEL_MAX_CHARS + 1)).is_err());
}
