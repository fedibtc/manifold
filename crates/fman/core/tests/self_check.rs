use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Hanging {
    entered: tokio::sync::Notify,
    dropped: Arc<AtomicUsize>,
}

struct DropMarker(Arc<AtomicUsize>);

impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ConnectivityChecks for Hanging {
    async fn run(&self, slots: Arc<Mutex<[Check; 6]>>) {
        let _marker = DropMarker(self.dropped.clone());
        slots.lock().unwrap()[0] = Check::new(
            CheckId::DiscoveryDns,
            CheckStatus::Pass,
            ReasonCode::Reached,
        );
        self.entered.notify_one();
        std::future::pending::<()>().await;
    }
}

fn guardian() -> Check {
    Check::new(
        CheckId::GuardianHealth,
        CheckStatus::NotApplicable,
        ReasonCode::NoFormedSeats,
    )
}

fn directory() -> Check {
    directory_check(&OnboardingStatus::RelayError {
        error: "private-host secret\n".into(),
    })
}

#[tokio::test]
async fn bounds_admission_preserves_completed_slots_and_cancels_inflight_work() {
    let adapter = Arc::new(Hanging {
        entered: tokio::sync::Notify::new(),
        dropped: Arc::new(AtomicUsize::new(0)),
    });
    let check = Arc::new(SelfCheck::new(adapter.clone()));
    let accepted = tokio::spawn({
        let check = check.clone();
        async move { check.run(guardian(), directory()).await }
    });
    adapter.entered.notified().await;
    assert!(matches!(
        check.run(guardian(), directory()).await,
        SelfCheckResponse::Busy
    ));
    let completed = tokio::time::timeout(Duration::from_secs(15), accepted)
        .await
        .unwrap()
        .unwrap();
    let SelfCheckResponse::Completed { report } = completed else {
        panic!("expected bounded completion")
    };
    assert_eq!(
        report.checks[0],
        Check::new(
            CheckId::DiscoveryDns,
            CheckStatus::Pass,
            ReasonCode::Reached
        )
    );
    assert_eq!(
        report.checks[1],
        Check::new(
            CheckId::DiscoveryHttps,
            CheckStatus::Unknown,
            ReasonCode::RunDeadline
        )
    );
    assert_eq!(
        report.checks[7],
        Check::new(
            CheckId::DirectoryObservation,
            CheckStatus::Unknown,
            ReasonCode::PreviousRelayError
        )
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("private-host")
    );
    assert_eq!(adapter.dropped.load(Ordering::SeqCst), 1);
    assert!(matches!(
        check.run(guardian(), directory()).await,
        SelfCheckResponse::Cooldown
    ));
}

#[tokio::test]
async fn shutdown_cancels_run_and_rejects_new_admission() {
    let adapter = Arc::new(Hanging {
        entered: tokio::sync::Notify::new(),
        dropped: Arc::new(AtomicUsize::new(0)),
    });
    let check = Arc::new(SelfCheck::new(adapter.clone()));
    let accepted = tokio::spawn({
        let check = check.clone();
        async move { check.run(guardian(), directory()).await }
    });
    adapter.entered.notified().await;
    check.shutdown().await;
    assert!(matches!(
        accepted.await.unwrap(),
        SelfCheckResponse::Unavailable
    ));
    assert!(matches!(
        check.run(guardian(), directory()).await,
        SelfCheckResponse::Unavailable
    ));
    assert_eq!(adapter.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn parameterized_request_cannot_select_targets() {
    assert!(
        serde_json::from_value::<crate::admin::AdminRequest>(
            serde_json::json!({"RunSelfCheck":{"url":"http://private-host/"}})
        )
        .is_err()
    );
}
