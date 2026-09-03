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
use fedimint_client::db::{OperationLogKey, OperationLogKeyPrefix};
use fedimint_client::module::ClientModuleInstance;
use fedimint_client_module::oplog::OperationLogEntry;
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::db::{Database, IDatabaseTransactionOpsCore, IDatabaseTransactionOpsCoreTyped};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::invite_code::InviteCode;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fman_core::guardian_fee::{
    AccountHistory, AccountId, Collected, CollectionFailure, CollectionFailurePhase,
    FederationFeeStatus, GuardianFeeAccountKey, GuardianFeeVault, Remittance,
};
use futures::StreamExt as _;
use stability_pool_client::api::StabilityPoolApiExt as _;
use stability_pool_client::common::{Account, AccountHistoryItem, AccountType, FiatOrAll};
use stability_pool_client::{
    StabilityPoolClientModule, StabilityPoolMeta, StabilityPoolWithdrawalOperationState,
};

use crate::{ClientScope, Wallet};

const COLLECTION_RECEIPT_TOTAL_KEY: &[u8] = b"total";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CollectionOperationMeta {
    fman_guardian_fee_collection: u8,
}

const COLLECTION_META: CollectionOperationMeta = CollectionOperationMeta {
    fman_guardian_fee_collection: 1,
};

