//! The stability-pool funding worker.
//!
//! A stability item advances in four steps — withdraw provider funds, claim the
//! target peg-in, submit `deposit_to_provide`, observe the result — under a
//! deadline consulted between phases rather than a timeout wrapped around the
//! item, so no durable write is dropped mid-flight.

use fedi_decentralized_service_liquidity_manager::{
    CompletionEvidence, LiquidityFailureCode, Sats, ServiceResult, SetupConfigView,
    StabilityPoolCompletionEvidence, WalletOperationStatus,
};

use crate::DaemonContext;
use crate::allocation_funding;
use crate::allocation_store::{
    self, PegInProgress, SpDepositStatus, StabilityPoolAllocationItem, StabilityPoolObservation,
};
use crate::daemon::Worker;
use crate::database::Database;
use crate::stability_deposit::StabilityDepositSubmission;
use crate::stability_pool::{
    FedimintStabilityPoolBackend, PegInAllocation, PegInStatus, StabilityDepositStatus,
    StabilityPoolBackend, StabilityPoolReport, SubmissionReceipt, TargetCheck,
};
use crate::wallet::FundsWallet;
use crate::{
    internal_error, invalid_argument, now_timestamp, run_interval_task, unavailable,
    validate_deposit_address,
};

/// How long one stability item may hold the worker before the pass moves on.
///
/// The worker advances items serially and the interval task awaits the whole
/// pass, so one unresponsive federation would otherwise stop every later
/// allocation and the next tick with it. A target Fedimint client can leave an
/// await pending indefinitely: the pinned client's threshold API-version
/// negotiation backs off with no terminal condition.
///
/// **The budget is a deadline the item consults, not a timeout wrapped around
/// it.** The item's future is never dropped to enforce it, so no await is cut
/// mid-flight. Two mechanisms replace the wrapper:
///
/// - **Phase boundaries.** [`ItemBudget::is_spent`] is consulted between
///   phases, after a durable write and before the next piece of remote work.
///   A spent budget returns whatever the item has already durably advanced, so
///   the item stops in a state the next pass resumes from.
/// - **Bounded observations.** [`ItemBudget::bound_observation`] wraps the
///   outbound calls that read remote state and write nothing durable locally,
///   so a target that never answers cannot hold the pass open forever.
///   Abandoning one loses an observation and nothing else.
///
/// `submit_deposit_to_provide` is deliberately **not** bounded: it leaves state
/// behind in the target client that FLIP must record, so abandoning it
/// mid-flight is precisely the hazard this design removes.
///
/// `ensure_client`, `allocate_peg_in_address`, and `recheck_peg_in` **are**
/// bounded despite not being pure reads, each on its own safety argument stated
/// at its call site. See [`ItemBudget::bound_observation`].
///
/// A `tokio::time::timeout` around the whole item would do the opposite: it
/// drops the item's future at whatever await it is suspended on, including a
/// durable write, so a terminal state can be observed and then lost. Consulting
/// a deadline between phases removes that case rather than enumerating it —
/// there is no await inside a cancellable region that a durable write can be
/// suspended on, because the durable writes are not in a cancellable region at
/// all.
///
/// Cancellation has not left this path entirely. `run_interval_task` selects the
/// pass against the shutdown token, so a shutdown still drops the pass at an
/// arbitrary await. That is one event per process lifetime, resumed from durable
/// state on the next start, and not something a counterparty can aim repeatedly
/// at a chosen write. The two `*_beyond_cancellation` helpers below exist for it.
///
/// Thirty seconds is generous against the work an item legitimately does —
/// several stream drains at five seconds each — and short enough that a stuck
/// target costs one pass rather than all of them.
///
/// The `updated_at` ordering **is** load-bearing. `mark_item_running` stamps the
/// item when its work *begins*, so an item that consumes a whole pass carries a
/// newer stamp than the items that pass never reached, and `active_item_rows`
/// orders oldest first — which is the only reason a slow or permanently failing
/// item cannot starve the queue.
const STABILITY_ITEM_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// The per-item time budget, as a deadline rather than a cancellation.
///
/// See [`STABILITY_ITEM_BUDGET`] for why this is a deadline the item consults
/// and not a `timeout` wrapped around it.
#[derive(Clone, Copy)]
struct ItemBudget {
    deadline: tokio::time::Instant,
}

impl ItemBudget {
    fn starting_now(budget: std::time::Duration) -> Self {
        Self {
            deadline: tokio::time::Instant::now() + budget,
        }
    }

    /// True once the item should stop at its next phase boundary.
    fn is_spent(self) -> bool {
        tokio::time::Instant::now() >= self.deadline
    }

