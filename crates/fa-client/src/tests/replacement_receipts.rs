use super::*;

async fn candidate_client() -> (TestClient, Arc<MgState>) {
    let (client, _, state, _) = formed_client_for_liquidity().await;
    report_guardian_zero_down(&client).await;
    (client, state)
}

/// Every guardian but the one at seat 0 reports a session count, and none
/// has seen it contribute lately: it becomes a replacement candidate.
async fn report_guardian_zero_down(client: &TestClient) {
    let authority = client
        .post_formed_authority(client.fa_id().unwrap())
        .await
        .unwrap();
    let reports = fedi_decentralized_domain::GuardianReports {
        federation_id: FederationId(
            formation::invite_federation_id(&authority.invite_code)
                .unwrap()
                .to_string(),
        ),
        peers: authority
            .seats
            .iter()
            .map(|seat| PeerId::from(seat.index))
            .collect(),
        sessions: authority
            .seats
            .iter()
            .skip(1)
            .map(|seat| {
                (
                    PeerId::from(seat.index),
                    fedi_decentralized_domain::GuardianSessionReport {
                        session_count: 1000,
                        disconnected: BTreeSet::new(),
                        last_contribution: BTreeMap::from([(PeerId::from(0), Some(1))]),
                    },
                )
            })
            .collect(),
        availability: BTreeMap::new(),
    };
    *client
        .inner
        .ports
        .consensus_reader
        .guardian_reports
        .lock()
        .unwrap() = Some(reports);
}

/// Present `receipt` with an approval of the test destination.
async fn request(
    client: &TestClient,
    receipt: &ReplacementReceipt,
) -> Result<GuardianTransfer, ReplacementStartError> {
    client
        .request_guardian_replacement(
            receipt,
            seat_purchase::destination_approval(PAYMENT_AMOUNT_MSATS),
            Some(payment_federation_id()),
            None,
            seat_purchase::transfer_options(),
        )
        .await
}

/// A receipt starts the guardian transfer of the guardian it names, even
/// after a later check no longer lists that guardian.
#[tokio::test]
async fn a_receipt_starts_the_transfer_of_its_guardian_after_a_refresh() {
    let (_database, _payments, _payment_state, state, client) =
        seat_purchase::formed_paid_client().await;
    report_guardian_zero_down(&client).await;
    let check = client.check_replacement_candidates().await.unwrap();
    let receipt = check.receipt(0).unwrap().clone();
    assert!(check.receipt(1).is_none());
    *client
        .inner
        .ports
        .consensus_reader
        .guardian_reports
        .lock()
        .unwrap() = None;
    let next = client.check_replacement_candidates().await.unwrap();
    assert!(next.receipt(0).is_none());
    // JSON is display-only: it cannot export an approval to reconstruct elsewhere.
    assert!(
        serde_json::to_value(&check)
            .unwrap()
            .get("receipts")
            .is_none()
    );
    state
        .seat_statuses
        .lock()
        .unwrap()
        .insert(seat_purchase::DESTINATION, ServiceStatus::New);

    let transfer = request(&client, &receipt).await.unwrap();
    assert_eq!(transfer.seat_index, 0);
    assert_eq!(transfer.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(client.guardian_transfer().await, Some(transfer));
    assert_eq!(state.pull_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn receipt_cannot_authorize_another_client_or_a_modified_display_target() {
    let (client, _) = candidate_client().await;
    let mut check = client.check_replacement_candidates().await.unwrap();
    check.candidates[0].seat_index = 1; // Caller can edit display data, not approval.
    assert!(check.receipt(1).is_none());
    let receipt = check.receipt(0).unwrap();
    let (other, other_state) = candidate_client().await;
    let creates = other_state.create_records.lock().unwrap().len();
    assert!(matches!(
        request(&other, receipt).await,
        Err(ReplacementStartError::InvalidReceipt)
    ));
    assert_eq!(other.guardian_transfer().await, None);
    assert_eq!(other_state.create_records.lock().unwrap().len(), creates);
}

#[tokio::test]
async fn receipt_is_invalid_after_reopening_the_same_database() {
    let (client, state) = candidate_client().await;
    let check = client.check_replacement_candidates().await.unwrap();
    let receipt = check.receipt(0).unwrap().clone();
    let store = client.inner.store.clone();
    drop(client);
    let (payments, _) = TestPayments::new();
    let reopened = open_client_with_store(store, payments, state, MgConfig::given_away()).await;
    assert!(matches!(
        request(&reopened, &receipt).await,
        Err(ReplacementStartError::InvalidReceipt)
    ));
    assert_eq!(reopened.guardian_transfer().await, None);
    assert!(reopened.observe_replacement_candidates().borrow().is_none());
}

#[tokio::test(start_paused = true)]
async fn replacement_schedule_checks_on_start_hourly_and_when_resumed() {
    let (client, _) = candidate_client().await;
    let mut observations = client.observe_replacement_candidates();
    let start = tokio::time::Instant::now();
    let worker_client = client.clone();
    let worker = tokio::spawn(async move { worker_client.run_replacement_checks().await });
    observations.changed().await.unwrap();
    assert_eq!(start.elapsed(), Duration::ZERO);
    observations.borrow_and_update();
    client.inner.progress.send_replace(client.status());
    tokio::task::yield_now().await;
    assert!(!observations.has_changed().unwrap());
    tokio::time::advance(Duration::from_secs(3599)).await;
    tokio::task::yield_now().await;
    assert!(!observations.has_changed().unwrap());
    let operation = client.inner.run_guard.lock().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(matches!(
        client.check_replacement_candidates().await,
        Err(FaError::Busy)
    ));
    assert!(observations.borrow().is_some());
    assert!(!observations.has_changed().unwrap());
    drop(operation);
    observations.changed().await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(3601));
    observations.borrow_and_update();
    worker.abort();
    let _ = worker.await;
    tokio::time::advance(Duration::from_secs(3600)).await;
    assert!(!observations.has_changed().unwrap());
    let worker_client = client.clone();
    let resumed_at = tokio::time::Instant::now();
    let resumed = tokio::spawn(async move { worker_client.run_replacement_checks().await });
    observations.changed().await.unwrap();
    assert_eq!(resumed_at.elapsed(), Duration::ZERO);
    resumed.abort();
    let _ = resumed.await;
}
