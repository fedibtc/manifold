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
use fedi_decentralized_manifold_environment::ManifoldEnvironment;

const ORIGIN: &str = "https://push.example.com";

/// A device's Iroh endpoint, fixed by `seed`.
fn device_key(seed: u8) -> iroh::SecretKey {
    iroh::SecretKey::from_bytes(&[seed; 32])
}

/// The connection a phone with key `seed` calls from.
fn caller(seed: u8) -> RpcConnectionContext {
    RpcConnectionContext {
        remote_id: device_key(seed).public(),
    }
}

/// The phone most tests link.
fn phone() -> RpcConnectionContext {
    caller(1)
}

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
    harness_in(ManifoldEnvironment::Development, origin).await
}

async fn harness_in(environment: ManifoldEnvironment, origin: Option<&str>) -> Harness {
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
            manifold_environment: environment,
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
        pressure: ReadinessOutcome::NotApplicable,
        host_pressure: None,
        max_host_pressure: None,
        ordering_latency: ReadinessOutcome::NotApplicable,
        ordering_latency_6h_ms: None,
        max_ordering_latency_6h_ms: None,
        uptime: outcome("uptime", ReadinessOutcome::DayUptimeLow),
        uptime_comparison: None,
    }
}

/// A setup-payment policy that recommends `recommended`, if any.
fn policy(recommended: Option<&str>) -> fedi_decentralized_domain::AdmittedFeaturePolicy {
    let field = recommended
        .map(|version| format!(r#","recommended_min_mg_version":"{version}""#))
        .unwrap_or_default();
    policy_with(&field)
}

/// A setup-payment policy that publishes the `guardian_link` flag.
fn flagged(enabled: bool) -> fedi_decentralized_domain::AdmittedFeaturePolicy {
    policy_with(&format!(r#","guardian_link":{enabled}"#))
}

fn policy_with(field: &str) -> fedi_decentralized_domain::AdmittedFeaturePolicy {
    AdmittedSetupPaymentFederations::parse(
        format!(
            r#"{{"version":1,"fman_version":"99.0.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
        .as_bytes(),
    )
    .unwrap().features().clone()
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
    rpc.link_device(
        phone(),
        LinkDeviceRequest {
            version: ProtocolV1,
            secret: invite.secret,
            device_label: "Pixel".to_owned(),
            callback: hook("h1/s1"),
        },
    )
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
        rpc.link_device(
            phone(),
            request(LinkSecret::from_bytes([7; 32]), hook("h/s"))
        )
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
        rpc.link_device(
            phone(),
            request(LinkSecret::from_bytes([7; 32]), hook("h/s"))
        )
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
        rpc.link_device(phone(), request(invite.secret.clone(), foreign))
            .await
            .unwrap_err(),
        GuardianLinkError::PushGatewayUnusable(_)
    ));
    rpc.link_device(phone(), request(invite.secret.clone(), hook("h/s")))
        .await
        .unwrap();
    // The secret is single use and the offer is gone.
    assert_eq!(
        rpc.link_device(phone(), request(invite.secret, hook("h/s")))
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
async fn production_keeps_phone_link_off_until_the_policy_turns_it_on() {
    let h = harness_in(ManifoldEnvironment::Production, Some(ORIGIN)).await;
    let (publish, policies) = watch::channel(None);
    h.fleet.bind_guardian_link_policy(policies);
    assert_eq!(
        h.fleet.guardian_link_status().await.unwrap().availability,
        GuardianLinkAvailability::Disabled
    );
    assert!(h.fleet.create_guardian_link_offer().is_err());
    publish.send_replace(Some(policy(None)));
    assert!(h.fleet.create_guardian_link_offer().is_err());
    publish.send_replace(Some(flagged(true)));
    assert_eq!(
        h.fleet.guardian_link_status().await.unwrap().availability,
        GuardianLinkAvailability::Ready
    );
    link(&h).await;
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn a_policy_that_turns_phone_link_off_stops_linking_and_alerts() {
    let h = harness(Some(ORIGIN)).await;
    let (publish, policies) = watch::channel(None);
    h.fleet.bind_guardian_link_policy(policies);
    let rpc = link(&h).await;
    let invite = h.fleet.create_guardian_link_offer().unwrap();
    support_message(&h.fleet, '1').await;
    let off = flagged(false);
    publish.send_replace(Some(off.clone()));
    assert_eq!(
        h.fleet.guardian_link_status().await.unwrap().availability,
        GuardianLinkAvailability::Disabled
    );
    // An offer handed out while on is refused once off.
    assert_eq!(
        rpc.link_device(
            phone(),
            LinkDeviceRequest {
                version: ProtocolV1,
                secret: invite.secret,
                device_label: "Other".to_owned(),
                callback: hook("h2/s2"),
            }
        )
        .await
        .unwrap_err(),
        GuardianLinkError::InvalidInvite
    );
    assert!(h.fleet.test_guardian_link_notification().await.is_err());
    let mut backoff = None;
    assert!(!tick(&h.fleet, Some(&off), &mut backoff).await.unwrap());
    assert!(h.invoker.calls().is_empty());
    let link = h.fleet.db().guardian_link().await.unwrap().unwrap();
    assert_eq!(link.device_label, "Pixel");
    assert!(link.notified_reasons.is_empty());
    // Turned back on, the next tick tells what is current.
    assert!(
        tick(&h.fleet, Some(&flagged(true)), &mut backoff)
            .await
            .unwrap()
    );
    assert_eq!(h.invoker.calls().len(), 1);
    h.fleet.shutdown().await;
}

#[tokio::test]
async fn offers_need_a_gateway_and_expire() {
    let h = harness(None).await;
    assert!(h.fleet.create_guardian_link_offer().is_err());
    assert_eq!(
        h.fleet.guardian_link_status().await.unwrap().availability,
        GuardianLinkAvailability::NoPushGateway
    );
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
    rpc.link_device(
        phone(),
        LinkDeviceRequest {
            version: ProtocolV1,
            secret: invite.secret,
            device_label: "  iPhone  ".to_owned(),
            callback: hook("h3/s3"),
        },
    )
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
    rpc.link_device(
        phone(),
        LinkDeviceRequest {
            version: ProtocolV1,
            secret: invite.secret,
            device_label: "iPhone".to_owned(),
            callback: hook("h3/s3"),
        },
    )
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

// The guardian-link forward (`crate::guardian_link_forward`), end to end
// over Iroh against fake web UIs that serve HTTP and a WebSocket.
mod forward {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;

    use axum::extract::ws::{Message as ServerMessage, WebSocketUpgrade};
    use fedi_decentralized_service_manifold_guardian::{
        AnnouncedForwardTarget, ForwardRefusal, ForwardStreamError, ForwardTarget, ForwardTargets,
        GUARDIAN_LINK_FORWARD_ALPN, MAX_FORWARD_STREAMS,
    };
    use futures::{SinkExt as _, StreamExt as _};
    use iroh::endpoint::{
        Connection, ConnectionError, ReadError, ReadToEndError, RecvStream, SendStream, presets,
    };
    use iroh::protocol::Router;
    use iroh::{Endpoint, RelayMode};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::guardian_link_forward::GuardianLinkForward;

    use ForwardTarget::{HostUi, MgDashboard};

    /// A stand-in for a web UI such as the umbrelOS dashboard: answers POSTs
    /// with its `name`, the caller's authorization and the body, and on its
    /// WebSocket pushes one message unasked, then echoes what it gets.
    async fn fake_ui(name: &'static str) -> SocketAddr {
        let app = axum::Router::new()
            .route(
                "/trpc",
                axum::routing::post(
                    move |headers: axum::http::HeaderMap, body: String| async move {
                        let auth = headers
                            .get("authorization")
                            .map(|value| value.to_str().unwrap().to_owned())
                            .unwrap_or_default();
                        format!("{name}|{auth}|{body}")
                    },
                ),
            )
            .route(
                "/ws",
                axum::routing::get(|upgrade: WebSocketUpgrade| async move {
                    upgrade.on_upgrade(|mut socket| async move {
                        socket
                            .send(ServerMessage::Text("pushed".into()))
                            .await
                            .unwrap();
                        while let Some(Ok(message)) = socket.next().await {
                            if let ServerMessage::Text(text) = message {
                                let reply = format!("echo:{}", text.as_str());
                                socket
                                    .send(ServerMessage::Text(reply.into()))
                                    .await
                                    .unwrap();
                            }
                        }
                    })
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    async fn host_ui_only() -> BTreeMap<ForwardTarget, SocketAddr> {
        BTreeMap::from([(HostUi, fake_ui("host-ui").await)])
    }

    async fn endpoint(key: Option<iroh::SecretKey>) -> Endpoint {
        let builder = Endpoint::builder(presets::N0).relay_mode(RelayMode::Disabled);
        match key {
            Some(key) => builder.secret_key(key),
            None => builder,
        }
        .bind()
        .await
        .unwrap()
    }

    /// The MG's router serving only the forward, to `targets`.
    async fn mg(fleet: &Arc<Fleet>, targets: BTreeMap<ForwardTarget, SocketAddr>) -> Router {
        Router::builder(endpoint(None).await)
            .accept(
                GUARDIAN_LINK_FORWARD_ALPN,
                GuardianLinkForward::new(fleet.clone(), targets),
            )
            .spawn()
    }

    /// Dial the MG as the phone with key `seed`.
    async fn dial(router: &Router, seed: u8) -> (Endpoint, Connection) {
        let phone = endpoint(Some(device_key(seed))).await;
        let connection = phone
            .connect(router.endpoint().addr(), GUARDIAN_LINK_FORWARD_ALPN)
            .await
            .unwrap();
        (phone, connection)
    }

    /// A stream that starts with `header`.
    async fn open_raw(
        connection: &Connection,
        header: &[u8],
    ) -> Result<(SendStream, RecvStream), ConnectionError> {
        let (mut send, recv) = connection.open_bi().await?;
        // The MG may already have reset the stream; a read reports it.
        let _ = send.write_all(header).await;
        Ok((send, recv))
    }

    /// One HTTP/1.1 exchange on a fresh stream to `target`; the response as
    /// text.
    async fn http(
        connection: &Connection,
        target: ForwardTarget,
        request: &str,
    ) -> Result<String, ReadError> {
        exchange(connection, &target.encode_header(), request).await
    }

    async fn exchange(
        connection: &Connection,
        header: &[u8],
        request: &str,
    ) -> Result<String, ReadError> {
        let (mut send, mut recv) = open_raw(connection, header).await?;
        let _ = send.write_all(request.as_bytes()).await;
        match recv.read_to_end(1 << 20).await {
            Ok(response) => Ok(String::from_utf8(response).unwrap()),
            Err(ReadToEndError::Read(error)) => Err(error),
            Err(ReadToEndError::TooLong) => panic!("response over 1 MiB"),
        }
    }

    async fn websocket(
        connection: &Connection,
        target: ForwardTarget,
    ) -> tokio_tungstenite::WebSocketStream<tokio::io::Join<RecvStream, SendStream>> {
        let (send, recv) = open_raw(connection, &target.encode_header()).await.unwrap();
        tokio_tungstenite::client_async("ws://host/ws", tokio::io::join(recv, send))
            .await
            .unwrap()
            .0
    }

    fn post(body: &str) -> String {
        format!(
            "POST /trpc HTTP/1.1\r\nHost: host\r\nAuthorization: Bearer t0k\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn reset_code(result: Result<String, ReadError>) -> Option<ForwardStreamError> {
        match result {
            Err(ReadError::Reset(code)) => ForwardStreamError::from_code(code.into_inner()),
            other => panic!("expected a reset, got {other:?}"),
        }
    }

    /// The refusal the MG closed `connection` with.
    async fn refusal(connection: &Connection) -> Option<ForwardRefusal> {
        match tokio::time::timeout(Duration::from_secs(10), connection.closed()).await {
            Ok(ConnectionError::ApplicationClosed(close)) => {
                ForwardRefusal::from_code(close.error_code.into_inner())
            }
            _ => None,
        }
    }

    /// The targets the MG announced on `connection`.
    async fn announced(connection: &Connection) -> ForwardTargets {
        let mut recv = connection.accept_uni().await.unwrap();
        serde_json::from_slice(&recv.read_to_end(4096).await.unwrap()).unwrap()
    }

    /// Link the phone with key `seed` through the RPC.
    async fn link_as(harness: &Harness, seed: u8) {
        let invite = harness.fleet.create_guardian_link_offer().unwrap();
        GuardianLinkRpc::new(harness.fleet.clone())
            .link_device(
                caller(seed),
                LinkDeviceRequest {
                    version: ProtocolV1,
                    secret: invite.secret,
                    device_label: format!("Phone {seed}"),
                    callback: hook("h1/s1"),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn each_stream_reaches_the_target_it_names_over_http_and_websocket() {
        let h = harness(Some(ORIGIN)).await;
        link_as(&h, 1).await;
        let host_ui = fake_ui("host-ui").await;
        let mg_dashboard = fake_ui("mg-dashboard").await;
        let router = mg(
            &h.fleet,
            BTreeMap::from([(MgDashboard, mg_dashboard), (HostUi, host_ui)]),
        )
        .await;
        let (_phone, connection) = dial(&router, 1).await;

        // The MG says what it serves, host UI first, with each one's port.
        assert_eq!(
            announced(&connection).await.targets,
            [
                AnnouncedForwardTarget {
                    name: "host-ui".into(),
                    port: host_ui.port()
                },
                AnnouncedForwardTarget {
                    name: "mg-dashboard".into(),
                    port: mg_dashboard.port()
                },
            ]
        );

        let response = http(&connection, HostUi, &post(r#"{"x":1}"#))
            .await
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.ends_with(r#"host-ui|Bearer t0k|{"x":1}"#),
            "{response}"
        );
        let response = http(&connection, MgDashboard, &post("y")).await.unwrap();
        assert!(
            response.ends_with("mg-dashboard|Bearer t0k|y"),
            "{response}"
        );

        let mut socket = websocket(&connection, MgDashboard).await;
        // The UI speaks first: a live update, not an answer.
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text("pushed".into())
        );
        socket.send(Message::Text("hi".into())).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text("echo:hi".into())
        );
        // HTTP keeps working beside the open socket.
        let response = http(&connection, HostUi, &post("again")).await.unwrap();
        assert!(response.ends_with("host-ui|Bearer t0k|again"), "{response}");

        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn a_stream_naming_no_served_target_is_reset_and_the_connection_lives() {
        let h = harness(Some(ORIGIN)).await;
        link_as(&h, 1).await;
        let router = mg(&h.fleet, host_ui_only().await).await;
        let (_phone, connection) = dial(&router, 1).await;
        // A known target this MG was not given, unknown names, and
        // malformed headers.
        let mut headers = vec![
            MgDashboard.encode_header(),
            b"\x07HOST-UI".to_vec(),
            b"\x0410.0".to_vec(),
            b"\x00".to_vec(),
        ];
        let mut long = vec![33u8];
        long.extend_from_slice(&[b'a'; 33]);
        headers.push(long);
        for header in headers {
            assert_eq!(
                reset_code(exchange(&connection, &header, &post("x")).await),
                Some(ForwardStreamError::UnknownTarget),
                "{header:?}"
            );
        }
        // A stream that stops partway through its header is reset too.
        let (_send, mut recv) = open_raw(&connection, b"\x07host").await.unwrap();
        match tokio::time::timeout(Duration::from_secs(15), recv.read_to_end(16)).await {
            Ok(Err(ReadToEndError::Read(ReadError::Reset(code)))) => assert_eq!(
                ForwardStreamError::from_code(code.into_inner()),
                Some(ForwardStreamError::UnknownTarget)
            ),
            other => panic!("expected a reset, got {other:?}"),
        }
        assert!(connection.close_reason().is_none());
        assert!(
            http(&connection, HostUi, &post("x"))
                .await
                .unwrap()
                .ends_with("|x")
        );
        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn any_other_endpoint_is_refused_before_a_stream_is_served() {
        let h = harness(Some(ORIGIN)).await;
        let router = mg(&h.fleet, host_ui_only().await).await;
        // Nothing linked yet: even the phone is a stranger.
        let (_phone, connection) = dial(&router, 1).await;
        assert_eq!(refusal(&connection).await, Some(ForwardRefusal::NotLinked));

        link_as(&h, 1).await;
        let (_stranger, connection) = dial(&router, 2).await;
        assert_eq!(refusal(&connection).await, Some(ForwardRefusal::NotLinked));
        assert!(http(&connection, HostUi, &post("x")).await.is_err());

        // A link made before the MG recorded endpoints forwards to nobody.
        sqlx::query("UPDATE guardian_link SET device_endpoint_id = NULL")
            .execute(h.fleet.db().pool())
            .await
            .unwrap();
        let (_phone, connection) = dial(&router, 1).await;
        assert_eq!(refusal(&connection).await, Some(ForwardRefusal::NotLinked));

        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn relinking_another_phone_closes_the_old_phones_connection() {
        let h = harness(Some(ORIGIN)).await;
        link_as(&h, 1).await;
        let router = mg(&h.fleet, host_ui_only().await).await;
        let (_old, old) = dial(&router, 1).await;
        assert!(http(&old, HostUi, &post("x")).await.is_ok());

        link_as(&h, 2).await;
        assert_eq!(refusal(&old).await, Some(ForwardRefusal::NotLinked));
        let (_new, new) = dial(&router, 2).await;
        assert!(
            http(&new, HostUi, &post("x"))
                .await
                .unwrap()
                .ends_with("|x")
        );

        h.fleet.revoke_guardian_link().await.unwrap();
        assert_eq!(refusal(&new).await, Some(ForwardRefusal::NotLinked));

        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn the_policy_flag_turns_the_forward_off_and_on() {
        let h = harness(Some(ORIGIN)).await;
        let (publish, policies) = watch::channel(None);
        h.fleet.bind_guardian_link_policy(policies);
        link_as(&h, 1).await;
        let router = mg(&h.fleet, host_ui_only().await).await;
        let (_phone, open) = dial(&router, 1).await;
        assert!(http(&open, HostUi, &post("x")).await.is_ok());

        // Turning it off closes the open connection and refuses new ones.
        publish.send_replace(Some(flagged(false)));
        assert_eq!(refusal(&open).await, Some(ForwardRefusal::Disabled));
        let (_phone, connection) = dial(&router, 1).await;
        assert_eq!(refusal(&connection).await, Some(ForwardRefusal::Disabled));
        // Off, a stranger still learns only that it is not linked.
        let (_stranger, connection) = dial(&router, 2).await;
        assert_eq!(refusal(&connection).await, Some(ForwardRefusal::NotLinked));

        publish.send_replace(Some(flagged(true)));
        let (_phone, connection) = dial(&router, 1).await;
        assert!(http(&connection, HostUi, &post("x")).await.is_ok());

        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn without_a_target_or_a_reachable_one_the_phone_is_told_so() {
        let h = harness(Some(ORIGIN)).await;
        link_as(&h, 1).await;
        let router = mg(&h.fleet, BTreeMap::new()).await;
        let (_phone, connection) = dial(&router, 1).await;
        assert_eq!(
            refusal(&connection).await,
            Some(ForwardRefusal::Unavailable)
        );
        router.shutdown().await.unwrap();

        // A port nothing listens on: the stream is reset, the connection lives.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let nowhere = closed.local_addr().unwrap();
        drop(closed);
        let router = mg(&h.fleet, BTreeMap::from([(HostUi, nowhere)])).await;
        let (_phone, connection) = dial(&router, 1).await;
        assert_eq!(
            reset_code(http(&connection, HostUi, &post("x")).await),
            Some(ForwardStreamError::TargetUnreachable)
        );
        assert!(connection.close_reason().is_none());
        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }

    #[tokio::test]
    async fn streams_beyond_the_cap_are_reset_and_freed_ones_reused() {
        let h = harness(Some(ORIGIN)).await;
        link_as(&h, 1).await;
        let router = mg(&h.fleet, host_ui_only().await).await;
        let (_phone, connection) = dial(&router, 1).await;
        // Hold every permit with an open WebSocket.
        let mut sockets = Vec::new();
        for _ in 0..MAX_FORWARD_STREAMS {
            let mut socket = websocket(&connection, HostUi).await;
            socket.next().await.unwrap().unwrap();
            sockets.push(socket);
        }
        assert_eq!(
            reset_code(http(&connection, HostUi, &post("x")).await),
            Some(ForwardStreamError::TooManyStreams)
        );
        let mut socket = sockets.pop().unwrap();
        socket.close(None).await.unwrap();
        while socket.next().await.is_some() {}
        drop(socket);
        // The freed permit serves the next stream.
        let mut served = false;
        for _ in 0..50 {
            if let Ok(response) = http(&connection, HostUi, &post("x")).await {
                served = response.ends_with("|x");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(served);
        router.shutdown().await.unwrap();
        h.fleet.shutdown().await;
    }
}
