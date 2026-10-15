//! Paid seat purchases stopped at every durable FA write.
//!
//! One FA process buys and forms a federation over a [`CrashStorage`] whose
//! plan stops it before or after its n-th write commit. A second process
//! reopens the same bytes, with the same wallet and MGs, and drives to the
//! end. Every stop must end with each paid seat funded once, and every
//! funded quote presented and either seated or refunded.

use super::crash_db::{CrashPlan, CrashPoint, CrashStorage};
use super::*;

#[derive(Clone, Copy, Debug)]
enum Scenario {
    /// Every MG charges.
    Paid,
    /// MGs 1 and 4 give their seats away, the rest charge.
    Mixed,
    /// Every MG charges and the first quote presented is refused after
    /// payment, so its seat leaves the purchase and its payment is refunded.
    RefusedAfterPayment,
}

impl Scenario {
    fn config(self) -> MgConfig {
        match self {
            Self::Paid | Self::Mixed => MgConfig::paid(),
            Self::RefusedAfterPayment => MgConfig {
                create_behavior: CreateBehavior::RefuseFirstQuote,
                ..MgConfig::paid()
            },
        }
    }

    fn mg_state(self) -> Arc<MgState> {
        let state = Arc::new(MgState::default());
        if matches!(self, Self::Mixed) {
            let mut prices = state.price_overrides_msats.lock().expect("test lock");
            prices.insert(1, 0);
            prices.insert(4, 0);
        }
        state
    }
}

/// [`options`] with timeouts no loaded machine reaches, so every run of a
/// scenario makes the same commits.
fn crash_options() -> Timeouts {
    Timeouts {
        poll: Duration::from_millis(1),
        long: Duration::from_secs(120),
        request: Duration::from_secs(60),
    }
}

/// What one crash run leaves behind, for the invariants.
struct Outcome {
    state: FaState,
    status: FaStatus,
    payments: Arc<PaymentState>,
    mgs: Arc<MgState>,
}

/// Buy and form in a process that follows `plan`. Returns once the work is
/// done or the process stopped at its crash point.
async fn first_process(
    storage: &CrashStorage,
    plan: Arc<CrashPlan>,
    scenario: Scenario,
    payments: &TestPayments,
    mgs: &Arc<MgState>,
) {
    let client = open_client(
        storage.open(plan.clone()),
        payments.clone(),
        mgs.clone(),
        scenario.config(),
    )
    .await;
    let work = async {
        let preview = client.preview_at(&intent(), locators());
        // A post-payment refusal ends the purchase step with an error; the
        // reopened process finishes what is left.
        if client
            .create_and_buy(&preview, Some(payment_federation_id()), crash_options())
            .await
            .is_ok()
        {
            let _ = client.drive_with_options(crash_options()).await;
        }
    };
    tokio::select! {
        () = work => {}
        () = plan.wait() => {}
    }
}

/// Run one process per crash in `crashes`, each stopping at its commit, then
/// a last process that drives to the end. Returns the outcome and how many
/// commits the first process made.
async fn crash_and_resume(scenario: Scenario, crashes: &[(usize, CrashPoint)]) -> (Outcome, usize) {
    let storage = CrashStorage::default();
    let (payments, payment_state) = TestPayments::new();
    let mgs = scenario.mg_state();
    let first_plan = match crashes.first() {
        Some(&(commit, point)) => CrashPlan::at(commit, point),
        None => CrashPlan::never(),
    };
    first_process(&storage, first_plan.clone(), scenario, &payments, &mgs).await;
    if let Some(crash) = crashes.first() {
        assert!(
            first_plan.crashed(),
            "{scenario:?} never reached commit {crash:?}"
        );
    }
    for &(commit, point) in crashes.iter().skip(1) {
        let plan = CrashPlan::at(commit, point);
        let client = open_client(
            storage.open(plan.clone()),
            payments.clone(),
            mgs.clone(),
            scenario.config(),
        )
        .await;
        tokio::select! {
            _ = async {
                for _ in 0..5 {
                    let _ = client.drive_with_options(crash_options()).await;
                }
            } => {}
            () = plan.wait() => {}
        }
        drop(client);
    }
    let client = open_client(
        storage.open(CrashPlan::never()),
        payments,
        mgs.clone(),
        scenario.config(),
    )
    .await;
    // Drive the purchase, then the formation, retrying step errors (a
    // refusal ending the purchase step, for one) as the run loop would.
    for _ in 0..6 {
        let _ = client.drive_with_options(crash_options()).await;
    }
    (
        Outcome {
            state: client.state(),
            status: client.status(),
            payments: payment_state,
            mgs,
        },
        first_plan.commits(),
    )
}

