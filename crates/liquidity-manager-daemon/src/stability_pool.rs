//! Stability-pool client adapter for a target federation.
//!
//! [`StabilityPoolBackend`] is the seam over peg-in allocation, provider
//! deposit submission, and the account report the worker observes.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use fedi_decentralized_service_liquidity_manager::{
    ChainObserverBackendView, ChainObserverConfigView, Sats,
};
use fedimint_client::ClientHandleArc;
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::util::SafeUrl;
use fedimint_wallet_client::{DepositStateV2, MaybeNewAddress, WalletClientModule};
use futures_util::StreamExt;
use stability_pool_client::common::{AccountType, ActiveDeposits, FeeRate};
use stability_pool_client::{
    StabilityPoolClientModule, StabilityPoolDepositOperationState, StabilityPoolMeta,
};

use crate::allocation_store::FundingTargetRecord;
use crate::stability_deposit::{
    StabilityDepositMetadata, StabilityDepositOperationId, StabilityDepositSubmission,
};
use crate::target_fedimint::TargetFedimintClients;

#[derive(Clone)]
pub(crate) struct FedimintStabilityPoolBackend {
    federations_dir: std::path::PathBuf,
    clients: TargetFedimintClients,
    esplora_url: Option<SafeUrl>,
}

impl FedimintStabilityPoolBackend {
    pub(crate) fn new(
        federations_dir: std::path::PathBuf,
        clients: TargetFedimintClients,
        chain_observer: &ChainObserverConfigView,
    ) -> Self {
        Self {
            federations_dir,
            clients,
            esplora_url: target_client_esplora_url(chain_observer),
        }
    }

    async fn client(&self, target: &FundingTargetRecord) -> anyhow::Result<ClientHandleArc> {
        self.clients
            .create_or_load(
                &self.federations_dir,
                &target.federation_id.0,
                &target.invite_code.0,
                self.esplora_url.as_ref(),
            )
            .await
            .map_err(anyhow::Error::from)
    }
}

/// The chain backend to give a target federation's wallet client, from this
/// daemon's own chain-observer configuration.
///
/// Only esplora qualifies. The Fedimint wallet client has no bitcoind path at
/// all, so a bitcoind-configured FLIP has nothing to offer and the client falls
/// back to the esplora the target federation advertises — which is the only way
/// such a deployment can claim a peg-in, and a reason to prefer esplora here.
fn target_client_esplora_url(chain_observer: &ChainObserverConfigView) -> Option<SafeUrl> {
    match &chain_observer.backend {
        ChainObserverBackendView::Esplora { url } => match SafeUrl::parse(&url.0) {
            Ok(url) => Some(url),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "configured esplora URL is unusable as a target-client chain backend; \
                     falling back to the target federation's advertised backend"
                );
                None
            }
        },
        ChainObserverBackendView::Bitcoind { .. } => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PegInAddress {
    pub operation_id: String,
    pub address: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PegInStatus {
    WaitingForTransaction,
    WaitingForConfirmation { amount: Sats },
    Confirmed { amount: Sats },
    Claimed { amount: Sats },
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StabilityDepositStatus {
    Initiated,
    TxAccepted,
    Success,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StabilityPoolReport {
    pub observed_provided_amount: Sats,
    pub liquidity_stats_json: String,
}

/// One stability-pool deposit the target client itself records having made.
///
/// The client's operation log is the only place a deposit FLIP failed to record
/// still exists, so this is what an operator reconciles against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetDepositOperation {
    pub operation_id: String,
    pub amount: Sats,

    /// The outcome the client has cached, when it has one.
    ///
    /// A deposit whose stream was never drained to completion has none — which
    /// is exactly the state a crash in the submit window leaves behind, so
    /// "unobserved" is a finding rather than a gap.
    pub outcome: Option<StabilityDepositStatus>,

    /// Seconds since the Unix epoch, as the client recorded them.
    pub created_at: u64,
}

/// A read of the target client's deposit history.
///
/// Carries whether the read was exhaustive, because the two conclusions drawn
/// from it are not symmetric: finding a deposit is evidence on its own, while
/// concluding that none exists — and therefore that submitting again is safe —
/// is only sound if nothing was left unread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetDepositScan {
    pub operations: Vec<TargetDepositOperation>,

    /// False when the operation log was longer than the scan bound.
    pub complete: bool,
}

/// Result of checking the exact caller-owned receipt before submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubmissionReceipt {
    /// No operation exists, so the caller-ID API committed a new one.
    Submitted,
    /// The exact validated operation already exists.
    Matching,
    /// The ID exists but its immutable operation commitment differs.
    Conflicting,
}