    /// Bounds one outbound call whose abandonment is safe.
    ///
    /// For the backend calls that read remote state and write nothing durable —
    /// locally or in the target client — so that abandoning one loses an
    /// observation and nothing else. An expiry becomes an ordinary error, which
    /// the pass logs and steps past, exactly as it does for an unreachable
    /// target.
    ///
    /// Three calls bounded here are not pure reads. Two carry their own argument
    /// for why abandoning them is safe, at their call sites:
    ///
    /// - `ensure_client` — the pool owns the open, so a cancelled one releases
    ///   the target client's RocksDB lock on the pool's schedule.
    /// - `allocate_peg_in_address` — the mint is idempotent, because it
    ///   allocates from the client's unused-address pool, so an abandoned call
    ///   yields the same address on the next pass.
    /// - `recheck_peg_in` — its only durable effect is a scheduling hint. The
    ///   upstream autocommit writes `next_check_time = now` onto the existing
    ///   tweak-index entry, preserving every other field, and wakes the peg-in
    ///   monitor; it claims nothing and returns no data. A lost call costs
    ///   promptness only, and the next pass repeats it.
    ///
    /// Do not read this helper as licence to bound an effectful call without
    /// such an argument. `submit_deposit_to_provide` moves money and is **not**
    /// bounded; it is guarded by a phase check before it starts instead.
    async fn bound_observation<T>(
        self,
        what: &'static str,
        call: impl std::future::Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        match tokio::time::timeout_at(self.deadline, call).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "{what} did not answer within the item's time budget"
            )),
        }
    }
}

trait StabilityPoolStepExt {
    fn set_peg_in_status(&mut self, status: PegInProgress, amount: Option<Sats>);
}

impl StabilityPoolStepExt for allocation_store::StabilityPoolAllocationStep {
    fn set_peg_in_status(&mut self, status: PegInProgress, amount: Option<Sats>) {
        self.peg_in_status = Some(status);
        if let Some(amount) = amount {
            self.peg_in_amount = Some(amount);
        }
    }
}

pub(crate) async fn run_stability_pool_allocation_task(
    context: DaemonContext,
) -> anyhow::Result<()> {
    run_interval_task(
        context,
        Worker::StabilityPoolAllocation,
        std::time::Duration::from_secs(10),
        "stability-pool allocation processing failed",
        |context| async move { process_stability_pool_allocations(&context).await },
    )
    .await
}

pub(crate) async fn process_stability_pool_allocations(
    context: &DaemonContext,
) -> ServiceResult<usize> {
    let (setup, wallet) = crate::funds_admin::configured_wallet(context).await?;
    let backend = FedimintStabilityPoolBackend::new(
        context.paths.federations_dir.clone(),
        context.target_fedimint_clients.clone(),
        &setup.chain_observer,
    );
    process_stability_pool_allocations_with(&context.database, &setup, &wallet, &backend).await
}

pub(crate) async fn process_stability_pool_allocations_with(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    backend: &impl StabilityPoolBackend,
) -> ServiceResult<usize> {
    process_with_item_budget(database, setup, wallet, backend, STABILITY_ITEM_BUDGET).await
}

/// The pass, with the per-item budget supplied.
///
/// Separate only so a test can drive the budget without waiting real seconds
/// for it: paused time is not an option here, because auto-advancing it fires
/// the SQLite pool's own acquire timeouts.
async fn process_with_item_budget(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    backend: &impl StabilityPoolBackend,
    item_budget: std::time::Duration,
) -> ServiceResult<usize> {
    let items = allocation_store::active_stability_pool_items(database).await?;
    let mut advanced = 0;
    for item in items {
        let item_id = item.item_id.0.clone();
        let federation_id = item.federation_id.0.clone();
        // No `timeout` around this call, deliberately. The item consults its own
        // deadline at phase boundaries and bounds its own observations, so the
        // budget can never drop the item's future on a durable write. See
        // `STABILITY_ITEM_BUDGET`.
        let budget = ItemBudget::starting_now(item_budget);
        match process_stability_pool_item(database, setup, wallet, backend, item, budget).await {
            Ok(true) => advanced += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%federation_id, %item_id, %error, "stability item failed this pass; continuing");
            }
        }
    }
    Ok(advanced)
}