#[derive(Clone, Copy, Debug, Default)]
struct Reconciled {
    newly_claimed: Amount,
    recorded_claimed: Amount,
}

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
        let _collection_guard = self
            .collection_exclusion(ClientScope::Guardian {
                federation_id: invite_code.federation_id(),
                seat_id: seat_id.to_string(),
            })
            .await;
        let module = sp_module(&client)?;
        let account = signing_account(&module, key)?;
        collect_operations(&FedimintCollectionOperations {
            client: &client,
            receipts: client.db().with_prefix(collection_receipt_prefix()),
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
    type Operation: Clone + Send;

    /// Resume durable collection operations whose original caller vanished.
    /// Successes returned here were never reported: terminal streams are
    /// drained and cached before an ordinary call can return them.
    async fn reconcile_unobserved(
        &self,
    ) -> Result<Reconciled, (CollectionFailurePhase, Reconciled)>;

    async fn receipt(&self, operation: &Self::Operation) -> anyhow::Result<Option<Amount>>;

    async fn recorded_claimed(&self) -> anyhow::Result<Amount>;

    async fn record_receipt(
        &self,
        operation: &Self::Operation,
        amount: Amount,
    ) -> anyhow::Result<Amount>;

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
    /// Seat-scoped client whose operation log is the recovery journal.
    client: &'a ClientHandleArc,
    /// FMan-owned receipts isolated inside this seat-scoped client database.
    receipts: Database,
    /// Stability-pool client module.
    module: &'a ClientModuleInstance<'a, StabilityPoolClientModule>,
    /// Account selected and checked against the seat key.
    account_id: stability_pool_common::AccountId,
}

#[async_trait::async_trait]
impl CollectionOperations for FedimintCollectionOperations<'_> {
    type Operation = OperationId;

    async fn reconcile_unobserved(
        &self,
    ) -> Result<Reconciled, (CollectionFailurePhase, Reconciled)> {
        let mut reconciled = Reconciled {
            newly_claimed: Amount::ZERO,
            recorded_claimed: self.recorded_claimed().await.map_err(|_| {
                (
                    CollectionFailurePhase::BalanceRefresh,
                    Reconciled::default(),
                )
            })?,
        };
        // Scan the operation-ID index directly. The chronological paginator
        // starts at wall-clock `now`, so a clock correction can otherwise
        // hide a durable future-dated operation from recovery.
        for (key, entry) in operation_log_entries(self.client.db()).await {
            let Ok(meta) = entry.try_meta::<StabilityPoolMeta>() else {
                continue;
            };
            let (extra_meta, phase) = match meta {
                StabilityPoolMeta::Withdrawal { extra_meta, .. } => {
                    (extra_meta, CollectionFailurePhase::Unlock)
                }
                StabilityPoolMeta::WithdrawIdleBalance { extra_meta, .. } => {
                    (extra_meta, CollectionFailurePhase::IdleClaim)
                }
                _ => continue,
            };
            if serde_json::from_value::<CollectionOperationMeta>(extra_meta)
                .ok()
                .filter(|meta| meta.fman_guardian_fee_collection == 1)
                .is_none()
            {
                continue;
            }
            if self
                .receipt(&key.operation_id)
                .await
                .map_err(|_| (CollectionFailurePhase::Receipt, reconciled))?
                .is_some()
            {
                continue;
            }
            match entry
                .try_outcome::<StabilityPoolWithdrawalOperationState>()
                .map_err(|_| (phase, reconciled))?
            {
                Some(state) if withdrawal_terminal(&state) => {
                    let amount = withdrawal_success_amount(&state).unwrap_or(Amount::ZERO);
                    let recorded_claimed = self
                        .record_receipt(&key.operation_id, amount)
                        .await
                        .map_err(|_| (CollectionFailurePhase::Receipt, reconciled))?;
                    reconciled.recorded_claimed = recorded_claimed;
                    reconciled.newly_claimed = checked_amount_sum(
                        reconciled.newly_claimed,
                        amount,
                        "recovered guardian-fee claims",
                    )
                    .map_err(|_| (phase, reconciled))?;
                    continue;
                }
                Some(_) => return Err((phase, reconciled)),
                None => {}
            }
            let result = match phase {
                CollectionFailurePhase::IdleClaim => self.await_idle(key.operation_id).await,
                CollectionFailurePhase::Unlock => self.await_unlock(key.operation_id).await,
                CollectionFailurePhase::Receipt => unreachable!(),
                CollectionFailurePhase::BalanceRefresh => unreachable!(),
            };
            match result {
                Ok(amount) => {
                    let recorded_claimed = self
                        .record_receipt(&key.operation_id, amount)
                        .await
                        .map_err(|_| (CollectionFailurePhase::Receipt, reconciled))?;
                    reconciled.recorded_claimed = recorded_claimed;
                    reconciled.newly_claimed = checked_amount_sum(
                        reconciled.newly_claimed,
                        amount,
                        "recovered guardian-fee claims",
                    )
                    .map_err(|_| (phase, reconciled))?;
                }
                Err(_) => {
                    // A defined terminal failure is cached after EOF and
                    // must not wedge later work. An uncached error remains
                    // ambiguous and blocks a new snapshot/submission.
                    let terminal = self
                        .client
                        .operation_log()
                        .get_operation(key.operation_id)
                        .await
                        .and_then(|entry| {
                            entry
                                .try_outcome::<StabilityPoolWithdrawalOperationState>()
                                .ok()
                                .flatten()
                                .filter(withdrawal_terminal)
                        })
                        .is_some();
                    if !terminal {
                        return Err((phase, reconciled));
                    }
                    reconciled.recorded_claimed = self
                        .record_receipt(&key.operation_id, Amount::ZERO)
                        .await
                        .map_err(|_| (CollectionFailurePhase::Receipt, reconciled))?;
                }
            }
        }
        Ok(reconciled)
    }

    async fn receipt(&self, operation: &Self::Operation) -> anyhow::Result<Option<Amount>> {
        read_collection_receipt(&self.receipts, operation).await
    }

    async fn recorded_claimed(&self) -> anyhow::Result<Amount> {
        read_recorded_claimed(&self.receipts).await
    }

    async fn record_receipt(
        &self,
        operation: &Self::Operation,
        amount: Amount,
    ) -> anyhow::Result<Amount> {
        record_collection_receipt(&self.receipts, operation, amount).await
    }

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
            .withdraw_idle_balance(AccountType::BtcDepositor, amount, COLLECTION_META)
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
            .withdraw(AccountType::BtcDepositor, FiatOrAll::All, COLLECTION_META)
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

async fn operation_log_entries(database: &Database) -> Vec<(OperationLogKey, OperationLogEntry)> {
    let mut dbtx = database.begin_transaction_nc().await;
    dbtx.find_by_prefix(&OperationLogKeyPrefix)
        .await
        .collect::<Vec<_>>()
        .await
}

fn withdrawal_terminal(state: &StabilityPoolWithdrawalOperationState) -> bool {
    matches!(
        state,
        StabilityPoolWithdrawalOperationState::Success(_)
            | StabilityPoolWithdrawalOperationState::UnlockTxRejected(_)
            | StabilityPoolWithdrawalOperationState::UnlockProcessingError(_)
            | StabilityPoolWithdrawalOperationState::WithdrawalTxRejected(_)
            | StabilityPoolWithdrawalOperationState::PrimaryOutputError(_)
    )
}

fn receipt_key(operation: &OperationId) -> Vec<u8> {
    let mut key = b"operation/".to_vec();
    key.extend(operation.consensus_encode_to_vec());
    key
}

fn collection_receipt_prefix() -> Vec<u8> {
    let mut prefix = vec![fedimint_client::db::DbKeyPrefix::UserData as u8];
    prefix.extend(b"fman/guardian-fee-collection-receipt/v1/");
    prefix
}

async fn read_collection_receipt(
    receipts: &Database,
    operation: &OperationId,
) -> anyhow::Result<Option<Amount>> {
    let mut tx = receipts.begin_transaction_nc().await;
    tx.raw_get_bytes(&receipt_key(operation))
        .await?
        .map(|bytes| {
            Amount::consensus_decode_whole(&bytes, &ModuleDecoderRegistry::default())
                .map_err(anyhow::Error::from)
        })
        .transpose()
}

async fn read_recorded_claimed(receipts: &Database) -> anyhow::Result<Amount> {
    let mut tx = receipts.begin_transaction_nc().await;
    decode_receipt_amount(tx.raw_get_bytes(COLLECTION_RECEIPT_TOTAL_KEY).await?)
}

async fn record_collection_receipt(
    receipts: &Database,
    operation: &OperationId,
    amount: Amount,
) -> anyhow::Result<Amount> {
    let key = receipt_key(operation);
    let value = amount.consensus_encode_to_vec();
    let mut tx = receipts.begin_transaction().await;
    if let Some(existing) = tx.raw_get_bytes(&key).await? {
        anyhow::ensure!(existing == value, "guardian-fee receipt amount changed");
        return decode_receipt_amount(tx.raw_get_bytes(COLLECTION_RECEIPT_TOTAL_KEY).await?);
    }
    let total = decode_receipt_amount(tx.raw_get_bytes(COLLECTION_RECEIPT_TOTAL_KEY).await?)?;
    let total = checked_amount_sum(total, amount, "recorded guardian-fee claims")?;
    tx.raw_insert_bytes(&key, &value).await?;
    tx.raw_insert_bytes(
        COLLECTION_RECEIPT_TOTAL_KEY,
        &total.consensus_encode_to_vec(),
    )
    .await?;
    tx.commit_tx().await;
    Ok(total)
}

fn decode_receipt_amount(value: Option<Vec<u8>>) -> anyhow::Result<Amount> {
    value
        .map(|bytes| {
            Amount::consensus_decode_whole(&bytes, &ModuleDecoderRegistry::default())
                .map_err(anyhow::Error::from)
        })
        .transpose()
        .map(|amount| amount.unwrap_or(Amount::ZERO))
}

fn withdrawal_success_amount(state: &StabilityPoolWithdrawalOperationState) -> Option<Amount> {
    match state {
        StabilityPoolWithdrawalOperationState::Success(amount) => Some(*amount),
        _ => None,
    }
}

/// Run the complete two-step collection workflow over one operation provider.
async fn collect_operations(operations: &impl CollectionOperations) -> anyhow::Result<Collected> {
    let reconciled = match operations.reconcile_unobserved().await {
        Ok(reconciled) => reconciled,
        // Without a durable total there is no truthful cumulative amount to
        // put in an incomplete response. Surface an error instead of
        // fabricating zero.
        Err((CollectionFailurePhase::BalanceRefresh, _)) => {
            return Err(anyhow::anyhow!(
                "read durable guardian-fee collection total"
            ));
        }
        Err((phase, reconciled)) => {
            return Ok(incomplete_after_refresh(
                operations,
                reconciled.newly_claimed,
                reconciled.recorded_claimed,
                phase,
                true,
            )
            .await);
        }
    };
    let mut confirmed_claimed = reconciled.newly_claimed;
    let mut recorded_claimed = reconciled.recorded_claimed;
    let before = operations
        .balances()
        .await
        .context("read guardian-fee account state")?;
    let before_awaiting_cycle = checked_amount_sum(
        before.staged,
        before.locked,
        "guardian-fee awaiting-cycle balance",
    )?;
    let mut operation_exists = recorded_claimed != Amount::ZERO;

    if before.idle != Amount::ZERO {
        let operation = operations.submit_idle(before.idle).await?;
        operation_exists = true;
        match operations.await_idle(operation.clone()).await {
            Ok(claimed) => {
                let Ok(committed_total) = operations.record_receipt(&operation, claimed).await
                else {
                    return Ok(incomplete_after_refresh(
                        operations,
                        confirmed_claimed,
                        recorded_claimed,
                        CollectionFailurePhase::Receipt,
                        true,
                    )
                    .await);
                };
                confirmed_claimed = checked_amount_sum(
                    confirmed_claimed,
                    claimed,
                    "confirmed guardian-fee claims",
                )?;
                recorded_claimed = committed_total;
            }
            Err(_) => {
                return Ok(incomplete_after_refresh(
                    operations,
                    confirmed_claimed,
                    recorded_claimed,
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
                    recorded_claimed,
                    CollectionFailurePhase::Unlock,
                    false,
                )
                .await);
            }
        };
        operation_exists = true;
        match operations.await_unlock(operation.clone()).await {
            Ok(claimed) => {
                let Ok(committed_total) = operations.record_receipt(&operation, claimed).await
                else {
                    return Ok(incomplete_after_refresh(
                        operations,
                        confirmed_claimed,
                        recorded_claimed,
                        CollectionFailurePhase::Receipt,
                        true,
                    )
                    .await);
                };
                confirmed_claimed = checked_amount_sum(
                    confirmed_claimed,
                    claimed,
                    "confirmed guardian-fee claims",
                )?;
                recorded_claimed = committed_total;
            }
            Err(_) => {
                return Ok(incomplete_after_refresh(
                    operations,
                    confirmed_claimed,
                    recorded_claimed,
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
                recorded_claimed,
                awaiting_cycle,
            }),
            Err(_) if operation_exists => Ok(incomplete(
                confirmed_claimed,
                recorded_claimed,
                None,
                CollectionFailurePhase::BalanceRefresh,
                false,
            )),
            Err(error) => Err(error),
        },
        Err(_) if operation_exists => Ok(incomplete(
            confirmed_claimed,
            recorded_claimed,
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
    recorded_claimed: Amount,
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
    incomplete(
        confirmed_claimed,
        recorded_claimed,
        observed,
        phase,
        operation_submitted,
    )
}

/// Add two dependency-supplied amounts without wrapping monetary state.
fn checked_amount_sum(left: Amount, right: Amount, what: &str) -> anyhow::Result<Amount> {
    left.checked_add(right)
        .ok_or_else(|| anyhow::anyhow!("{what} exceeds the representable amount"))
}

/// Build a structural incomplete result for core to project safely.
fn incomplete(
    confirmed_claimed: Amount,
    recorded_claimed: Amount,
    observed_awaiting_cycle: Option<Amount>,
    phase: CollectionFailurePhase,
    operation_submitted: bool,
) -> Collected {
    Collected::Incomplete {
        confirmed_claimed,
        recorded_claimed,
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
    let mut terminal = None;
    while let Some(state) = updates.next().await {
        let result = match state {
            StabilityPoolWithdrawalOperationState::Success(amount) => Some(Ok(amount)),
            StabilityPoolWithdrawalOperationState::UnlockTxRejected(err)
            | StabilityPoolWithdrawalOperationState::UnlockProcessingError(err)
            | StabilityPoolWithdrawalOperationState::WithdrawalTxRejected(err)
            | StabilityPoolWithdrawalOperationState::PrimaryOutputError(err) => {
                Some(Err(format!("guardian-fee {what} failed: {err}")))
            }
            _ => None,
        };
        if let Some(result) = result {
            anyhow::ensure!(
                terminal.is_none(),
                "guardian-fee {what} produced contradictory terminal states"
            );
            terminal = Some(result);
        }
    }
    match terminal {
        Some(Ok(amount)) => Ok(amount),
        Some(Err(error)) => anyhow::bail!(error),
        None => anyhow::bail!("guardian-fee {what} ended without a terminal state"),
    }
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