/// How far back through the target client's operation log to look.
///
/// One FLIP item makes one deposit, so anything relevant to reconciling an
/// interrupted item is recent; the bound keeps an operator's request from
/// walking an arbitrarily long history.
const TARGET_OPERATION_SCAN_LIMIT: usize = 50;

/// Backend for the target federation's on-chain peg-in and stability-pool
/// provider deposit.
///
/// Progress is observed by *consuming* Fedimint operation update streams. Both
/// `subscribe_deposit` and `subscribe_deposit_operation` build their stream with
/// `outcome_or_updates`, which returns the terminal state directly once the
/// operation log has an outcome, and otherwise wraps a generator in
/// `caching_operation_update_stream`. That wrapper writes the operation's
/// outcome only after the inner stream *ends*, and the generator's first item is
/// always the initial state (`WaitingForTransaction` / `Initiated`).
///
/// So the stream must be drained to completion: a subscriber that reads one item
/// and drops the stream caches nothing and re-reads the initial state on every
/// later subscription, never advancing. Once drained, the outcome is cached in
/// the operation log and every later subscription resolves it immediately —
/// including after a daemon restart.
#[async_trait]
pub(crate) trait StabilityPoolBackend: Send + Sync {
    async fn ensure_client(&self, target: &FundingTargetRecord) -> anyhow::Result<()>;
    async fn allocate_peg_in_address(
        &self,
        target: &FundingTargetRecord,
    ) -> anyhow::Result<PegInAllocation>;
    /// Furthest peg-in state reachable this tick, from the deposit operation's
    /// own update stream.
    async fn observe_peg_in(
        &self,
        target: &FundingTargetRecord,
        operation_id: &str,
    ) -> anyhow::Result<PegInStatus>;
    /// Prompt the wallet client's background peg-in monitor to re-scan the
    /// deposit address and claim it if it is ready. The monitor is woken once
    /// when the address is allocated (before the on-chain funds arrive) and
    /// otherwise only polls on its own slow schedule, so it must be nudged
    /// while the deposit confirms. Draining the deposit stream does not do this:
    /// the stream *waits* on the monitor's `ClaimedPegInKey` write, it does not
    /// drive it.
    async fn recheck_peg_in(
        &self,
        target: &FundingTargetRecord,
        operation_id: &str,
    ) -> anyhow::Result<()>;
    /// Spendable BTC-denominated balance of the target client, in sats. Used to
    /// size the provider deposit against the e-cash actually on hand — never as
    /// a completion signal, since it is a client-wide total that any other
    /// operation also moves.
    async fn target_wallet_balance(&self, target: &FundingTargetRecord) -> anyhow::Result<Sats>;
    /// Reuses one immutable caller-owned request.
    ///
    /// Implementations must perform exact global lookup first, return
    /// [`SubmissionReceipt::Matching`] only after validating the operation kind,
    /// provider role, owner, amount, fee, and metadata version, return
    /// [`SubmissionReceipt::Conflicting`] without submitting for an occupied
    /// mismatching ID, and call the financial API only when the ID is absent.
    /// Every error is ambiguous; callers preserve the tuple and retry this method.
    async fn submit_deposit_to_provide(
        &self,
        target: &FundingTargetRecord,
        submission: StabilityDepositSubmission,
        diagnostic_item_id: &str,
    ) -> anyhow::Result<SubmissionReceipt>;
    /// Furthest provider-deposit state reachable this tick, from the deposit
    /// operation's own update stream.
    async fn observe_deposit(
        &self,
        target: &FundingTargetRecord,
        operation_id: StabilityDepositOperationId,
    ) -> anyhow::Result<StabilityDepositStatus>;
    /// Provider-account active-deposit report. Corroborates a `Success` deposit
    /// before the item is completed, and feeds the recorded observation.
    async fn report(&self, target: &FundingTargetRecord) -> anyhow::Result<StabilityPoolReport>;

