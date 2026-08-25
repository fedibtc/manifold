//! The guardian-fee vault: moving fee revenue out of the stability pool and
//! out of the daemon.
//!
//! Where the money goes is not decided here — `fman-core` derives
//! the recipient account and owns the policy, and hands this implementation
//! the seat's key on every call
//! ([`GuardianFeeVault`](fman_core::guardian_fee::GuardianFeeVault)).
//! This module only does what needs a Fedimint client in the guarded
//! federation: one ordinary client per seat, through the public client API,
//! never the seat's guardian authority.

use std::ops::Range;

use anyhow::Context as _;
use fedi_decentralized_service_fleet_manager::SeatId;
use fedimint_client::ClientHandleArc;
use fedimint_client::module::ClientModuleInstance;
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::invite_code::InviteCode;
use fman_core::guardian_fee::{
    AccountHistory, AccountId, Collected, CollectionFailure, CollectionFailurePhase,
    FederationFeeStatus, GuardianFeeAccountKey, GuardianFeeVault, Remittance,
};
use futures::StreamExt as _;
use stability_pool_client::api::StabilityPoolApiExt as _;
use stability_pool_client::common::{Account, AccountHistoryItem, AccountType, FiatOrAll};
use stability_pool_client::{StabilityPoolClientModule, StabilityPoolWithdrawalOperationState};

use crate::Wallet;

#[cfg(test)]
mod tests;

/// One seat's remittance-account history behind the stability-pool API.
///
/// Only the two reads live here. Walking them — how far back, and what the
/// entries add up to — is `fman-core`'s
/// ([`AccountHistory`](fman_core::guardian_fee::AccountHistory)), so that the
/// arithmetic a wrong lifetime total would come from is testable without a
/// federation.
struct RemittanceHistory {
    client: ClientHandleArc,
    account_id: AccountId,
}

#[async_trait::async_trait]
impl AccountHistory for RemittanceHistory {
    async fn count(&self) -> anyhow::Result<u64> {
        Ok(sp_module(&self.client)?
            .client_ctx
            .module_api()
            .account_sync(self.account_id)
            .await
            .context("read guardian-fee account state")?
            .account_history_count)
    }

    async fn page(&self, range: Range<u64>) -> anyhow::Result<Vec<AccountHistoryItem>> {
        sp_module(&self.client)?
            .client_ctx
            .module_api()
            .account_history(self.account_id, range)
            .await
            .context("read guardian-fee account history")
    }
}