async fn process_stability_pool_item(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    backend: &impl StabilityPoolBackend,
    mut item: StabilityPoolAllocationItem,
    budget: ItemBudget,
) -> ServiceResult<bool> {
    if item.corrupt_step_json.is_some() {
        // Error rather than warning: this is not a dependency refusing FLIP,
        // it is FLIP unable to read what it wrote. Nothing retries it, the
        // item holds its reservation until an operator acts, and the raw row
        // is kept for them to read.
        tracing::error!(
            federation_id = %item.federation_id.0,
            item_id = %item.item_id.0,
            "persisted stability-pool step JSON is unreadable; the raw row is retained"
        );
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::StabilityPoolFailed,
            "persisted stability-pool step JSON is unreadable; raw state retained for operator reconciliation",
        )
        .await?;
        return Ok(true);
    }
    if let Some(operation_id) = item.step.sp_deposit_operation_id.as_deref()
        && crate::stability_deposit::StabilityDepositOperationId::parse(operation_id).is_err()
    {
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::StabilityPoolFailed,
            "persisted stability-pool operation ID is malformed; reconcile the target client",
        )
        .await?;
        return Ok(true);
    }
    if let Err(detail) = item.step.submitting_submission() {
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::StabilityPoolFailed,
            format!("{detail}; reconcile the target client and provider account"),
        )
        .await?;
        return Ok(true);
    }

    if !allocation_store::mark_item_running(database, &item.federation_id, &item.item_id).await? {
        return Ok(false);
    }

    let federation_id = item.federation_id.0.clone();
    let mut advanced = false;
    if item.step.target_client_opened_at.is_none() {
        tracing::info!(%federation_id, "stability: opening target federation client");
        // Bounded, and this is the one effectful call that is. The client open
        // is what hangs when a target's peers stop answering — the pinned
        // client's threshold API-version negotiation backs off with no terminal
        // condition — so leaving it unbounded reinstates the starvation this
        // budget exists to stop.
        //
        // Abandoning it is safe because the *pool* owns the open, not this
        // caller: a cancelled open stays the pool's, so the target client's
        // RocksDB lock is released on the pool's schedule. Without that
        // ownership a cancelled join leaks the lock for the process lifetime and
        // wedges every later pass in an uncancellable `flock`. It is a property
        // of the pool, not of this call site — if the pool ever hands the open
        // to its caller, this bound must go with it.
        //
        // Nothing durable in FLIP depends on the abandoned attempt: the
        // `target_client_opened_at` write below happens after the call returns.
        budget
            .bound_observation("target client open", backend.ensure_client(&item.target))
            .await
            .map_err(unavailable)?;
        tracing::info!(%federation_id, "stability: target federation client opened");
        item.step.target_client_opened_at = Some(now_timestamp());
        allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
        advanced = true;
    }

    // Every tick, before anything that can move provider value. The item's own
    // recorded progress is not a substitute: the client is cached by federation
    // id alone, so a later tick can be looking at a different config than the
    // tick that opened it, and the peg-in address this worker funds is an
    // ordinary wallet address that an unusable target accepts just as readily
    // as a usable one.
    // Phase boundary: the client open above can be slow, and everything past
    // this point either moves provider value or writes durably.
    if budget.is_spent() {
        return Ok(stopped_at_phase_boundary(
            &item,
            "before the target check",
            advanced,
        ));
    }

    match budget
        .bound_observation("target check", backend.check_target(&item.target))
        .await
        .map_err(unavailable)?
    {
        TargetCheck::Usable => {}
        TargetCheck::Unusable(reason) => {
            tracing::warn!(%federation_id, %reason, "stability: refusing to fund target");
            allocation_store::fail_item(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::StabilityPoolFailed,
                &format!("stability-pool target cannot be funded: {reason}"),
            )
            .await?;
            return Ok(true);
        }
    }

    if item.step.peg_in_address.is_none() || item.step.peg_in_operation_id.is_none() {
        // Phase boundary, so a spent budget declines to start the mint rather
        // than reaching it with nothing left.
        if budget.is_spent() {
            return Ok(stopped_at_phase_boundary(
                &item,
                "before allocating a peg-in address",
                advanced,
            ));
        }
        tracing::info!(%federation_id, "stability: allocating peg-in address");
        // Bounded, which is safe only because the mint is idempotent: the
        // backend allocates from the client's unused-address pool, so a call
        // abandoned here returns the same address on the next pass instead of
        // minting a second one. Without that idempotence, bounding it would mint
        // a second peg-in address per abandoned call; leaving it unbounded would
        // let a target that never answers hold the pass open. See
        // `FedimintStabilityPoolBackend::allocate_peg_in_address`.
        let peg_in = match budget
            .bound_observation(
                "peg-in address allocation",
                backend.allocate_peg_in_address(&item.target),
            )
            .await
            .map_err(unavailable)?
        {
            PegInAllocation::Allocated(peg_in) => peg_in,
            // The same decision `check_target` makes, taken on the handle that
            // would have minted the address. Failing the item here rather than
            // erroring keeps it from aborting the pass for every item behind it.
            PegInAllocation::Unusable(reason) => {
                tracing::warn!(%federation_id, %reason, "stability: refusing to fund target");
                allocation_store::fail_item(
                    database,
                    &item.federation_id,
                    &item.item_id,
                    LiquidityFailureCode::StabilityPoolFailed,
                    &format!("stability-pool target cannot be funded: {reason}"),
                )
                .await?;
                return Ok(true);
            }
        };
        // The address is deliberately absent. A target-federation peg-in address
        // is private federation data, and logs are read by people the federation
        // has not authorised. An operator who needs it has
        // `inspect_target_client` on the authenticated Admin API.
        tracing::info!(%federation_id, "stability: peg-in address allocated");
        validate_deposit_address(&peg_in.address, setup.network).map_err(invalid_argument)?;
        item.step.peg_in_operation_id = Some(peg_in.operation_id);
        item.step.peg_in_address = Some(peg_in.address);
        item.step.peg_in_status = Some(PegInProgress::WaitingForTransaction);
        allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
        advanced = true;
    }

    // Phase boundary before the funding send. `submit_funding_withdrawal` moves
    // provider value, so a spent budget declines to begin one rather than
    // risking an abandoned send that wallet reconciliation then has to own.
    if budget.is_spent() {
        return Ok(stopped_at_phase_boundary(
            &item,
            "before the funding wallet operation",
            advanced,
        ));
    }

    let operation = allocation_funding::ensure_wallet_operation(
        database,
        setup,
        &stability_funding_step(&item),
    )
    .await?;
    let Some(operation) = operation else {
        return Ok(false);
    };
    tracing::debug!(%federation_id, status = ?operation.status, "stability: wallet operation status");
    if item.step.wallet_operation_id.as_deref() != Some(operation.operation_id.0.as_str()) {
        item.step.wallet_operation_id = Some(operation.operation_id.0.clone());
        allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
    }
    match operation.status {
        WalletOperationStatus::Pending => {
            allocation_funding::submit_funding_withdrawal(
                database,
                setup,
                wallet,
                &stability_funding_step(&item),
                &operation.operation_id,
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Broadcast | WalletOperationStatus::Confirmed => Ok(advanced),
        WalletOperationStatus::InDoubt | WalletOperationStatus::ManualReviewRequired => {
            Ok(advanced)
        }
        WalletOperationStatus::Failed => {
            allocation_store::require_item_action(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::WithdrawFailed,
                "stability-pool funding wallet operation failed",
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Cancelled => {
            allocation_store::fail_item(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::WithdrawFailed,
                "stability-pool funding wallet operation was cancelled while item was active",
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Completed => {
            advance_after_withdrawal_completed(database, setup, backend, item, budget).await
        }
    }
}

/// Logs an item stopping cleanly because its budget is spent, and reports what
/// it durably advanced before stopping.
///
/// A phase boundary is a point where the item holds no half-finished remote
/// work and its durable state is one the next pass resumes from, so stopping
/// here costs a pass and nothing else.
fn stopped_at_phase_boundary(
    item: &StabilityPoolAllocationItem,
    phase: &'static str,
    advanced: bool,
) -> bool {
    tracing::debug!(
        federation_id = %item.federation_id.0,
        item_id = %item.item_id.0,
        %phase,
        "stability item stopped at a phase boundary: its time budget is spent; \
         resuming on a later pass"
    );
    advanced
}

fn stability_funding_step(
    item: &StabilityPoolAllocationItem,
) -> allocation_funding::FundingStep<'_> {
    allocation_funding::FundingStep {
        kind: allocation_funding::FundingKind::StabilityPool,
        federation_id: &item.federation_id,
        item_id: &item.item_id,
        address: item.step.peg_in_address.as_deref(),
        amount: item.reserved_amount,
    }
}

async fn advance_after_withdrawal_completed(
    database: &Database,
    setup: &SetupConfigView,
    backend: &impl StabilityPoolBackend,
    mut item: StabilityPoolAllocationItem,
    budget: ItemBudget,
) -> ServiceResult<bool> {
    let peg_in_operation_id = item
        .step
        .peg_in_operation_id
        .clone()
        .ok_or_else(|| internal_error("missing stability-pool peg-in operation id"))?;
    let status = budget
        .bound_observation(
            "peg-in observation",
            backend.observe_peg_in(&item.target, &peg_in_operation_id),
        )
        .await
        .map_err(unavailable)?;
    tracing::debug!(
        federation_id = %item.federation_id.0,
        ?status,
        committed = item.committed_amount.0,
        "stability: peg-in observation"
    );

    // Up to and including `WaitingForConfirmation`, the deposit is waiting on the
    // wallet client's background peg-in monitor to claim it, and draining the
    // stream does not drive that monitor — the stream *waits* on the monitor's
    // `ClaimedPegInKey` write. The monitor was woken once when the address was
    // allocated, before the funds existed, and otherwise polls on its own slow
    // schedule, so nudge it while we wait (as aqueduct does). From `Confirmed`
    // on, that key has already been written and the remaining wait is on the
    // primary-module outputs, which the monitor has no part in.
    if matches!(
        status,
        PegInStatus::WaitingForTransaction | PegInStatus::WaitingForConfirmation { .. }
    ) {
        // Bounded, and abandoning it is safe. Its only durable effect is a
        // scheduling hint: `recheck_pegin_address` autocommits
        // `next_check_time = now` onto the existing `PegInTweakIndexData` with
        // `..db_val` preserving every other field, then wakes the monitor on
        // commit. It claims nothing, moves no value, and mutates no operation
        // state.
        //
        // So both abandonment points are safe. Before the commit, nothing is
        // written and the monitor keeps its own slower schedule. After it,
        // the write has landed and the call returns no data FLIP records —
        // its result is `()`. It is idempotent besides: a repeat only writes a
        // newer `now`. A lost call therefore costs promptness, and the next
        // pass repeats it while the status still matches above.
        budget
            .bound_observation(
                "peg-in recheck",
                backend.recheck_peg_in(&item.target, &peg_in_operation_id),
            )
            .await
            .map_err(unavailable)?;
    }

    match status {
        PegInStatus::WaitingForTransaction => {
            item.step
                .set_peg_in_status(PegInProgress::WaitingForTransaction, None);
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            Ok(true)
        }
        PegInStatus::WaitingForConfirmation { amount } => {
            item.step
                .set_peg_in_status(PegInProgress::WaitingForConfirmation, Some(amount));
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            Ok(true)
        }
        PegInStatus::Confirmed { amount } => {
            item.step
                .set_peg_in_status(PegInProgress::Confirmed, Some(amount));
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            Ok(true)
        }
        PegInStatus::Claimed { amount } => {
            item.step
                .set_peg_in_status(PegInProgress::Claimed, Some(amount));
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            advance_stability_deposit(database, setup, backend, item, amount, budget).await
        }
        PegInStatus::Failed(reason) => {
            // The funding withdrawal has already completed, so provider BTC is
            // on-chain at the peg-in address and a failed claim strands it.
            // Terminal `Failed` would hide that from the operator; surface it
            // for reconciliation instead, as the gateway path does for every
            // post-send failure.
            // Beyond cancellation for the same reason the success arm is: this
            // is the only durable effect of a *failed peg-in claim*, and losing
            // it leaves the item nonterminal for another run of responsive
            // invocations.
            require_item_action_beyond_cancellation(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::StabilityPoolFailed,
                reason,
            )
            .await?;
            Ok(true)
        }
    }
}

async fn advance_stability_deposit(
    database: &Database,
    setup: &SetupConfigView,
    backend: &impl StabilityPoolBackend,
    item: StabilityPoolAllocationItem,
    claimed_amount: Sats,
    budget: ItemBudget,
) -> ServiceResult<bool> {
    advance_stability_deposit_with(
        database,
        setup,
        backend,
        item,
        claimed_amount,
        budget,
        StabilityDepositSubmission::generate,
    )
    .await
}

async fn advance_stability_deposit_with(
    database: &Database,
    setup: &SetupConfigView,
    backend: &impl StabilityPoolBackend,
    mut item: StabilityPoolAllocationItem,
    claimed_amount: Sats,
    budget: ItemBudget,
    generate_submission: impl FnOnce(Sats, u64) -> StabilityDepositSubmission,
) -> ServiceResult<bool> {
    // Whether the CAS below committed this pass, so a phase-boundary stop after
    // it reports the durable progress it made.
    let mut began_submission = false;
    if item.step.sp_deposit_status != Some(SpDepositStatus::Submitting) {
        if let Some(ref operation_id) = item.step.sp_deposit_operation_id {
            let operation_id =
                crate::stability_deposit::StabilityDepositOperationId::parse(operation_id)
                    .map_err(internal_error)?;
            return observe_stability_deposit(database, backend, item, operation_id, budget).await;
        }

        let wallet_balance = budget
            .bound_observation(
                "target wallet balance",
                backend.target_wallet_balance(&item.target),
            )
            .await
            .map_err(unavailable)?;
        let deposit_amount =
            committed_deposit_amount(item.committed_amount, claimed_amount, wallet_balance);
        if deposit_amount.0 == 0 {
            // The peg-in has already completed, so abandoning this item would
            // strand spendable e-cash in a client no workflow touches again.
            allocation_store::require_item_action(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::StabilityPoolFailed,
                "target client holds less spendable e-cash than this item committed; \
                 reconcile the target client before resuming",
            )
            .await?;
            return Ok(true);
        }

        let expected_step = item.step.clone();
        let candidate = generate_submission(
            deposit_amount,
            setup.funding_policy.stability_pool_min_fee_rate_ppb,
        );
        item.step.begin_submission(candidate);
        if !allocation_store::compare_and_set_item_step(
            database,
            &item.item_id,
            &expected_step,
            &item.step,
        )
        .await?
        {
            return Ok(false);
        }
        began_submission = true;
    }

    let submission = item
        .step
        .submitting_submission()
        .map_err(internal_error)?
        .ok_or_else(|| internal_error("stability deposit is not marked submitting"))?;
    if submission.amount() != item.committed_amount
        || 1_000_000_000 < submission.min_fee_rate_ppb()
        || submission.amount().0.checked_mul(1_000).is_none()
    {
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::StabilityPoolFailed,
            "persisted stability-pool submission parameters are invalid; reconcile the target client",
        )
        .await?;
        return Ok(true);
    }

    // Phase boundary before the deposit. The `submitting` marker, the
    // caller-owned operation id, and the immutable request are all committed by
    // now, so stopping here is resumable: the next pass re-reads this same
    // submission and resubmits under the same id. The submit itself is
    // deliberately unbounded — it moves money into the provider account, and
    // abandoning one mid-flight is what the caller-owned id exists to make
    // survivable rather than something to do on purpose.
    if budget.is_spent() {
        return Ok(stopped_at_phase_boundary(
            &item,
            "before the provider deposit",
            began_submission,
        ));
    }

    // The fence every irreversible call gets: a predecessor compare-and-set in
    // the durable database immediately before it, whose zero-rows result stops
    // the call.
    //
    // The compare-and-set above is not that fence on every pass. It sits inside
    // the block this function skips when the step already reads `submitting` —
    // the resume case, which the phase boundary a few lines up exists to create.
    // On that path the item snapshot was taken when the worker loaded its item
    // list, and the worker iterates serially, so by the time this item's turn
    // comes the snapshot is as old as every preceding item's pass. An operator
    // cancel committed in that interval is not refused: `cancel_allocation`
    // guards on the item's *wallet* operations, and a stability deposit is not
    // one.
    //
    // Re-asserting the same step is what makes the check a database predicate
    // rather than a Rust-side reading of a stale struct: it matches on
    // `item_id`, on the item still being `pending` or `running`, and on
    // `step_json` being exactly what this call is about to act on. Returning
    // `false` leaves the item for the next pass, which re-reads it and resubmits
    // under the same caller-owned operation id.
    if !allocation_store::compare_and_set_item_step(database, &item.item_id, &item.step, &item.step)
        .await?
    {
        tracing::info!(
            federation_id = %item.federation_id.0,
            "stability: item moved while the deposit was being prepared; not submitting"
        );
        return Ok(began_submission);
    }

    // Captured before `submit_deposit_to_provide` consumes the submission. It is
    // the id `begin_submission` just staged on the step.
    let submission_operation_id = submission.operation_id().to_string();
    tracing::info!(
        federation_id = %item.federation_id.0,
        operation_id = %submission.operation_id(),
        deposit_amount = submission.amount().0,
        "stability: submitting idempotent deposit_to_provide"
    );
    let receipt = backend
        .submit_deposit_to_provide(&item.target, submission, &item.item_id.0)
        .await
        .map_err(unavailable)?;
    if receipt == SubmissionReceipt::Conflicting {
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::StabilityPoolFailed,
            "operation ID exists with a conflicting receipt; reconcile the target client",
        )
        .await?;
        return Ok(true);
    }

    // Through the same setter as every other deposit-status write, so the
    // monotonicity is one greppable shape rather than a rule some call sites
    // observe. This one cannot lower anything — `begin_submission` has just
    // staged `submitting` for this id — and goes through it anyway.
    item.step
        .advance_sp_deposit_status(&submission_operation_id, SpDepositStatus::Initiated);
    allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
    Ok(true)
}

async fn observe_stability_deposit(
    database: &Database,
    backend: &impl StabilityPoolBackend,
    mut item: StabilityPoolAllocationItem,
    operation_id: crate::stability_deposit::StabilityDepositOperationId,
    budget: ItemBudget,
) -> ServiceResult<bool> {
    let status = budget
        .bound_observation(
            "provider deposit observation",
            backend.observe_deposit(&item.target, operation_id),
        )
        .await
        .map_err(unavailable)?;
    tracing::debug!(
        federation_id = %item.federation_id.0,
        ?status,
        "stability: provider deposit observation"
    );
    match status {
        // Both nonterminal arms can arrive *after* a durable `success` for the
        // same operation id: the drain replays from the start whenever
        // `optimistically_set_operation_outcome` failed its write, which it only
        // logs. Assigning here would walk a terminal status backwards. The
        // setter refuses that and the observation is dropped, which is correct —
        // the later state is the one already recorded.
        StabilityDepositStatus::Initiated => {
            if !item
                .step
                .advance_sp_deposit_status(&operation_id.to_string(), SpDepositStatus::Initiated)
            {
                return Ok(true);
            }
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            Ok(true)
        }
        StabilityDepositStatus::TxAccepted => {
            if !item
                .step
                .advance_sp_deposit_status(&operation_id.to_string(), SpDepositStatus::TxAccepted)
            {
                return Ok(true);
            }
            allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
            Ok(true)
        }
        StabilityDepositStatus::Success => {
            item.step
                .advance_sp_deposit_status(&operation_id.to_string(), SpDepositStatus::Success);
            // Committed before the report, not after it. The report gate decides
            // completion; it must not also decide whether the observation was
            // recorded. With the write behind the report, an invocation could
            // drain the deposit stream to a terminal state, return `Ok`, and
            // still leave the item recorded as `initiated`. This ordering is
            // what makes the observation durable.
            //
            // The per-item budget cannot cut this write: it is a deadline
            // consulted at phase boundaries rather than a `timeout` wrapped
            // around the item, so nothing drops this future mid-write. What
            // remains is the shutdown drop in `run_interval_task`, which is why
            // this commits beyond its caller's cancellation — once per process
            // lifetime rather than on every pass an adversary can aim.
            commit_step_beyond_cancellation(database, &item.item_id, &item.step).await?;

            let fulfilled_amount = item
                .step
                .sp_deposit_amount
                .ok_or_else(|| internal_error("missing stability-pool deposit amount"))?;
            // A successful operation means our deposit transaction was accepted.
            // The provider account's own report is what says the funds are
            // actually provided, so require both before completing the item.
            let report = observe_stability_pool(database, backend, &mut item, budget).await?;
            if report.observed_provided_amount.0 < fulfilled_amount.0 {
                allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
                return Ok(true);
            }
            complete_stability_pool_item(database, item, fulfilled_amount).await
        }
        StabilityDepositStatus::Failed(reason) => {
            // The peg-in has been claimed, so the e-cash is held by the target
            // client and a rejected provider deposit leaves it there unprovided.
            // Same reasoning as the peg-in failure above: this needs an operator,
            // not a terminal status.
            //
            // Beyond cancellation: the rejection is the only durable effect of
            // a rejected deposit, and losing it to the shutdown `select!` leaves
            // the item nonterminal for another run of responsive invocations —
            // the same loss the peg-in failure arm three arms up guards against.
            require_item_action_beyond_cancellation(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::StabilityPoolFailed,
                reason,
            )
            .await?;
            Ok(true)
        }
    }
}

/// Commits an item step on a task the caller's cancellation cannot drop.
///
/// `update_item_step` is an await: it waits for a pool connection and then for
/// SQLite's write lock, which another task can hold for up to the configured
/// busy timeout. A drop landing there loses the write.
///
/// **The per-item budget is not the drop this guards against.** It is a
/// deadline consulted at phase boundaries rather than a `tokio::time::timeout`
/// wrapped around the item, so it drops nothing. A `timeout` would cut the write
/// on every pass, which anyone able to delay an earlier dependency could aim,
/// leaving a terminal observation unrecorded for an unbounded number of
/// responsive invocations.
///
/// What remains is `run_interval_task`'s shutdown `select!`, which drops the
/// whole pass at an arbitrary await. That is one event per process lifetime and
/// the item resumes from durable state on the next start, so this helper is a
/// small guarantee — worth keeping because a lost terminal observation is
/// expensive and a detached write is cheap.
///
/// Only the writes that *end* an observation use this. Recording something the
/// worker has already seen is safe to finish after its caller has gone: the
/// statement is guarded on the item still being active, and re-running it on a
/// later tick is idempotent.
async fn commit_step_beyond_cancellation(
    database: &Database,
    item_id: &fedi_decentralized_service_liquidity_manager::ItemId,
    step: &allocation_store::StabilityPoolAllocationStep,
) -> ServiceResult<()> {
    let database = database.clone();
    let item_id = item_id.clone();
    let step = serde_json::to_string(step).map_err(internal_error)?;
    let write = tokio::spawn(async move {
        let outcome = allocation_store::update_item_step_json(&database, &item_id, &step).await;
        // Logged here as well as returned: when the caller has been dropped
        // nothing reads the return value, and a lost terminal observation with
        // no trace is exactly what this helper exists to prevent.
        if let Err(error) = &outcome {
            tracing::error!(item_id = %item_id.0, ?error, "stability: detached step write failed");
        }
        outcome
    });
    write.await.map_err(internal_error)?
}

/// `require_item_action` on a task the caller's cancellation cannot drop.
///
/// Companion to [`commit_step_beyond_cancellation`], and it guards the same
/// drop: an arm that records a terminal observation must not lose it to the
/// shutdown `select!`. `set_item_failure` opens a `BEGIN IMMEDIATE`, so it
/// queues on SQLite's single write lock behind any other worker and the drop
/// window is the busy timeout, not an instant.
async fn require_item_action_beyond_cancellation(
    database: &Database,
    federation_id: &fedi_decentralized_service_liquidity_manager::FederationId,
    item_id: &fedi_decentralized_service_liquidity_manager::ItemId,
    code: LiquidityFailureCode,
    reason: impl Into<String>,
) -> ServiceResult<()> {
    let database = database.clone();
    let federation_id = federation_id.clone();
    let item_id = item_id.clone();
    let reason = reason.into();
    let federation_id_for_log = federation_id.0.clone();
    let item_id_for_log = item_id.0.clone();
    tokio::spawn(async move {
        let outcome = allocation_store::require_item_action(
            &database,
            &federation_id,
            &item_id,
            code,
            reason,
        )
        .await;
        // Logged inside for the same reason as the companion above: once the
        // caller is dropped nothing reads the return value. This one records a
        // terminal item failure, so losing it without a trace leaves an item
        // the operator is never told about.
        if let Err(error) = &outcome {
            tracing::error!(
                federation_id = %federation_id_for_log,
                item_id = %item_id_for_log,
                ?error,
                "stability: detached item-action write failed"
            );
        }
        outcome
    })
    .await
    .map_err(internal_error)?
}

async fn observe_stability_pool(
    database: &Database,
    backend: &impl StabilityPoolBackend,
    item: &mut StabilityPoolAllocationItem,
    budget: ItemBudget,
) -> ServiceResult<StabilityPoolReport> {
    // The provider report issues `request_current_consensus` calls that carry no
    // bound of their own, which is why it is bounded here.
    let report = budget
        .bound_observation("provider report", backend.report(&item.target))
        .await
        .map_err(unavailable)?;
    item.step.observed_provided_amount = Some(report.observed_provided_amount);
    allocation_store::upsert_stability_pool_observation(
        database,
        &StabilityPoolObservation {
            federation_id: item.target.federation_id.0.clone(),
            status: "provider_observed".to_owned(),
            observed_provided_amount: Some(report.observed_provided_amount),
            liquidity_stats_json: Some(report.liquidity_stats_json.clone()),
            observed_at: now_timestamp(),
        },
    )
    .await?;
    allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
    Ok(report)
}

async fn complete_stability_pool_item(
    database: &Database,
    item: StabilityPoolAllocationItem,
    fulfilled_amount: Sats,
) -> ServiceResult<bool> {
    allocation_store::complete_item(
        database,
        &item.federation_id,
        &item.item_id,
        fulfilled_amount,
        CompletionEvidence::StabilityPool(StabilityPoolCompletionEvidence {
            fulfilled_amount,
            observed_provided_amount: item.step.observed_provided_amount.unwrap_or(Sats(0)),
            observed_at: now_timestamp(),
            peg_in_operation_id: item.step.peg_in_operation_id.clone(),
            stability_pool_deposit_operation_id: item.step.sp_deposit_operation_id.clone(),
        }),
    )
    .await?;
    Ok(true)
}

fn committed_deposit_amount(
    committed_amount: Sats,
    claimed_amount: Sats,
    wallet_balance: Sats,
) -> Sats {
    if claimed_amount.0 < committed_amount.0 || wallet_balance.0 < committed_amount.0 {
        return Sats(0);
    }
    committed_amount
}

#[cfg(test)]
#[path = "../tests/stability_allocation.rs"]
mod tests;