    /// Stability-pool deposits the target client records having made.
    ///
    /// Read-only, and deliberately does not drive any operation's stream: an
    /// operator asking what happened should not have to wait on pending work,
    /// and an undrained stream is itself part of the answer.
    async fn list_deposit_operations(
        &self,
        target: &FundingTargetRecord,
    ) -> anyhow::Result<TargetDepositScan>;

    /// Looks up one exact global operation ID and validates that it is a
    /// stability-pool deposit.
    async fn get_deposit_operation(
        &self,
        target: &FundingTargetRecord,
        operation_id: StabilityDepositOperationId,
    ) -> anyhow::Result<Option<TargetDepositOperation>>;

    /// Whether the client the worker would actually fund is the target this
    /// allocation was accepted for, and can hold a provider deposit.
    ///
    /// `Err` means the answer is not available this tick and the item should
    /// be retried. `Ok(TargetCheck::Unusable)` is a decision: no later tick
    /// reaches a different answer for this recorded target.
    async fn check_target(&self, target: &FundingTargetRecord) -> anyhow::Result<TargetCheck>;
}

/// Whether `client` is the target this allocation was accepted for, and can
/// hold a provider deposit.
///
/// Takes the open handle rather than the record, so the answer describes the
/// client the caller is holding rather than whichever one a fresh
/// `create_or_load` would return. The distinction is the whole point: the pool
/// is keyed by federation id, so two `client()` calls either side of a drop can
/// return different clients with different configs.
/// The config-hash half of [`usable_target`], separated so that it can be
/// tested without a live federation.
///
/// It must stay an exact string comparison: both sides are lowercase hex from
/// `hex::encode`, and any normalisation — trimming, case folding, prefix
/// matching — would let a client carrying a different config mint the address
/// the provider funds.
fn config_hash_check(observed: &str, expected: &str) -> TargetCheck {
    if observed != expected {
        return TargetCheck::Unusable(format!(
            "target client config hash {observed} is not the accepted {expected}"
        ));
    }
    TargetCheck::Usable
}

async fn usable_target(client: &ClientHandleArc, target: &FundingTargetRecord) -> TargetCheck {
    // A federation id does not commit the module map. An earlier initialization
    // for this id can have stored a different config than the one the request
    // was accepted against, and the client loads that stored config without
    // comparing it to anything. Comparing here is what joins the accepted
    // target to the funded one.
    let observed = hex::encode(
        fedi_decentralized_federation_preview::federation_config_hash(&client.config().await).0,
    );
    if let TargetCheck::Unusable(reason) =
        config_hash_check(&observed, &target.federation_config_hash)
    {
        return TargetCheck::Unusable(reason);
    }

    // Asked before funding rather than at the first deposit. The deposit lookup
    // happens after the peg-in is claimed, so by the time it answers the
    // provider's value is already inside a client that cannot deposit it.
    if client
        .get_first_module::<StabilityPoolClientModule>()
        .is_err()
    {
        return TargetCheck::Unusable(
            "target federation has no usable stability-pool module".to_owned(),
        );
    }

    TargetCheck::Usable
}

