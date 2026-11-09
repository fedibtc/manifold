use std::sync::Mutex;

use fedi_decentralized_service_manifold_guardian::DkgCompletionCallbackInput;
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
use fedi_decentralized_domain::AdmittedSetupPaymentFederations;

const ORIGIN: &str = "https://push.example.com";

/// Records every invocation and answers with whatever outcome is queued.
struct RecordingInvoker {
    calls: Mutex<Vec<String>>,
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

    /// The idempotency key of each call so far.
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for RecordingInvoker {
    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        self.calls
            .lock()
            .unwrap()
            .push(callback.idempotency_key().to_owned());
        *self.outcome.lock().unwrap()
    }
}

struct Harness {
    fleet: Arc<Fleet>,
    invoker: Arc<RecordingInvoker>,
    _temp: TempDir,
}

async fn harness(origin: Option<&str>) -> Harness {
    let temp = TempDir::new().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    crate::onboarding::onboard_as_new(&db).await.unwrap();
    db.complete_onboarding_for_test(Some(1)).await.unwrap();
    let invoker = RecordingInvoker::new();
    let fleet = Fleet::open(
        db,
        FleetConfig {
            transfer_source: Arc::new(crate::transfer_source::UnavailableTransferSource),
            replacement_guardian_verifier: Arc::new(
                crate::replacement_guardian::UnverifiedReplacementGuardian,
            ),
            guardian_config_validator: Arc::new(crate::transfer_native::RejectGuardianConfig),
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
    Harness {
        fleet,
        invoker,
        _temp: temp,
    }
}

/// A readiness report whose checks pass except those named.
fn readiness(failing: &[&str]) -> ReadinessReport {
    let outcome = |check: &str, failed: ReadinessOutcome| {
        if failing.contains(&check) {
            failed
        } else {
            ReadinessOutcome::Pass
        }
    };
    ReadinessReport {
        checked_at_ms: 1,
        relay: outcome("relay", ReadinessOutcome::RelayDisconnected),
        discovery: outcome("discovery", ReadinessOutcome::DiscoveryRecordMissing),
        bitcoin: outcome("bitcoin", ReadinessOutcome::BitcoinSyncing),
        version: outcome("version", ReadinessOutcome::MgVersionTooOld),
        uptime: outcome("uptime", ReadinessOutcome::DayUptimeLow),
        uptime_comparison: None,
    }
}

/// A setup-payment policy that recommends `recommended`, if any.
fn policy(recommended: Option<&str>) -> AdmittedSetupPaymentFederations {
    let field = recommended
        .map(|version| format!(r#","recommended_min_mg_version":"{version}""#))
        .unwrap_or_default();
    AdmittedSetupPaymentFederations::parse(
        format!(
            r#"{{"version":1,"fman_version":"99.0.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
        .as_bytes(),
    )
    .unwrap()
}

async fn support_message(fleet: &Fleet, id: char) {
    fleet
        .db()
        .record_support_message(&crate::db::SupportRow {
            rumor_id: id.to_string().repeat(64),
            from_fedi: true,
            body: "hello".to_owned(),
            created_at: 1,
            unread: true,
        })
        .await
        .unwrap();
}

fn hook(path: &str) -> DkgCompletionCallback {
    DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{ORIGIN}/hooks/{path}"),
        idempotency_key: "unused".to_owned(),
    })
    .unwrap()
}

/// Link a phone through the RPC, the way the app does.
async fn link(harness: &Harness) -> GuardianLinkRpc {
    let rpc = GuardianLinkRpc::new(harness.fleet.clone());
    let invite = harness.fleet.create_guardian_link_offer().unwrap();
    rpc.link_device(LinkDeviceRequest {
        version: ProtocolV1,
        secret: invite.secret,
        device_label: "Pixel".to_owned(),
        callback: hook("h1/s1"),
    })
    .await
    .unwrap();
    rpc
}

#[tokio::test]
async fn reasons_are_failing_health_checks_a_recommended_update_and_unread_support() {
    let h = harness(Some(ORIGIN)).await;
    // Before the first readiness run nothing is known, so nothing fails.
    let reasons = h.fleet.attention_reasons(None).await.unwrap();
    assert!(reasons.is_empty(), "{reasons:?}");
    h.fleet
        .set_seat_readiness(readiness(&["discovery", "uptime"]))
        .await
        .unwrap();
    support_message(&h.fleet, '1').await;
    // This release (the workspace's) is below 99.0.0 and above 0.0.1.
    let reasons = h
        .fleet
        .attention_reasons(Some(&policy(Some("99.0.0"))))
        .await
        .unwrap();
    assert_eq!(
        reasons,
        BTreeSet::from([
            AttentionReason::DiscoveryCheckFailed,
            AttentionReason::UptimeCheckFailed,
            AttentionReason::UpdateRecommended,
            AttentionReason::SupportMessage,
        ])
    );
    h.fleet
        .db()
        .mark_support_read(&["1".repeat(64)])
        .await
        .unwrap();
    h.fleet.set_seat_readiness(readiness(&[])).await.unwrap();
    for current in [policy(Some("0.0.1")), policy(None)] {
        let reasons = h.fleet.attention_reasons(Some(&current)).await.unwrap();
        assert!(reasons.is_empty(), "{reasons:?}");
    }
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn notifier_sends_once_per_new_reason_and_again_after_it_clears() {
    let h = harness(Some(ORIGIN)).await;
    link(&h).await;
    let mut backoff = None;
    h.fleet
        .set_seat_readiness(readiness(&["relay"]))
        .await
        .unwrap();
    assert!(tick(&h.fleet, None, &mut backoff).await.unwrap());
    // Same reasons again: silence.
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    // A second reason joins: one more send carries both.
    support_message(&h.fleet, '1').await;
    assert!(tick(&h.fleet, None, &mut backoff).await.unwrap());
    // The relay recovers; nothing new, so nothing is sent, but the memory
    // narrows so a relapse is told again.
    h.fleet.set_seat_readiness(readiness(&[])).await.unwrap();
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    h.fleet
        .set_seat_readiness(readiness(&["relay"]))
        .await
        .unwrap();
    assert!(tick(&h.fleet, None, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    let calls = h.invoker.calls();
    let linked = link.linked_at_ms;
    assert_eq!(
        calls,
        vec![
            format!("guardian-link:{linked}:0"),
            format!("guardian-link:{linked}:1"),
            format!("guardian-link:{linked}:2"),
        ]
    );
    assert_eq!(
        link.notified_reasons,
        vec![
            AttentionReason::RelayCheckFailed,
            AttentionReason::SupportMessage
        ]
    );
    assert_eq!(link.notification_seq, 3);
    assert!(link.last_notified_at_ms.is_some());
    h.fleet.shutdown().await;
}

/// Waits until the invoker has seen `count` calls, or fails after a few
/// seconds.
async fn calls_reach(h: &Harness, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.invoker.calls().len() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{count} calls, saw {:?}", h.invoker.calls()));
}

#[tokio::test]
async fn the_running_notifier_wakes_on_each_source_and_not_on_a_timer() {
    let h = harness(Some(ORIGIN)).await;
    let (publish, policies) = watch::channel(None);
    let notifier = spawn_notifier(h.fleet.clone(), policies);
    link(&h).await;
    // Linked with nothing wrong: the notifier stays quiet.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.invoker.calls().is_empty());
    support_message(&h.fleet, '1').await;
    calls_reach(&h, 1).await;
    h.fleet
        .set_seat_readiness(readiness(&["bitcoin"]))
        .await
        .unwrap();
    calls_reach(&h, 2).await;
    publish.send_replace(Some(policy(Some("99.0.0"))));
    calls_reach(&h, 3).await;
    // With every reason told and no source changing, nothing more is sent.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.invoker.calls().len(), 3);
    notifier.shutdown().await;
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn notifier_retries_transient_failures_and_parks_terminal_ones() {
    let h = harness(Some(ORIGIN)).await;
    link(&h).await;
    h.fleet
        .set_seat_readiness(readiness(&["relay"]))
        .await
        .unwrap();
    let mut backoff = None;
    h.invoker.answer(CallbackAttemptOutcome::Retryable(
        CompletionCallbackReason::Network,
    ));
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    // Inside the backoff window the hook is left alone.
    backoff.as_mut().unwrap().1 = tokio::time::Instant::now() + Duration::from_secs(60);
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    assert_eq!(h.invoker.calls().len(), 1);
    backoff.as_mut().unwrap().1 = tokio::time::Instant::now();
    h.invoker.answer(CallbackAttemptOutcome::Delivered);
    assert!(tick(&h.fleet, None, &mut backoff).await.unwrap());
    // The retry reuses the sequence number, so the gateway deduplicates.
    let calls = h.invoker.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert!(backoff.is_none());

    // A terminal rejection parks the link until the phone relinks.
    support_message(&h.fleet, '1').await;
    h.invoker.answer(CallbackAttemptOutcome::Terminal(
        CompletionCallbackReason::HookExpiredOrRevoked,
    ));
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(
        link.delivery,
        GuardianLinkDelivery::Terminal(CompletionCallbackReason::HookExpiredOrRevoked)
    );
    assert!(link.callback.is_none());
    h.invoker.answer(CallbackAttemptOutcome::Delivered);
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    assert_eq!(h.invoker.calls().len(), 3);
    assert!(h.fleet.test_guardian_link_notification().await.is_err());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn an_inactive_phone_keeps_being_retried_and_recovers_on_delivery() {
    let h = harness(Some(ORIGIN)).await;
    link(&h).await;
    h.fleet
        .set_seat_readiness(readiness(&["relay"]))
        .await
        .unwrap();
    let mut backoff = None;
    h.invoker.answer(CallbackAttemptOutcome::Retryable(
        CompletionCallbackReason::TargetInactive,
    ));
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(link.delivery, GuardianLinkDelivery::Inactive);
    assert!(link.callback.is_some(), "the hook is kept for the retry");
    assert!(link.notified_reasons.is_empty(), "nothing was told yet");

    // A test alert is still allowed and reports the same.
    assert_eq!(
        h.fleet.test_guardian_link_notification().await.unwrap(),
        CallbackAttemptOutcome::Retryable(CompletionCallbackReason::TargetInactive)
    );

    // The app re-registered: the pending alert gets through on the retry.
    backoff.as_mut().unwrap().1 = tokio::time::Instant::now();
    h.invoker.answer(CallbackAttemptOutcome::Delivered);
    assert!(tick(&h.fleet, None, &mut backoff).await.unwrap());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(link.delivery, GuardianLinkDelivery::Active);
    assert_eq!(link.notified_reasons, [AttentionReason::RelayCheckFailed]);

    // An inactive phone can still end for good.
    support_message(&h.fleet, '1').await;
    h.invoker.answer(CallbackAttemptOutcome::Retryable(
        CompletionCallbackReason::TargetInactive,
    ));
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    backoff.as_mut().unwrap().1 = tokio::time::Instant::now();
    h.invoker.answer(CallbackAttemptOutcome::Terminal(
        CompletionCallbackReason::HookExpiredOrRevoked,
    ));
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    assert_eq!(
        h.fleet
            .db()
            .guardian_link()
            .await
            .unwrap()
            .unwrap()
            .delivery,
        GuardianLinkDelivery::Terminal(CompletionCallbackReason::HookExpiredOrRevoked)
    );
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn link_needs_the_open_offer_and_a_hook_under_the_gateway() {
    let h = harness(Some(ORIGIN)).await;
    let rpc = GuardianLinkRpc::new(h.fleet.clone());
    let request = |secret: LinkSecret, callback: DkgCompletionCallback| LinkDeviceRequest {
        version: ProtocolV1,
        secret,
        device_label: "Pixel".to_owned(),
        callback,
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
    rpc.link_device(request(invite.secret.clone(), hook("h/s")))
        .await
        .unwrap();
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
async fn relinking_replaces_the_phone_and_test_notifies_it() {
    let h = harness(Some(ORIGIN)).await;
    let rpc = link(&h).await;
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    rpc.link_device(LinkDeviceRequest {
        version: ProtocolV1,
        secret: invite.secret,
        device_label: "  iPhone  ".to_owned(),
        callback: hook("h3/s3"),
    })
    .await
    .unwrap();
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(link.device_label, "iPhone");
    assert!(
        link.callback
            .unwrap()
            .callback_url()
            .ends_with("/hooks/h3/s3")
    );
    assert_eq!(
        h.fleet.test_guardian_link_notification().await.unwrap(),
        CallbackAttemptOutcome::Delivered
    );
    assert_eq!(h.invoker.calls().len(), 1);
    // A test is not a reason the phone was told about.
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert!(link.notified_reasons.is_empty());
    assert_eq!(link.notification_seq, 1);
    assert!(h.fleet.revoke_guardian_link().await.unwrap());
    assert!(!h.fleet.revoke_guardian_link().await.unwrap());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn an_outcome_for_the_replaced_phone_leaves_the_new_link_alone() {
    let h = harness(Some(ORIGIN)).await;
    let rpc = link(&h).await;
    let old = h.fleet.db().guardian_link().await.unwrap().unwrap();
    // The phone relinks while a notification to its old hook is in flight. A
    // link is known by its `linked_at_ms`, so let the clock move first.
    tokio::time::sleep(Duration::from_millis(2)).await;
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    rpc.link_device(LinkDeviceRequest {
        version: ProtocolV1,
        secret: invite.secret,
        device_label: "iPhone".to_owned(),
        callback: hook("h3/s3"),
    })
    .await
    .unwrap();
    let told = BTreeSet::from([AttentionReason::SupportMessage]);
    for outcome in [
        CallbackAttemptOutcome::Delivered,
        CallbackAttemptOutcome::Terminal(CompletionCallbackReason::HookExpiredOrRevoked),
    ] {
        h.fleet
            .record_guardian_link_outcome(&old, outcome, &told)
            .await
            .unwrap();
    }
    let new = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(new.device_label, "iPhone");
    assert_eq!(new.delivery, GuardianLinkDelivery::Active);
    assert_eq!(new.notification_seq, 0);
    assert!(new.notified_reasons.is_empty());
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn a_restart_does_not_retell_a_reason_whose_source_is_not_read_yet() {
    // A restarted daemon has the told reasons on disk but no readiness report
    // and maybe no policy yet: unknown is not cleared.
    let h = harness(Some(ORIGIN)).await;
    link(&h).await;
    let linked_at_ms = h
        .fleet
        .db()
        .guardian_link()
        .await
        .unwrap()
        .unwrap()
        .linked_at_ms;
    h.fleet
        .db()
        .set_guardian_link_notified_reasons(
            linked_at_ms,
            &[
                AttentionReason::BitcoinCheckFailed,
                AttentionReason::UpdateRecommended,
            ],
        )
        .await
        .unwrap();
    let mut backoff = None;
    assert!(!tick(&h.fleet, None, &mut backoff).await.unwrap());
    h.fleet
        .set_seat_readiness(readiness(&["bitcoin"]))
        .await
        .unwrap();
    let recommending = policy(Some("99.0.0"));
    assert!(
        !tick(&h.fleet, Some(&recommending), &mut backoff)
            .await
            .unwrap()
    );
    assert!(h.invoker.calls().is_empty());
    // Once read, a source that no longer holds does clear its reason.
    h.fleet.set_seat_readiness(readiness(&[])).await.unwrap();
    assert!(
        !tick(&h.fleet, Some(&policy(None)), &mut backoff)
            .await
            .unwrap()
    );
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert!(
        link.notified_reasons.is_empty(),
        "{:?}",
        link.notified_reasons
    );
    h.fleet.shutdown().await;
}

#[test]
fn device_labels_are_short_and_printable() {
    assert!(validate_device_label("Pixel 8").is_ok());
    assert!(validate_device_label("   ").is_err());
    assert!(validate_device_label("a\nb").is_err());
    assert!(validate_device_label(&"x".repeat(DEVICE_LABEL_MAX_CHARS + 1)).is_err());
}
