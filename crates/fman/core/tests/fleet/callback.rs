use std::sync::atomic::{AtomicUsize, Ordering};

use fedi_decentralized_service_fleet_manager::DkgCompletionCallbackInput;

use super::*;
use crate::push_callback::{
    CallbackAttemptOutcome, CompletionCallbackInvoker, PushGatewayOriginPolicy,
    ValidatedDkgCompletionCallback,
};

const CALLBACK_ORIGIN: &str = "http://127.0.0.1:3000/";

#[derive(Clone)]
struct FakeCallbackGateway {
    calls: Arc<AtomicUsize>,
    idempotency_keys: Arc<std::sync::Mutex<Vec<String>>>,
    responses: Arc<std::sync::Mutex<std::collections::VecDeque<CallbackAttemptOutcome>>>,
}

impl Default for FakeCallbackGateway {
    fn default() -> Self {
        Self::with_responses([
            CallbackAttemptOutcome::Retryable(
                crate::facts::CompletionCallbackReason::GatewayUnavailable,
            ),
            CallbackAttemptOutcome::Delivered,
        ])
    }
}

impl FakeCallbackGateway {
    fn with_responses(responses: impl IntoIterator<Item = CallbackAttemptOutcome>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            idempotency_keys: Arc::new(std::sync::Mutex::new(Vec::new())),
            responses: Arc::new(std::sync::Mutex::new(responses.into_iter().collect())),
        }
    }
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for FakeCallbackGateway {
    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        self.idempotency_keys
            .lock()
            .expect("test callback lock")
            .push(callback.idempotency_key().to_owned());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .expect("test callback responses")
            .pop_front()
            .unwrap_or(CallbackAttemptOutcome::Delivered)
    }
}

async fn start_running_callback_seat(fleet: &Fleet, quote: u8) -> (SeatId, FakeSeatChildHandle) {
    start_running_callback_seat_with_key(fleet, quote, "shared-formation-dkg-complete").await
}

async fn start_running_callback_seat_with_key(
    fleet: &Fleet,
    quote: u8,
    idempotency_key: &str,
) -> (SeatId, FakeSeatChildHandle) {
    let (fi_id, seat_id) = create_free_seat(fleet, quote).await;
    let _seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .configure_fake_child(
            &seat_id,
            FakeApiState {
                complete_dkg: true,
                ..Default::default()
            },
        )
        .await;
    let own_code = dkg_code(fleet, &fi_id, &seat_id, None).await.unwrap();
    let mut codes = vec![own_code];
    codes.extend((1..7).map(|index| bare_dkg_code(endpoint_setup(index))));
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{CALLBACK_ORIGIN}hooks/hook-id/bearer-secret"),
        idempotency_key: idempotency_key.to_owned(),
    })
    .unwrap();
    start_dkg_with_callback(fleet, &fi_id, &seat_id, &codes, &callback)
        .await
        .unwrap();
    (seat_id, fake)
}