/// Fedimint module kind a stability-pool allocation requires of its target.
///
/// Spelled out rather than read from `stability_pool_client::common::KIND`
/// because it is also what fixtures and tests must name; the test below fails
/// if upstream ever renames the kind.
pub const STABILITY_POOL_MODULE_KIND: &str = "multi_sig_stability_pool";

/// Result of allocating a deposit address on a checked target.
///
/// Separate from `anyhow::Result` because the trait draws a line this has to
/// respect: `Err` means "not available this tick, retry", and the worker
/// propagates it out of the whole pass. A target that is not the accepted one
/// is a *decision*, so it belongs in the value rather than the error.
#[derive(Debug)]
pub(crate) enum PegInAllocation {
    /// The target was the accepted one and this address is its deposit address.
    Allocated(PegInAddress),

    /// The target the pool would have funded is not the accepted one, for the
    /// stated reason. No later tick reaches a different answer for it.
    Unusable(String),
}

/// Result of the pre-funding target check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TargetCheck {
    /// The opened client matches the accepted config hash and has the module.
    Usable,

    /// The opened client is not one this allocation may fund, for the stated
    /// reason.
    Unusable(String),
}

/// How long one processing tick may spend draining an operation update stream.
///
/// This is not a wait for the deposit to confirm. When the operation has already
/// finished, its whole stream resolves in about one API round-trip and the drain
/// completes well inside this budget, caching the outcome for good. When it has
/// not, the drain blocks on the pending step for the whole budget, so a pass
/// over pending items costs up to this much per item — chosen to absorb a slow
/// esplora round-trip without letting a stalled item dominate the pass.
const STREAM_DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// Consume an operation update stream and return the furthest state it reached.
///
/// Callers pass `subscription.into_stream()`, which flattens the
/// already-cached-outcome case into a single-item stream so both arms drain the
/// same way. Returns `None` only if nothing was yielded at all.
async fn drain_furthest_state<T>(updates: impl futures_util::Stream<Item = T>) -> Option<T> {
    let deadline = tokio::time::Instant::now() + STREAM_DRAIN_BUDGET;
    let mut updates = std::pin::pin!(updates);
    let mut furthest = None;
    // Ends on `Ok(None)` (stream complete, outcome now cached) or on `Err`
    // (budget spent, nothing cached — the next tick resumes from the start).
    while let Ok(Some(update)) = tokio::time::timeout_at(deadline, updates.next()).await {
        furthest = Some(update);
    }
    furthest
}

#[async_trait]
impl StabilityPoolBackend for FedimintStabilityPoolBackend {
    async fn ensure_client(&self, target: &FundingTargetRecord) -> anyhow::Result<()> {
        self.client(target).await?;
        Ok(())
    }

    async fn check_target(&self, target: &FundingTargetRecord) -> anyhow::Result<TargetCheck> {
        let client = self.client(target).await?;
        Ok(usable_target(&client, target).await)
    }