impl Wallet {
    /// This seat's remittance-account history, ready for `fman-core` to walk.
    ///
    /// The module is resolved once here so a federation without a
    /// stability pool fails with that sentence rather than mid-walk.
    async fn account_history(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<RemittanceHistory> {
        let client = self.guardian_fee_client(invite_code, seat_id, key).await?;
        sp_module(&client)?;
        Ok(RemittanceHistory {
            client,
            account_id: key.account().id(),
        })
    }
}
// `HISTORY_PAGE` moved to `fman_core::guardian_fee` with the walk itself: the
// page size is part of how the history is read, and the reading is now core's.

#[async_trait::async_trait]
impl GuardianFeeVault for Wallet {
    /// Current balances and account identity for one guarded federation.
    async fn status(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<FederationFeeStatus> {
        let client = self.guardian_fee_client(invite_code, seat_id, key).await?;
        let module = sp_module(&client)?;
        let account = key.account();
        let sync = module
            .client_ctx
            .module_api()
            .account_sync(account.id())
            .await
            .context("read guardian-fee account state")?;
        Ok(FederationFeeStatus {
            federation_id: client.federation_id(),
            account_id: account.id(),
            staged: sync.staged_balance,
            locked: sync.locked_balance,
            idle: sync.idle_balance,
            history_count: sync.account_history_count,
        })
    }

    /// The most recent remittances into this FMan's account, newest first.
    async fn remittances(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
        limit: u64,
    ) -> anyhow::Result<Vec<Remittance>> {
        let history = self.account_history(invite_code, seat_id, key).await?;
        fman_core::guardian_fee::recent_remittances(&history, key, limit).await
    }

    /// Everything ever remitted into this FMan's account for this seat.
    async fn total_remitted(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Amount> {
        let history = self.account_history(invite_code, seat_id, key).await?;
        fman_core::guardian_fee::total_remitted(&history).await
    }

    /// Move everything remitted so far out of the pool and into this client's
    /// ecash balance, as far as the module allows right now.
    ///
    /// Two module operations are needed, and neither subsumes the other:
    /// balance already sitting idle is claimed directly, while staged and
    /// locked deposits go through an unlock. Locked deposits cannot leave
    /// mid-cycle — the module registers an unlock request and credits idle
    /// balance at the next cycle turnover — so a successful call here does not
    /// mean the account is empty. A complete result reports what remains, while
    /// an incomplete result preserves confirmed progress and reports whether a
    /// current remaining balance could be observed.
    async fn collect(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Collected> {
        let client = self.guardian_fee_client(invite_code, seat_id, key).await?;
        let module = sp_module(&client)?;
        let account = signing_account(&module, key)?;
        collect_operations(&FedimintCollectionOperations {
            module: &module,
            account_id: account.id(),
        })
        .await
    }

    /// Ecash balance already collected out of the pool and sitting in this
    /// client, awaiting a Lightning sweep by the operator.
    async fn ecash_balance(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Amount> {
        let client = self.guardian_fee_client(invite_code, seat_id, key).await?;
        client.get_balance_for_btc().await
    }
}

/// The stability-pool balances needed by collection orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CollectionBalances {
    /// Balance claimable immediately.
    idle: Amount,
    /// Balance staged for the current cycle.
    staged: Amount,
    /// Balance locked until cycle turnover.
    locked: Amount,
}

/// Operations used by the phase runner, abstracted for failure-point tests.
#[async_trait::async_trait]
trait CollectionOperations: Sync {
    /// Opaque durable token returned by a successful submission.
    type Operation: Send;

    /// Read current stability-pool balances.
    async fn balances(&self) -> anyhow::Result<CollectionBalances>;

    /// Submit a claim for immediately available idle balance.
    async fn submit_idle(&self, amount: Amount) -> anyhow::Result<Self::Operation>;

    /// Wait for a submitted idle claim to terminate.
    async fn await_idle(&self, operation: Self::Operation) -> anyhow::Result<Amount>;

    /// Submit release of all staged and locked balance.
    async fn submit_unlock(&self) -> anyhow::Result<Self::Operation>;

    /// Wait for a submitted unlock to terminate.
    async fn await_unlock(&self, operation: Self::Operation) -> anyhow::Result<Amount>;
}

/// Fedimint-backed collection operations for one account.
struct FedimintCollectionOperations<'a> {
    /// Stability-pool client module.
    module: &'a ClientModuleInstance<'a, StabilityPoolClientModule>,
    /// Account selected and checked against the seat key.
    account_id: stability_pool_common::AccountId,
}

#[async_trait::async_trait]
impl CollectionOperations for FedimintCollectionOperations<'_> {
    type Operation = OperationId;

    async fn balances(&self) -> anyhow::Result<CollectionBalances> {
        let account = self
            .module
            .client_ctx
            .module_api()
            .account_sync(self.account_id)
            .await?;
        Ok(CollectionBalances {
            idle: account.idle_balance,
            staged: account.staged_balance,
            locked: account.locked_balance,
        })
    }

    async fn submit_idle(&self, amount: Amount) -> anyhow::Result<Self::Operation> {
        let (operation_id, _txid) = self
            .module
            .withdraw_idle_balance(AccountType::BtcDepositor, amount, ())
            .await
            .context("submit guardian-fee idle-balance claim")?;
        Ok(operation_id)
    }

    async fn await_idle(&self, operation_id: Self::Operation) -> anyhow::Result<Amount> {
        let mut updates = self
            .module
            .subscribe_withdraw_idle_balance(operation_id)
            .await
            .context("subscribe to guardian-fee idle-balance claim")?
            .into_stream();
        terminal_withdrawal(&mut updates, "idle-balance claim").await
    }

    async fn submit_unlock(&self) -> anyhow::Result<Self::Operation> {
        let (operation_id, _txid) = self
            .module
            .withdraw(AccountType::BtcDepositor, FiatOrAll::All, ())
            .await
            .context("submit guardian-fee unlock")?;
        Ok(operation_id)
    }

    async fn await_unlock(&self, operation_id: Self::Operation) -> anyhow::Result<Amount> {
        let mut updates = self
            .module
            .subscribe_withdraw(operation_id)
            .await
            .context("subscribe to guardian-fee unlock")?
            .into_stream();
        terminal_withdrawal(&mut updates, "unlock").await
    }
}

/// Run the complete two-step collection workflow over one operation provider.
async fn collect_operations(operations: &impl CollectionOperations) -> anyhow::Result<Collected> {
    let before = operations
        .balances()
        .await
        .context("read guardian-fee account state")?;
    let before_awaiting_cycle = checked_amount_sum(
        before.staged,
        before.locked,
        "guardian-fee awaiting-cycle balance",
    )?;
    let mut confirmed_claimed = Amount::ZERO;
    let mut operation_exists = false;

    if before.idle != Amount::ZERO {
        let operation = operations.submit_idle(before.idle).await?;
        operation_exists = true;
        match operations.await_idle(operation).await {
            Ok(claimed) => {
                confirmed_claimed = checked_amount_sum(
                    confirmed_claimed,
                    claimed,
                    "confirmed guardian-fee claims",
                )?;
            }
            Err(_) => {
                return Ok(incomplete_after_refresh(
                    operations,
                    confirmed_claimed,
                    CollectionFailurePhase::IdleClaim,
                    true,
                )
                .await);
            }
        }
    }

    if before_awaiting_cycle != Amount::ZERO {
        let operation = match operations.submit_unlock().await {
            Ok(operation) => operation,
            Err(error) if !operation_exists && confirmed_claimed == Amount::ZERO => {
                return Err(error);
            }
            Err(_) => {
                return Ok(incomplete_after_refresh(
                    operations,
                    confirmed_claimed,
                    CollectionFailurePhase::Unlock,
                    false,
                )
                .await);
            }
        };
        operation_exists = true;
        match operations.await_unlock(operation).await {
            Ok(claimed) => {
                confirmed_claimed = checked_amount_sum(
                    confirmed_claimed,
                    claimed,
                    "confirmed guardian-fee claims",
                )?;
            }
            Err(_) => {
                return Ok(incomplete_after_refresh(
                    operations,
                    confirmed_claimed,
                    CollectionFailurePhase::Unlock,
                    true,
                )
                .await);
            }
        }
    }

    match operations
        .balances()
        .await
        .context("re-read guardian-fee account state")
    {
        Ok(after) => match checked_amount_sum(
            after.staged,
            after.locked,
            "guardian-fee awaiting-cycle balance",
        ) {
            Ok(awaiting_cycle) => Ok(Collected::Complete {
                claimed: confirmed_claimed,
                awaiting_cycle,
            }),
            Err(_) if operation_exists => Ok(incomplete(
                confirmed_claimed,
                None,
                CollectionFailurePhase::BalanceRefresh,
                false,
            )),
            Err(error) => Err(error),
        },
        Err(_) if operation_exists => Ok(incomplete(
            confirmed_claimed,
            None,
            CollectionFailurePhase::BalanceRefresh,
            false,
        )),
        Err(error) => Err(error),
    }
}

/// Best-effort refresh after preserving the original failed phase.
async fn incomplete_after_refresh(
    operations: &impl CollectionOperations,
    confirmed_claimed: Amount,
    phase: CollectionFailurePhase,
    operation_submitted: bool,
) -> Collected {
    let observed = operations.balances().await.ok().and_then(|balance| {
        checked_amount_sum(
            balance.staged,
            balance.locked,
            "guardian-fee awaiting-cycle balance",
        )
        .ok()
    });
    incomplete(confirmed_claimed, observed, phase, operation_submitted)
}

/// Add two dependency-supplied amounts without wrapping monetary state.
fn checked_amount_sum(left: Amount, right: Amount, what: &str) -> anyhow::Result<Amount> {
    left.checked_add(right)
        .ok_or_else(|| anyhow::anyhow!("{what} exceeds the representable amount"))
}

/// Build a structural incomplete result for core to project safely.
fn incomplete(
    confirmed_claimed: Amount,
    observed_awaiting_cycle: Option<Amount>,
    phase: CollectionFailurePhase,
    operation_submitted: bool,
) -> Collected {
    Collected::Incomplete {
        confirmed_claimed,
        observed_awaiting_cycle,
        failure: CollectionFailure {
            phase,
            operation_submitted,
        },
    }
}

/// Drive one withdrawal operation to its terminal state and report what
/// reached the client's ecash balance.
async fn terminal_withdrawal(
    updates: &mut (impl futures::Stream<Item = StabilityPoolWithdrawalOperationState> + Unpin),
    what: &str,
) -> anyhow::Result<Amount> {
    while let Some(state) = updates.next().await {
        match state {
            StabilityPoolWithdrawalOperationState::Success(amount) => return Ok(amount),
            StabilityPoolWithdrawalOperationState::UnlockTxRejected(err)
            | StabilityPoolWithdrawalOperationState::UnlockProcessingError(err)
            | StabilityPoolWithdrawalOperationState::WithdrawalTxRejected(err)
            | StabilityPoolWithdrawalOperationState::PrimaryOutputError(err) => {
                anyhow::bail!("guardian-fee {what} failed: {err}")
            }
            _ => {}
        }
    }
    anyhow::bail!("guardian-fee {what} ended without a terminal state")
}

fn sp_module(
    client: &ClientHandleArc,
) -> anyhow::Result<ClientModuleInstance<'_, StabilityPoolClientModule>> {
    client
        .get_first_module::<StabilityPoolClientModule>()
        .context("federation has no stability-pool module, so it cannot remit guardian fees at all")
}

/// The account for operations the module has to *sign* for.
///
/// Reads go by account id and need no key, but anything that moves money is
/// signed by whichever `BtcDepositor` key the module is carrying. That is the
/// module's own derivation from the client root unless the client was built
/// with this seat's key, and acting on it would move funds in an account no
/// payer was ever told about — so this refuses rather than operating on the
/// wrong account.
fn signing_account(
    module: &ClientModuleInstance<'_, StabilityPoolClientModule>,
    key: &GuardianFeeAccountKey,
) -> anyhow::Result<Account> {
    let ours = key.account();
    let carried = module.our_account(AccountType::BtcDepositor);
    anyhow::ensure!(
        carried == ours,
        "stability-pool client is carrying account {carried_id}, but this seat's committed \
         remittance account is {our_id}: the client module has to be built with the seat's \
         BtcDepositor key before collection can sign for it",
        carried_id = carried.id(),
        our_id = ours.id(),
    );
    Ok(ours)
}