fn check(scenario: Scenario, crashes: &[(usize, CrashPoint)], outcome: &Outcome) {
    let context = format!("{scenario:?} stopped at {crashes:?}");
    let funded = outcome
        .payments
        .created_quotes
        .lock()
        .expect("test lock")
        .clone();
    let funded_set = funded.iter().copied().collect::<HashSet<_>>();
    assert_eq!(
        funded.len(),
        funded_set.len(),
        "{context}: a quote was funded twice"
    );

    let creates = outcome.mgs.create_records.lock().expect("test lock");
    let seat_quotes = creates
        .iter()
        .map(|record| (record.seat_id.clone(), record.quote_id))
        .collect::<HashMap<_, _>>();
    let seated = match &outcome.status.federation {
        Some(federation) => federation
            .seats
            .iter()
            .map(|seat| seat.seat_id.clone())
            .collect::<Vec<_>>(),
        None => accepted_seats(&outcome.status)
            .into_iter()
            .filter_map(|seat| seat.seat_id)
            .collect(),
    };
    let pooled = seated
        .iter()
        .map(|seat_id| seat_quotes[seat_id])
        .collect::<HashSet<_>>();
    let refunded = outcome
        .payments
        .settled_quotes
        .lock()
        .expect("test lock")
        .clone();

    let quotes = outcome.mgs.quote_records.lock().expect("test lock");
    let free_seats = pooled
        .iter()
        .filter(|quote_id| {
            quotes.iter().any(|record| {
                record.quote_id == **quote_id
                    && matches!(record.plan, Plan::InfiniteBestEffort { price_msats: 0 })
            })
        })
        .count();
    let paid_seats = seated.len() - free_seats;
    // Each pooled paid seat was funded once, and nothing else was funded
    // except what came back as a refund.
    assert_eq!(
        funded_set.intersection(&pooled).count(),
        paid_seats,
        "{context}: pooled paid seats and funded quotes differ"
    );
    for quote in &funded_set {
        assert!(
            pooled.contains(quote) || refunded.contains(quote),
            "{context}: funded quote {quote:?} is neither seated nor refunded"
        );
    }
    let presented = outcome.mgs.allocated_quotes.lock().expect("test lock");
    assert!(
        funded_set.is_subset(&presented),
        "{context}: a funded quote was never presented to its MG"
    );

    // A stop before the creation was durable leaves nothing to resume.
    if matches!(outcome.state, FaState::Idle) {
        assert!(funded.is_empty(), "{context}: an abandoned creation paid");
        return;
    }
    match scenario {
        Scenario::Paid | Scenario::Mixed => {
            assert!(refunded.is_empty(), "{context}: unexpected refund");
            let federation = outcome
                .status
                .federation
                .as_ref()
                .unwrap_or_else(|| panic!("{context}: no federation: {:?}", outcome.status));
            assert_eq!(
                federation.seats.len(),
                usize::from(MIN_FEDERATION_SIZE),
                "{context}"
            );
        }
        Scenario::RefusedAfterPayment => {
            assert_eq!(
                refunded.len(),
                1,
                "{context}: the refused payment comes back once"
            );
            assert_eq!(
                accepted_seats(&outcome.status).len(),
                usize::from(MIN_FEDERATION_SIZE) - 1,
                "{context}: the refused seat left the pool; {:?}",
                outcome.status
            );
            assert!(
                matches!(
                    &outcome.state,
                    FaState::Creating(creation)
                        if matches!(creation.stage, CreationStage::NeedsSeats { count: 1, .. })
                ),
                "{context}: the creation needs the refused seat again; {:?}",
                outcome.state
            );
        }
    }
}

const BOTH: [CrashPoint; 2] = [CrashPoint::BeforeCommit, CrashPoint::AfterCommit];

/// Stop once at every commit, then reopen and finish.
async fn sweep(scenario: Scenario) {
    let (outcome, commits) = crash_and_resume(scenario, &[]).await;
    check(scenario, &[], &outcome);
    assert!(commits > 5, "{scenario:?} made only {commits} commits");
    for commit in 1..=commits {
        for point in BOTH {
            let crashes = [(commit, point)];
            check(
                scenario,
                &crashes,
                &crash_and_resume(scenario, &crashes).await.0,
            );
        }
    }
}

/// Stop at every commit, then stop the reopened process again at one of
/// its first commits, before a third process finishes. The cases run in
/// parallel to fit the test time limit.
async fn sweep_twice(scenario: Scenario) {
    let (_, commits) = crash_and_resume(scenario, &[]).await;
    let mut cases = tokio::task::JoinSet::new();
    for commit in 1..=commits {
        for point in BOTH {
            for again in 1..=3 {
                for again_point in BOTH {
                    cases.spawn(async move {
                        let crashes = [(commit, point), (again, again_point)];
                        let (outcome, _) = crash_and_resume(scenario, &crashes).await;
                        check(scenario, &crashes, &outcome);
                    });
                }
            }
        }
    }
    while let Some(case) = cases.join_next().await {
        case.expect("crash case");
    }
}

#[tokio::test]
async fn a_paid_formation_stopped_at_any_commit_pays_each_seat_once() {
    sweep(Scenario::Paid).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paid_formation_stopped_twice_pays_each_seat_once() {
    sweep_twice(Scenario::Paid).await;
}

#[tokio::test]
async fn a_mixed_free_and_paid_formation_stopped_at_any_commit_pays_each_seat_once() {
    sweep(Scenario::Mixed).await;
}

#[tokio::test]
async fn a_refusal_after_payment_stopped_at_any_commit_refunds_once() {
    sweep(Scenario::RefusedAfterPayment).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_after_payment_stopped_twice_refunds_once() {
    sweep_twice(Scenario::RefusedAfterPayment).await;
}