#[tokio::test]
async fn running_seat_retries_and_durably_completes_callback_without_fi_polling() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let gateway = FakeCallbackGateway::default();

    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_250).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(gateway.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 25).await;
    let _seat = fleet.seat_by_id(&seat_id).unwrap();
    let _fake = fleet
        .configure_fake_child(
            &seat_id,
            FakeApiState {
                complete_dkg: true,
                ..Default::default()
            },
        )
        .await;
    let own_code = dkg_code(&fleet, &fi_id, &seat_id, None).await.unwrap();
    let mut codes = vec![own_code];
    codes.extend((1..7).map(|index| bare_dkg_code(endpoint_setup(index))));
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{CALLBACK_ORIGIN}hooks/hook-id/bearer-secret"),
        idempotency_key: "shared-formation-dkg-complete".to_owned(),
    })
    .unwrap();

    start_dkg_with_callback(&fleet, &fi_id, &seat_id, &codes, &callback)
        .await
        .unwrap();
    // No GetStatus/GetInviteCode call drives this. The seat's callback worker
    // independently observes running, retries the injected 503, and commits.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let attempt = fleet
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                attempt.status,
                crate::facts::CompletionCallbackStatus::Delivered { .. }
            ) {
                assert!(attempt.callback.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("callback is retried and marked delivered");

    assert_eq!(gateway.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        gateway
            .idempotency_keys
            .lock()
            .expect("test callback lock")
            .as_slice(),
        &[
            "shared-formation-dkg-complete".to_owned(),
            "shared-formation-dkg-complete".to_owned(),
        ]
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 2);

    fleet.shutdown().await;
    drop(fleet);
    let reopened = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        gateway.calls.load(Ordering::SeqCst),
        2,
        "durable delivery marker suppresses restart replay"
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn periodic_scan_delivers_when_every_wake_is_missed() {
    let gateway = FakeCallbackGateway::with_responses([CallbackAttemptOutcome::Delivered]);
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_252).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(gateway.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    let (_fi_id, seat_id) = create_free_seat(&fleet, 45).await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{CALLBACK_ORIGIN}hooks/hook-id/bearer-secret"),
        idempotency_key: "missed-wake".to_owned(),
    })
    .unwrap();
    // Write through the database deliberately, bypassing both seat wake sites.
    fleet
        .db
        .install_completion_callback(&seat_id, Some(&callback))
        .await
        .unwrap();
    fleet
        .db
        .record_formed(&seat_id, &InviteCode("invite".to_owned()))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), async {
        while gateway.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("periodic relational scan is the correctness path");
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn definitive_callback_rejection_is_terminal_and_clears_bearer_across_reopen() {
    let gateway = FakeCallbackGateway::with_responses([CallbackAttemptOutcome::Terminal(
        crate::facts::CompletionCallbackReason::MaxUsesExceeded,
    )]);

    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_254).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(gateway.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (seat_id, _fake) = start_running_callback_seat(&fleet, 26).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let attempt = fleet
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                attempt.status,
                crate::facts::CompletionCallbackStatus::Terminal {
                    reason: crate::facts::CompletionCallbackReason::MaxUsesExceeded,
                    ..
                }
            ) {
                assert!(attempt.callback.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("max-use rejection becomes terminal");
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);

    fleet.shutdown().await;
    drop(fleet);
    let reopened = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    reopened.shutdown().await;
}

#[tokio::test]
async fn missing_origin_blocks_without_network_and_restored_origin_resumes() {
    let gateway = FakeCallbackGateway::default();

    let temp = TempDir::new().unwrap();
    let mut configured = config(&temp, 1, 30_258).await;
    configured.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    configured.completion_callback_invoker = Arc::new(gateway.clone());
    configured.push_callback_retry_interval = Duration::from_millis(500);
    let fleet = open_fleet(configured.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (seat_id, _fake) = start_running_callback_seat(&fleet, 27).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while gateway.calls.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fleet.shutdown().await;
    drop(fleet);

    let mut missing = configured.clone();
    missing.push_gateway_origin = None;
    missing.push_callback_retry_interval = Duration::from_millis(10);
    let blocked = open_fleet(missing, Arc::new(NoWallet)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let attempt = blocked
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                attempt.status,
                crate::facts::CompletionCallbackStatus::OperatorBlocked {
                    reason: crate::facts::CompletionCallbackReason::GatewayOriginMissing,
                    ..
                }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("missing origin becomes operator-blocked");
    let blocked_status = blocked
        .db
        .completion_callback(&seat_id)
        .await
        .unwrap()
        .unwrap()
        .status;
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(
        blocked
            .db
            .completion_callback(&seat_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        blocked_status,
        "multiple timer ticks leave an unchanged runtime/configuration blocked",
    );
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    blocked.shutdown().await;
    drop(blocked);

    configured.push_callback_retry_interval = Duration::from_millis(10);
    let restored = open_fleet(configured, Arc::new(NoWallet)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(
            restored
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            crate::facts::CompletionCallbackStatus::Delivered { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("restored matching origin resumes callback");
    assert!(matches!(
        restored
            .db
            .completion_callback(&seat_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::facts::CompletionCallbackStatus::Delivered { .. }
    ));
    restored.shutdown().await;
}

#[derive(Clone, Default)]
struct GatedCallbackGateway {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for GatedCallbackGateway {
    async fn invoke(&self, _callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        CallbackAttemptOutcome::Delivered
    }
}

#[tokio::test]
async fn decommission_terminalizes_in_flight_callback_and_ignores_late_success() {
    let gateway = GatedCallbackGateway::default();

    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_262).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(gateway.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    let (seat_id, _fake) = start_running_callback_seat(&fleet, 28).await;
    tokio::time::timeout(Duration::from_secs(3), gateway.entered.notified())
        .await
        .expect("callback entered gateway");

    assert!(fleet.decommission_seat(&seat_id).await.unwrap());
    gateway.release.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let attempt = fleet
        .db
        .completion_callback(&seat_id)
        .await
        .unwrap()
        .unwrap();
    assert!(attempt.callback.is_none());
    assert!(matches!(
        attempt.status,
        crate::facts::CompletionCallbackStatus::Terminal {
            reason: crate::facts::CompletionCallbackReason::Decommissioned,
            ..
        }
    ));
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn restart_formation_race_delivers_without_reconstructing_a_dkg_attempt() {
    let gateway = FakeCallbackGateway::with_responses([CallbackAttemptOutcome::Delivered]);
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_264).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(gateway.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    fleet_config.process_spawner =
        SeatProcessSpawner::Fake(Arc::new(FakeSeatProcessSpawner::scripted([vec![
            vec![
                FakeDkgStep::Message(ChildMessage::DkgStarted {}),
                FakeDkgStep::InstallFinalOnStop,
            ],
            vec![FakeDkgStep::Message(ChildMessage::Hello {
                proto: PROTOCOL_VERSION,
                code_version: "fake-fedimintd".to_owned(),
                state: ChildState::AlreadyConfigured {
                    invite_code: invite.0,
                },
            })],
        ]])));
    let fleet = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 39).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("{CALLBACK_ORIGIN}hooks/hook-id/bearer-secret"),
        idempotency_key: "restart-race".to_owned(),
    })
    .unwrap();
    start_dkg_with_callback(&fleet, &fi_id, &seat_id, &codes, &callback)
        .await
        .unwrap();
    assert_eq!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap(),
        ServiceStatus::Running
    );

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let callback = fleet
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                callback.status,
                crate::facts::CompletionCallbackStatus::Delivered { .. }
            ) {
                assert!(callback.callback.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("formed-row reconciliation delivers despite the cleared DKG session");
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    fleet.shutdown().await;
}

struct UnavailableCallbackInvoker;

#[async_trait::async_trait]
impl CompletionCallbackInvoker for UnavailableCallbackInvoker {
    fn is_available(&self) -> bool {
        false
    }

    async fn invoke(&self, _callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        unreachable!("core checks adapter availability before invocation")
    }
}

#[derive(Clone, Default)]
struct BlockingCallbackInvoker {
    active: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
}

struct ActiveInvocation(Arc<AtomicUsize>);

impl Drop for ActiveInvocation {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for BlockingCallbackInvoker {
    async fn invoke(&self, _callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveInvocation(self.active.clone());
        self.started.notify_one();
        std::future::pending().await
    }
}

#[derive(Clone, Default)]
struct SelectiveBlockingInvoker {
    blocked: BlockingCallbackInvoker,
    delivered: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for SelectiveBlockingInvoker {
    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        if callback.idempotency_key() == "blocked" {
            return self.blocked.invoke(callback).await;
        }
        self.delivered.fetch_add(1, Ordering::SeqCst);
        CallbackAttemptOutcome::Delivered
    }
}

#[tokio::test]
async fn hung_callback_does_not_block_another_seat() {
    let invoker = SelectiveBlockingInvoker::default();
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 2, 30_675).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(invoker.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    let (_blocked_seat, _blocked_fake) =
        start_running_callback_seat_with_key(&fleet, 43, "blocked").await;
    tokio::time::timeout(Duration::from_secs(3), invoker.blocked.started.notified())
        .await
        .expect("first seat enters its hung invocation");

    let (delivered_seat, _delivered_fake) =
        start_running_callback_seat_with_key(&fleet, 44, "delivered").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(
            fleet
                .db
                .completion_callback(&delivered_seat)
                .await
                .unwrap()
                .unwrap()
                .status,
            crate::facts::CompletionCallbackStatus::Delivered { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second seat delivers while the first remains hung");
    assert_eq!(invoker.blocked.active.load(Ordering::SeqCst), 1);
    assert_eq!(invoker.delivered.load(Ordering::SeqCst), 1);
    fleet.shutdown().await;
    assert_eq!(invoker.blocked.active.load(Ordering::SeqCst), 0);
}

#[derive(Clone, Default)]
struct PanickingThenDeliveredInvoker {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for PanickingThenDeliveredInvoker {
    async fn invoke(&self, _callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("injected callback invoker panic");
        }
        CallbackAttemptOutcome::Delivered
    }
}

#[tokio::test]
async fn panicking_callback_invoker_rearms_retry_and_keeps_lifecycle_live() {
    let invoker = PanickingThenDeliveredInvoker::default();
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_670).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(invoker.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    let (seat_id, _fake) = start_running_callback_seat(&fleet, 37).await;

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let attempt = fleet
                .db
                .completion_callback(&seat_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                attempt.status,
                crate::facts::CompletionCallbackStatus::Delivered { .. }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a panicking invocation is cleared and retried");
    assert_eq!(invoker.calls.load(Ordering::SeqCst), 2);
    assert!(fleet.seat_by_id(&seat_id).unwrap().report().await.is_ok());
    fleet.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_and_joins_callback_before_drop_and_reopen() {
    let blocking = BlockingCallbackInvoker::default();
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_680).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(blocking.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_seat_id, fake) = start_running_callback_seat(&fleet, 38).await;
    tokio::time::timeout(Duration::from_secs(3), blocking.started.notified())
        .await
        .expect("callback invocation starts");
    assert_eq!(blocking.active.load(Ordering::SeqCst), 1);

    tokio::time::timeout(Duration::from_secs(1), fleet.shutdown())
        .await
        .expect("shutdown cancels and joins the callback invocation");
    assert_eq!(
        blocking.active.load(Ordering::SeqCst),
        0,
        "no invocation retains the bearer after shutdown returns"
    );
    drop(fleet);
    drop(fake);

    let resumed = FakeCallbackGateway::with_responses([CallbackAttemptOutcome::Delivered]);
    fleet_config.completion_callback_invoker = Arc::new(resumed.clone());
    let reopened = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while resumed.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reopen owns the sole resumed invocation");
    assert_eq!(blocking.active.load(Ordering::SeqCst), 0);
    reopened.shutdown().await;
}

#[tokio::test]
async fn bare_drop_cancels_callback_and_retains_data_root_lock_until_seat_cleanup() {
    let blocking = BlockingCallbackInvoker::default();
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 1, 30_690).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(blocking.clone());
    fleet_config.push_callback_retry_interval = Duration::from_millis(10);
    let fleet = open_fleet(fleet_config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (seat_id, fake) = start_running_callback_seat(&fleet, 40).await;
    let retained_seat = fleet.seat_by_id(&seat_id).unwrap();
    tokio::time::timeout(Duration::from_secs(3), blocking.started.notified())
        .await
        .expect("callback invocation starts");
    drop(fleet);
    drop(fake);

    assert!(
        Db::open(&fleet_config.process.data_root).await.is_err(),
        "a retained seat keeps its loop and data-root lock alive"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while blocking.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Fleet drop cancels the old invocation");
    drop(retained_seat);

    let resumed = FakeCallbackGateway::with_responses([CallbackAttemptOutcome::Delivered]);
    fleet_config.completion_callback_invoker = Arc::new(resumed.clone());
    let reopened = open_fleet(fleet_config, Arc::new(NoWallet)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while resumed.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reopen starts only after old callback cleanup");
    assert_eq!(blocking.active.load(Ordering::SeqCst), 0);
    reopened.shutdown().await;
}

#[tokio::test]
async fn blocked_callback_decommission_clears_runtime_bearer_and_status() {
    let temp = TempDir::new().unwrap();
    let mut fleet_config = config(&temp, 2, 30_266).await;
    fleet_config.push_gateway_origin = Some(
        crate::push_callback::PushGatewayOrigin::parse(
            CALLBACK_ORIGIN,
            PushGatewayOriginPolicy::AllowInsecureLoopback,
        )
        .unwrap(),
    );
    fleet_config.completion_callback_invoker = Arc::new(UnavailableCallbackInvoker);
    let fleet = open_fleet(fleet_config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();

    // A free/no-callback seat remains fully usable: the failing client
    // constructor is reached only once resumable callback work exists.
    let _no_callback_seat = create_free_seat(&fleet, 29).await;
    let (callback_seat, _fake) = start_running_callback_seat(&fleet, 30).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let attempt = fleet
                .db
                .completion_callback(&callback_seat)
                .await
                .unwrap()
                .unwrap();
            if matches!(
                attempt.status,
                crate::facts::CompletionCallbackStatus::OperatorBlocked {
                    reason: crate::facts::CompletionCallbackReason::HttpClientUnavailable,
                    ..
                }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client construction failure becomes operator-blocked without panic");
    assert!(
        fleet
            .db
            .completion_callback(&callback_seat)
            .await
            .unwrap()
            .unwrap()
            .callback
            .is_some()
    );
    assert!(fleet.decommission_seat(&callback_seat).await.unwrap());
    let callback = fleet
        .db
        .completion_callback(&callback_seat)
        .await
        .unwrap()
        .unwrap();
    assert!(callback.callback.is_none());
    assert!(matches!(
        callback.status,
        crate::facts::CompletionCallbackStatus::Terminal {
            reason: crate::facts::CompletionCallbackReason::Decommissioned,
            ..
        }
    ));
    fleet.shutdown().await;
}