    async fn allocate_peg_in_address(
        &self,
        target: &FundingTargetRecord,
    ) -> anyhow::Result<PegInAllocation> {
        let client = self.client(target).await?;

        // Asked again, on this handle. `check_target`'s answer described
        // whichever client it opened, and it dropped that handle before
        // returning — after which the pool can close the client and reopen it,
        // and `ClientBuilder::open` promotes any pending config the federation
        // published in between. Re-asking here is what makes the answer describe
        // the client that actually mints the address the provider funds.
        if let TargetCheck::Unusable(reason) = usable_target(&client, target).await {
            return Ok(PegInAllocation::Unusable(reason));
        }

        let wallet = client.get_first_module::<WalletClientModule>()?;
        // Refuse to peg into a federation whose wallet module cannot process
        // deposits above `ALEPH_BFT_UNIT_BYTE_LIMIT`, which would silently burn
        // the funds. This is the same predicate `safe_allocate_deposit_address`
        // enforces (`SAFE_DEPOSIT_MODULE_CONSENSUS_VERSION` is 2.2), but asked
        // read-only.
        //
        // We cannot use `safe_allocate_deposit_address` itself: its
        // `supports_safe_deposit` guard *caches* the answer through a
        // non-autocommit write to `SupportsSafeDepositKey`, racing the wallet
        // client's own background probe task on that same key. Both write with
        // `insert_new_entry` + `commit_tx`, which panic on an already-present
        // key and on a write conflict respectively, so whichever loses dies and
        // the allocation hangs. `btc_tx_has_no_size_limit` reads the same
        // consensus version without touching the cache, leaving the background
        // probe as the single writer.
        anyhow::ensure!(
            wallet.btc_tx_has_no_size_limit().await?,
            "target federation's wallet module consensus version does not support safe deposits"
        );
        // Pooled rather than `allocate_deposit_address_expert_only`, so that
        // this call is idempotent for the caller that loses its result.
        //
        // The expert-only variant mints a fresh tweak index and operation id on
        // every call. FLIP records what it returns *after* the call returns, so
        // anything that stops the worker in between — a crash, or a budget that
        // abandons the call — would leave the address minted in the target
        // client and unknown to FLIP, and the ordinary retry would mint a second
        // one. Upstream takes no caller-supplied operation id here, unlike
        // `deposit_to_provide`, so the id cannot be fenced the way the provider
        // deposit's is.
        //
        // `max_gap_size` of 1 makes the retry return the address the lost call
        // minted instead of a new one: the pooled call reuses whenever at least
        // one *unused* address exists — `PegInTweakIndexData::claimed` empty —
        // and mints fresh only when none does. Reuse is safe here because a
        // target client serves at most one stability item: the allocation schema
        // permits one allocation per federation and one item per source type,
        // and the target client is keyed by federation id. So the single unused
        // address on this client is this item's own.
        //
        // An address that has already been claimed is not "unused", so a fresh
        // one is minted for the next item that legitimately needs one, and a
        // reused address is one no deposit has been claimed on.
        let deposit = match wallet.allocate_deposit_address_pooled_stateless(1).await? {
            MaybeNewAddress::NewAddress(deposit) => deposit,
            MaybeNewAddress::TooManyUnusedAddresses(addresses) => addresses
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("pooled allocation reported no reusable address"))?,
        };
        Ok(PegInAllocation::Allocated(PegInAddress {
            operation_id: deposit.operation_id.fmt_full().to_string(),
            address: deposit.address.to_string(),
        }))
    }

    async fn observe_peg_in(
        &self,
        target: &FundingTargetRecord,
        operation_id: &str,
    ) -> anyhow::Result<PegInStatus> {
        let client = self.client(target).await?;
        let wallet = client.get_first_module::<WalletClientModule>()?;
        let operation_id = OperationId::from_str(operation_id)?;
        let updates = wallet.subscribe_deposit(operation_id).await?.into_stream();
        Ok(match drain_furthest_state(updates).await {
            Some(DepositStateV2::Claimed { btc_deposited, .. }) => PegInStatus::Claimed {
                amount: Sats(btc_deposited.to_sat()),
            },
            Some(DepositStateV2::Confirmed { btc_deposited, .. }) => PegInStatus::Confirmed {
                amount: Sats(btc_deposited.to_sat()),
            },
            Some(DepositStateV2::WaitingForConfirmation { btc_deposited, .. }) => {
                PegInStatus::WaitingForConfirmation {
                    amount: Sats(btc_deposited.to_sat()),
                }
            }
            Some(DepositStateV2::Failed(reason)) => PegInStatus::Failed(reason),
            // An empty drain means the budget expired before even the generator's
            // first item, which is indistinguishable from "not started yet".
            Some(DepositStateV2::WaitingForTransaction) | None => {
                PegInStatus::WaitingForTransaction
            }
        })
    }

    async fn recheck_peg_in(
        &self,
        target: &FundingTargetRecord,
        operation_id: &str,
    ) -> anyhow::Result<()> {
        let client = self.client(target).await?;
        let wallet = client.get_first_module::<WalletClientModule>()?;
        let operation_id = OperationId::from_str(operation_id)?;
        wallet.recheck_pegin_address_by_op_id(operation_id).await
    }

    async fn target_wallet_balance(&self, target: &FundingTargetRecord) -> anyhow::Result<Sats> {
        let client = self.client(target).await?;
        let balance = client.get_balance_for_btc().await?;
        Ok(Sats(balance.msats / 1000))
    }

    async fn submit_deposit_to_provide(
        &self,
        target: &FundingTargetRecord,
        submission: StabilityDepositSubmission,
        diagnostic_item_id: &str,
    ) -> anyhow::Result<SubmissionReceipt> {
        let operation_id = submission.operation_id().as_fedimint();
        let client = self.client(target).await?;
        let stability_pool = client.get_first_module::<StabilityPoolClientModule>()?;

        if let Some(entry) = client.operation_log().get_operation(operation_id).await {
            let metadata_matches = entry.operation_module_kind()
                == stability_pool_client::common::KIND.as_str()
                && matches!(
                    entry.try_meta::<StabilityPoolMeta>(),
                    Ok(StabilityPoolMeta::Deposit { ref extra_meta, amount, .. })
                        if amount == Amount::from_sats(submission.amount().0)
                            && serde_json::from_value::<StabilityDepositMetadata>(
                                extra_meta.clone()
                            )
                            .is_ok_and(|metadata| metadata.matches(diagnostic_item_id, submission))
                );
            return Ok(if metadata_matches {
                SubmissionReceipt::Matching
            } else {
                SubmissionReceipt::Conflicting
            });
        }

        stability_pool
            .deposit_to_provide_with_operation_id(
                operation_id,
                Amount::from_sats(submission.amount().0),
                FeeRate(submission.min_fee_rate_ppb()),
                StabilityDepositMetadata::new(diagnostic_item_id, submission),
            )
            .await?;
        Ok(SubmissionReceipt::Submitted)
    }

    async fn observe_deposit(
        &self,
        target: &FundingTargetRecord,
        operation_id: StabilityDepositOperationId,
    ) -> anyhow::Result<StabilityDepositStatus> {
        let client = self.client(target).await?;
        let stability_pool = client.get_first_module::<StabilityPoolClientModule>()?;
        let updates = stability_pool
            .subscribe_deposit_operation(operation_id.as_fedimint())
            .await?
            .into_stream();
        Ok(match drain_furthest_state(updates).await {
            Some(StabilityPoolDepositOperationState::Success) => StabilityDepositStatus::Success,
            Some(StabilityPoolDepositOperationState::TxAccepted) => {
                StabilityDepositStatus::TxAccepted
            }
            Some(StabilityPoolDepositOperationState::TxRejected(reason)) => {
                StabilityDepositStatus::Failed(reason)
            }
            Some(StabilityPoolDepositOperationState::PrimaryOutputError(reason)) => {
                StabilityDepositStatus::Failed(reason)
            }
            Some(StabilityPoolDepositOperationState::Initiated) | None => {
                StabilityDepositStatus::Initiated
            }
        })
    }

    async fn report(&self, target: &FundingTargetRecord) -> anyhow::Result<StabilityPoolReport> {
        let client = self.client(target).await?;
        let stability_pool = client.get_first_module::<StabilityPoolClientModule>()?;
        let account = stability_pool.our_account(AccountType::Provider).id();
        let active = stability_pool.active_deposits(account).await?;
        let liquidity_stats = stability_pool.liquidity_stats().await?;
        let observed_provided_amount = active_provided_sats(active);
        Ok(StabilityPoolReport {
            observed_provided_amount,
            liquidity_stats_json: serde_json::to_string(&liquidity_stats)?,
        })
    }

    async fn list_deposit_operations(
        &self,
        target: &FundingTargetRecord,
    ) -> anyhow::Result<TargetDepositScan> {
        let client = self.client(target).await?;
        let entries = client
            .operation_log()
            .paginate_operations_rev(TARGET_OPERATION_SCAN_LIMIT, None)
            .await;
        // The bound applies to operations of every kind, so a full page means
        // there may be older ones this read never saw.
        let complete = entries.len() < TARGET_OPERATION_SCAN_LIMIT;

        let mut deposits = Vec::new();
        for (key, entry) in entries {
            if entry.operation_module_kind() != stability_pool_client::common::KIND.as_str() {
                continue;
            }
            // The log holds transfers and withdrawals under the same module
            // kind; only a deposit put e-cash into the pool.
            let Ok(StabilityPoolMeta::Deposit { amount, .. }) =
                entry.try_meta::<StabilityPoolMeta>()
            else {
                continue;
            };
            deposits.push(TargetDepositOperation {
                operation_id: key.operation_id.fmt_full().to_string(),
                amount: Sats(amount.msats / 1000),
                outcome: entry
                    .try_outcome::<StabilityPoolDepositOperationState>()
                    .ok()
                    .flatten()
                    .map(deposit_status_from_state),
                created_at: key
                    .creation_time
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_secs())
                    .unwrap_or_default(),
            });
        }
        Ok(TargetDepositScan {
            operations: deposits,
            complete,
        })
    }

    async fn get_deposit_operation(
        &self,
        target: &FundingTargetRecord,
        operation_id: StabilityDepositOperationId,
    ) -> anyhow::Result<Option<TargetDepositOperation>> {
        let client = self.client(target).await?;
        let operation_id = operation_id.as_fedimint();
        let Some(entry) = client.operation_log().get_operation(operation_id).await else {
            return Ok(None);
        };
        if entry.operation_module_kind() != stability_pool_client::common::KIND.as_str() {
            return Ok(None);
        }
        let Ok(StabilityPoolMeta::Deposit { amount, .. }) = entry.try_meta::<StabilityPoolMeta>()
        else {
            return Ok(None);
        };
        Ok(Some(TargetDepositOperation {
            operation_id: operation_id.fmt_full().to_string(),
            amount: Sats(amount.msats / 1000),
            outcome: entry
                .try_outcome::<StabilityPoolDepositOperationState>()
                .ok()
                .flatten()
                .map(deposit_status_from_state),
            created_at: 0,
        }))
    }
}

fn deposit_status_from_state(state: StabilityPoolDepositOperationState) -> StabilityDepositStatus {
    match state {
        StabilityPoolDepositOperationState::Success => StabilityDepositStatus::Success,
        StabilityPoolDepositOperationState::TxAccepted => StabilityDepositStatus::TxAccepted,
        StabilityPoolDepositOperationState::Initiated => StabilityDepositStatus::Initiated,
        StabilityPoolDepositOperationState::TxRejected(reason)
        | StabilityPoolDepositOperationState::PrimaryOutputError(reason) => {
            StabilityDepositStatus::Failed(reason)
        }
    }
}

fn active_provided_sats(active: ActiveDeposits) -> Sats {
    match active {
        ActiveDeposits::Provider { staged, locked } => {
            let staged_msats: u64 = staged.iter().map(|provide| provide.amount.msats).sum();
            let locked_msats: u64 = locked.iter().map(|provide| provide.amount.msats).sum();
            Sats(staged_msats.saturating_add(locked_msats) / 1000)
        }
        ActiveDeposits::Seeker { .. } => Sats(0),
    }
}

#[cfg(test)]
#[path = "../tests/stability_pool.rs"]
mod tests;
