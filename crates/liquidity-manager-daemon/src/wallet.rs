//! The provider wallet: the gatewayd-backed backend, and the durable record of
//! every operation against it.
//!
//! An outflow is written down before it is sent and reconciled against chain
//! evidence afterwards, so a send that may have happened is `in_doubt` rather
//! than lost or repeated. See
//! [SPEC-flip-funding-safety](../specs/SPEC-flip-funding-safety.md).

#[cfg(test)]
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::address::NetworkUnchecked;
use bitcoin::{Address, Network};
use fedi_decentralized_service_liquidity_manager::{
    AdminFailure, BitcoinNetwork, FederationId, ItemId, ListResponse, PageRequest, Sats,
    ServiceResult, SetupConfigView, TimeRange, Timestamp, WalletOperation, WalletOperationId,
    WalletOperationStatus, WalletOperationSummary, WalletOperationType,
};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::BitcoinAmountOrAll;
use fedimint_core::util::SafeUrl;
use fedimint_gateway_common::{
    GatewayBalances, GatewayInfo, LightningInfo, SendOnchainRequest, V1_API_ENDPOINT,
};
use fedimint_ln_common::client::GatewayApi;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, Transaction};
#[cfg(test)]
use tokio::sync::{Mutex, Notify};

use crate::allocation_store::RESERVING_ITEM_STATUSES;
use crate::chain_observer::ChainOutputEvidence;
use crate::database::{Database, OffsetPage, push_in_list};
use crate::{internal_error, invalid_argument, not_found, now_timestamp, to_i64_sats};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletBackendBalance {
    pub network: BitcoinNetwork,
    pub spendable: Sats,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedWithdrawal {
    pub operation_id: WalletOperationId,
    pub address: Address,
    pub amount: Sats,
    pub fee_rate_sat_per_vbyte: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SubmittedWithdrawal {
    pub txid: String,
}

/// How a withdrawal submission can fail. `Failed` asserts the send provably
/// did not happen (safe to retry); `InDoubt` means the outcome is unknown and
/// must be reconciled before retrying.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SubmitWithdrawalError {
    // The gatewayd backend maps every submit error to `InDoubt`; only the
    // test wallet constructs `Failed` today, pinning the consumer paths for
    // backends that can prove a send never happened.
    #[allow(dead_code)]
    Failed(String),
    InDoubt(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WalletOperationSync {
    pub operation_id: WalletOperationId,
    pub status: SyncedWalletStatus,
    pub txid: Option<String>,
    pub confirmation_count: Option<u32>,
    pub amount: Option<Sats>,
    pub detail: Option<String>,
}

/// Statuses a wallet/chain sync can assign to an existing operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum SyncedWalletStatus {
    Broadcast,
    Confirmed,
    Completed,
}

#[async_trait]
pub(crate) trait FundsWallet: Send + Sync {
    async fn network(&self) -> anyhow::Result<BitcoinNetwork>;
    async fn balance_summary(&self) -> anyhow::Result<WalletBackendBalance>;
    async fn allocate_deposit_address(
        &self,
        operation_id: &WalletOperationId,
        label: Option<&str>,
    ) -> anyhow::Result<String>;
    async fn prepare_withdrawal(
        &self,
        operation_id: &WalletOperationId,
        address: &str,
        amount: Sats,
        fee_rate_sat_per_vbyte: u64,
    ) -> anyhow::Result<PreparedWithdrawal>;
    async fn submit_prepared_withdrawal(
        &self,
        prepared: PreparedWithdrawal,
    ) -> Result<SubmittedWithdrawal, SubmitWithdrawalError>;
    async fn sync_operations(&self) -> anyhow::Result<Vec<WalletOperationSync>>;
}

#[derive(Clone)]
pub(crate) struct GatewaydFundsWallet {
    config: SetupConfigView,
    api: GatewayApi,
    base_url: SafeUrl,
}

impl GatewaydFundsWallet {
    pub(crate) async fn new(
        config: SetupConfigView,
        admin_credential: String,
    ) -> anyhow::Result<Self> {
        let connectors = ConnectorRegistry::build_from_client_defaults()
            .bind()
            .await?;
        let base_url = SafeUrl::parse(&config.gateway.admin_url)?.join(V1_API_ENDPOINT)?;
        Ok(Self {
            config,
            api: GatewayApi::new(Some(admin_credential), connectors),
            base_url,
        })
    }
}

#[async_trait]
impl FundsWallet for GatewaydFundsWallet {
    async fn network(&self) -> anyhow::Result<BitcoinNetwork> {
        let info: GatewayInfo =
            fedimint_gateway_client::get_info(&self.api, &self.base_url).await?;
        match info.lightning_info {
            LightningInfo::Connected {
                network,
                synced_to_chain,
                ..
            } => {
                if !synced_to_chain {
                    anyhow::bail!("gatewayd lightning node is not synced to chain");
                }
                Ok(bitcoin_network_to_domain(network))
            }
            LightningInfo::NotConnected => {
                anyhow::bail!("gatewayd lightning node is not connected")
            }
        }
    }

    async fn balance_summary(&self) -> anyhow::Result<WalletBackendBalance> {
        let network = self.network().await?;
        let balances: GatewayBalances =
            fedimint_gateway_client::get_balances(&self.api, &self.base_url).await?;
        Ok(WalletBackendBalance {
            network,
            spendable: Sats(balances.onchain_balance_sats),
            observed_at: now_timestamp(),
        })
    }

    async fn allocate_deposit_address(
        &self,
        _operation_id: &WalletOperationId,
        _label: Option<&str>,
    ) -> anyhow::Result<String> {
        let expected_network = domain_network_to_bitcoin(self.config.network);
        let address =
            fedimint_gateway_client::get_ln_onchain_address(&self.api, &self.base_url).await?;
        let address = address.require_network(expected_network)?;
        Ok(address.to_string())
    }

    async fn prepare_withdrawal(
        &self,
        operation_id: &WalletOperationId,
        address: &str,
        amount: Sats,
        fee_rate_sat_per_vbyte: u64,
    ) -> anyhow::Result<PreparedWithdrawal> {
        if amount.0 == 0 {
            anyhow::bail!("withdrawal amount must be greater than zero");
        }
        let expected_network = domain_network_to_bitcoin(self.config.network);
        let address = address.parse::<Address<NetworkUnchecked>>()?;
        let address = address.require_network(expected_network)?;
        Ok(PreparedWithdrawal {
            operation_id: operation_id.clone(),
            address,
            amount,
            fee_rate_sat_per_vbyte,
        })
    }

    async fn submit_prepared_withdrawal(
        &self,
        prepared: PreparedWithdrawal,
    ) -> Result<SubmittedWithdrawal, SubmitWithdrawalError> {
        let txid = fedimint_gateway_client::send_onchain(
            &self.api,
            &self.base_url,
            SendOnchainRequest {
                address: prepared.address.as_unchecked().clone(),
                amount: BitcoinAmountOrAll::Amount(bitcoin::Amount::from_sat(prepared.amount.0)),
                fee_rate_sats_per_vbyte: prepared.fee_rate_sat_per_vbyte,
            },
        )
        .await
        .map_err(|error| SubmitWithdrawalError::InDoubt(error.to_string()))?;
        Ok(SubmittedWithdrawal {
            txid: txid.to_string(),
        })
    }

    async fn sync_operations(&self) -> anyhow::Result<Vec<WalletOperationSync>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct TestFundsWallet {
    inner: Arc<Mutex<TestFundsWalletState>>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct TestFundsWalletState {
    network: BitcoinNetwork,
    spendable: Sats,
    next_address: String,
    submit_result: TestSubmitResult,
    /// Makes `prepare_withdrawal` fail for a well-formed address, which the
    /// address parse alone cannot produce. This is what reaches the
    /// "nothing was sent" branch for an address a real deployment would use.
    prepare_failure: Option<String>,
    submitted: Vec<PreparedWithdrawal>,
    submit_before_barrier: Option<(Arc<Notify>, Arc<Notify>)>,
    submit_barrier: Option<(Arc<Notify>, Arc<Notify>)>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
enum TestSubmitResult {
    Success(String),
    InDoubt(String),
    Failed(String),
}

#[cfg(test)]
impl TestFundsWallet {
    pub(crate) fn new(network: BitcoinNetwork, spendable: Sats, next_address: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TestFundsWalletState {
                network,
                spendable,
                next_address,
                submit_result: TestSubmitResult::Success(test_txid()),
                prepare_failure: None,
                submitted: Vec::new(),
                submit_before_barrier: None,
                submit_barrier: None,
            })),
        }
    }

    pub(crate) async fn set_submit_in_doubt(&self, detail: impl Into<String>) {
        self.inner.lock().await.submit_result = TestSubmitResult::InDoubt(detail.into());
    }

    pub(crate) async fn set_submit_success(&self, txid: impl Into<String>) {
        self.inner.lock().await.submit_result = TestSubmitResult::Success(txid.into());
    }

    pub(crate) async fn set_submit_failed(&self, detail: impl Into<String>) {
        self.inner.lock().await.submit_result = TestSubmitResult::Failed(detail.into());
    }

    /// Refuses the next `prepare_withdrawal`, so a test can reach the branch
    /// where the send provably never happened.
    pub(crate) async fn set_prepare_failed(&self, detail: impl Into<String>) {
        self.inner.lock().await.prepare_failure = Some(detail.into());
    }

    pub(crate) async fn submitted_count(&self) -> usize {
        self.inner.lock().await.submitted.len()
    }

    pub(crate) async fn pause_submission(&self) -> (Arc<Notify>, Arc<Notify>) {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        self.inner.lock().await.submit_barrier = Some((started.clone(), release.clone()));
        (started, release)
    }

    /// Pauses a test before its prepared withdrawal reaches the wallet backend.
    pub(crate) async fn pause_before_submission(&self) -> (Arc<Notify>, Arc<Notify>) {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        self.inner.lock().await.submit_before_barrier = Some((started.clone(), release.clone()));
        (started, release)
    }
}

#[cfg(test)]
#[async_trait]
impl FundsWallet for TestFundsWallet {
    async fn network(&self) -> anyhow::Result<BitcoinNetwork> {
        Ok(self.inner.lock().await.network)
    }

    async fn balance_summary(&self) -> anyhow::Result<WalletBackendBalance> {
        let state = self.inner.lock().await;
        Ok(WalletBackendBalance {
            network: state.network,
            spendable: state.spendable,
            observed_at: now_timestamp(),
        })
    }

    async fn allocate_deposit_address(
        &self,
        _operation_id: &WalletOperationId,
        _label: Option<&str>,
    ) -> anyhow::Result<String> {
        Ok(self.inner.lock().await.next_address.clone())
    }

    async fn prepare_withdrawal(
        &self,
        operation_id: &WalletOperationId,
        address: &str,
        amount: Sats,
        fee_rate_sat_per_vbyte: u64,
    ) -> anyhow::Result<PreparedWithdrawal> {
        if let Some(detail) = self.inner.lock().await.prepare_failure.clone() {
            anyhow::bail!("{detail}");
        }
        let network = domain_network_to_bitcoin(self.inner.lock().await.network);
        let address = address.parse::<Address<NetworkUnchecked>>()?;
        let address = address.require_network(network)?;
        Ok(PreparedWithdrawal {
            operation_id: operation_id.clone(),
            address,
            amount,
            fee_rate_sat_per_vbyte,
        })
    }

    async fn submit_prepared_withdrawal(
        &self,
        prepared: PreparedWithdrawal,
    ) -> Result<SubmittedWithdrawal, SubmitWithdrawalError> {
        let before_barrier = self.inner.lock().await.submit_before_barrier.clone();
        if let Some((started, release)) = before_barrier {
            started.notify_one();
            release.notified().await;
        }
        let (result, barrier) = {
            let mut state = self.inner.lock().await;
            state.submitted.push(prepared);
            (state.submit_result.clone(), state.submit_barrier.clone())
        };
        if let Some((started, release)) = barrier {
            started.notify_one();
            release.notified().await;
        }
        match result {
            TestSubmitResult::Success(txid) => Ok(SubmittedWithdrawal { txid }),
            TestSubmitResult::InDoubt(detail) => Err(SubmitWithdrawalError::InDoubt(detail)),
            TestSubmitResult::Failed(detail) => Err(SubmitWithdrawalError::Failed(detail)),
        }
    }

    async fn sync_operations(&self) -> anyhow::Result<Vec<WalletOperationSync>> {
        Ok(Vec::new())
    }
}

pub(crate) fn domain_network_to_bitcoin(network: BitcoinNetwork) -> Network {
    match network {
        BitcoinNetwork::Bitcoin => Network::Bitcoin,
        BitcoinNetwork::Testnet => Network::Testnet,
        BitcoinNetwork::Signet => Network::Signet,
        BitcoinNetwork::Regtest => Network::Regtest,
    }
}

pub(crate) fn bitcoin_network_to_domain(network: Network) -> BitcoinNetwork {
    match network {
        Network::Bitcoin => BitcoinNetwork::Bitcoin,
        Network::Testnet | Network::Testnet4 => BitcoinNetwork::Testnet,
        Network::Signet => BitcoinNetwork::Signet,
        Network::Regtest => BitcoinNetwork::Regtest,
    }
}

#[cfg(test)]
fn test_txid() -> String {
    "0000000000000000000000000000000000000000000000000000000000000000".to_owned()
}

/// Wallet-operation types that debit the provider wallet.
///
/// `Deposit` is deliberately absent: it moves the balance the other way, and a
/// capacity subtraction that included it would charge the provider for money
/// arriving.
pub(crate) const OUTGOING_OPERATION_TYPES: &[WalletOperationType] = &[
    WalletOperationType::Withdrawal,
    WalletOperationType::GatewayFunding,
    WalletOperationType::StabilityPoolFunding,
];

/// Wallet-operation statuses that still await settlement (and reserve wallet
/// funds). The `idx_wallet_operations_active` partial index in the baseline
/// migration restates this list as SQL literals; a test pins the two
/// together, so adding a status here without the index fails CI rather than
/// silently narrowing the index.
pub(crate) const PENDING_SETTLEMENT_STATUSES: &[WalletOperationStatus] = &[
    WalletOperationStatus::Pending,
    WalletOperationStatus::Broadcast,
    WalletOperationStatus::Confirmed,
    WalletOperationStatus::InDoubt,
    WalletOperationStatus::ManualReviewRequired,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WalletOperationInput {
    pub operation_id: WalletOperationId,
    pub operation_type: WalletOperationType,
    pub status: WalletOperationStatus,
    pub amount: Sats,
    pub address: Option<String>,
    pub label: Option<String>,
    pub fee_rate_sat_per_vbyte: Option<u64>,
    pub federation_id: Option<FederationId>,
    pub item_id: Option<ItemId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WalletOperationPageRequest {
    pub page: PageRequest,
    pub status_filter: Option<WalletOperationStatus>,
    pub time_range: Option<TimeRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WalletAccountingSums {
    pub pending_incoming: Sats,
    pub pending_outgoing: Sats,
    pub in_flight_allocations: Sats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WalletBalanceObservation {
    pub network: String,
    pub spendable: Sats,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ChainEvidenceClaim {
    Applied(Box<WalletOperation>),
    NoMatch,
    Ambiguous { candidate_count: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperatorWithdrawalIntent {
    pub operation_id: WalletOperationId,
    pub address: String,
    pub amount: Sats,
    pub fee_rate_sat_per_vbyte: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct OperationMetadata {
    label: Option<String>,
}

pub(crate) async fn insert_wallet_operation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    input: &WalletOperationInput,
) -> ServiceResult<()> {
    let metadata = OperationMetadata {
        label: input.label.clone(),
    };
    let operation_json = serde_json::to_string(&metadata).map_err(internal_error)?;
    sqlx::query(
        "INSERT INTO wallet_operations \
         (operation_id, operation_type, status, operation_json, federation_id, item_id, \
          amount_sats, address, label, fee_rate_sat_per_vbyte, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())",
    )
    .bind(&input.operation_id.0)
    .bind(input.operation_type.to_string())
    .bind(input.status.to_string())
    .bind(operation_json)
    .bind(input.federation_id.as_ref().map(|id| id.0.as_str()))
    .bind(input.item_id.as_ref().map(|id| id.0.as_str()))
    .bind(to_i64_sats(input.amount)?)
    .bind(input.address.as_deref())
    .bind(input.label.as_deref())
    .bind(input.fee_rate_sat_per_vbyte.map(u64_to_i64).transpose()?)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

/// Binds the operator's intent id to the operation, under a compare-and-set on
/// the status the caller just wrote.
///
/// The predicate is the fence every irreversible call gets: the last durable
/// write before `submit_prepared_withdrawal` re-asserts the state it is acting
/// on rather than trusting the caller's reading of it. Today the caller inserts
/// the row as
/// `in_doubt` inside this same `BEGIN IMMEDIATE` transaction, so the predicate
/// cannot fail — that is deliberate. It fences the site uniformly with the other
/// two irreversible calls, so a later change that moved the insert out of this
/// transaction, or that reached this write from a resume path, would be refused
/// here instead of sending twice.
pub(crate) async fn bind_operator_withdrawal_intent_tx(
    tx: &mut Transaction<'_, Sqlite>,
    operation_id: &WalletOperationId,
    withdrawal_intent_id: &str,
) -> ServiceResult<()> {
    let result = sqlx::query(
        "UPDATE wallet_operations \
         SET withdrawal_intent_id = ?, submitted_at = unixepoch(), updated_at = unixepoch() \
         WHERE operation_id = ? AND status = ?",
    )
    .bind(withdrawal_intent_id)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::InDoubt.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    if result.rows_affected() != 1 {
        return Err(crate::failed_precondition(
            "wallet operation left in_doubt before its withdrawal intent was bound",
        ));
    }
    Ok(())
}

pub(crate) async fn operator_withdrawal_for_intent(
    database: &Database,
    withdrawal_intent_id: &str,
) -> ServiceResult<Option<OperatorWithdrawalIntent>> {
    let row = sqlx::query(
        "SELECT operation_id, address, amount_sats, fee_rate_sat_per_vbyte \
         FROM wallet_operations WHERE withdrawal_intent_id = ?",
    )
    .bind(withdrawal_intent_id)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    row.as_ref().map(operator_withdrawal_from_row).transpose()
}

pub(crate) async fn operator_withdrawal_for_intent_tx(
    tx: &mut Transaction<'_, Sqlite>,
    withdrawal_intent_id: &str,
) -> ServiceResult<Option<OperatorWithdrawalIntent>> {
    let row = sqlx::query(
        "SELECT operation_id, address, amount_sats, fee_rate_sat_per_vbyte \
         FROM wallet_operations WHERE withdrawal_intent_id = ?",
    )
    .bind(withdrawal_intent_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?;
    row.as_ref().map(operator_withdrawal_from_row).transpose()
}

fn operator_withdrawal_from_row(row: &SqliteRow) -> ServiceResult<OperatorWithdrawalIntent> {
    Ok(OperatorWithdrawalIntent {
        operation_id: WalletOperationId(row.try_get("operation_id").map_err(internal_error)?),
        address: row.try_get("address").map_err(internal_error)?,
        amount: Sats(i64_to_u64(
            row.try_get("amount_sats").map_err(internal_error)?,
        )?),
        fee_rate_sat_per_vbyte: i64_to_u64(
            row.try_get("fee_rate_sat_per_vbyte")
                .map_err(internal_error)?,
        )?,
    })
}

pub(crate) async fn mark_withdrawal_broadcast(
    database: &Database,
    operation_id: &WalletOperationId,
    txid: &str,
) -> ServiceResult<WalletOperation> {
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, txid = CASE WHEN tx_vout IS NULL THEN ? ELSE txid END, failure_json = NULL, \
             submitted_at = COALESCE(submitted_at, unixepoch()), updated_at = unixepoch() \
         WHERE operation_id = ? AND status IN (?, ?, ?, ?)",
    )
    .bind(WalletOperationStatus::Broadcast.to_string())
    .bind(txid)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(WalletOperationStatus::Broadcast.to_string())
    .bind(WalletOperationStatus::Confirmed.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    get_wallet_operation(database, operation_id).await
}

pub(crate) async fn mark_operation_in_doubt(
    database: &Database,
    operation_id: &WalletOperationId,
    detail: &str,
) -> ServiceResult<WalletOperation> {
    let failure = AdminFailure {
        code: "in_doubt".to_owned(),
        message: detail.to_owned(),
        occurred_at: now_timestamp(),
        federation_id: None,
        item_id: None,
    };
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, failure_json = ?, submitted_at = COALESCE(submitted_at, unixepoch()), updated_at = unixepoch() \
         WHERE operation_id = ? AND status IN (?, ?)",
    )
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(serde_json::to_string(&failure).map_err(internal_error)?)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(WalletOperationStatus::InDoubt.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    get_wallet_operation(database, operation_id).await
}

/// Escalates one `in_doubt` operation to `manual_review_required` when it has
/// been unresolved for longer than the operator's review threshold.
///
/// The age test lives in the same statement as the write, and the write is
/// guarded on the operation still being `in_doubt`, so evidence that settles it
/// concurrently wins and terminal states stay monotonic. Returns whether the
/// escalation happened.
pub(crate) async fn escalate_in_doubt_to_manual_review(
    database: &Database,
    operation_id: &WalletOperationId,
    review_after_secs: u64,
    detail: &str,
) -> ServiceResult<bool> {
    let failure = AdminFailure {
        code: "manual_review_required".to_owned(),
        message: detail.to_owned(),
        occurred_at: now_timestamp(),
        federation_id: None,
        item_id: None,
    };
    let result = sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, failure_json = ?, updated_at = unixepoch() \
         WHERE operation_id = ? AND status = ? \
           AND COALESCE(submitted_at, updated_at) <= unixepoch() - ?",
    )
    .bind(WalletOperationStatus::ManualReviewRequired.to_string())
    .bind(serde_json::to_string(&failure).map_err(internal_error)?)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(u64_to_i64(review_after_secs)?)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(result.rows_affected() > 0)
}

/// Applies an operator's resolution of a `manual_review_required` operation.
///
/// Every arm is guarded on the operation still being under manual review, so a
/// resolution cannot be applied twice and cannot overwrite a state something
/// else reached first. Returns whether the resolution was applied.
pub(crate) async fn resolve_manual_review_tx(
    tx: &mut Transaction<'_, Sqlite>,
    operation_id: &WalletOperationId,
    resolution: &ManualReviewOutcome,
) -> ServiceResult<bool> {
    let under_review = WalletOperationStatus::ManualReviewRequired.to_string();
    let result = match resolution {
        // The operator has established the send did happen and named the
        // transaction. `tx_vout` stays unset: chain observation owns exact
        // output attribution, and an operator-supplied txid is not that.
        //
        // The watermark is stamped here for the same reason
        // `claim_chain_evidence` stamps it: this row is settling, so the debit
        // has left the wallet, but the balance observation admission reads may
        // still predate it. Until the observation advances past this sequence,
        // `active_wallet_withdrawal_amount_tx` must keep charging the amount.
        // The plain count is right here, and only here: this writer runs inside
        // the sync task, whose passes are serial with its own backend reads, so
        // the next observation provably read after this settle.
        //
        // Omitting it overcommits. The row leaves the pending-settlement
        // statuses on completion and then matches neither branch of that query,
        // so a truthfully-resolved send stops being counted by the reserved
        // term *and* by the unsettled term at once, and the next admission sees
        // capacity that has already been spent. `COALESCE` keeps any stamp
        // chain evidence already wrote.
        //
        // The plain count, like every other stamp. No `+ 1` compensation is
        // needed: observations record when their read *began*, so a resolution
        // landing between an observation's read and its write cannot be
        // released by an observation that read before the debit.
        ManualReviewOutcome::Completed { txid } => sqlx::query(
            "UPDATE wallet_operations \
             SET status = ?, txid = ?, failure_json = NULL, \
                 completed_at = unixepoch(), \
                 settled_tick = COALESCE( \
                   settled_tick, \
                   (SELECT tick FROM wallet_observation_ticks WHERE id = 1), \
                   0), \
                 updated_at = unixepoch() \
             WHERE operation_id = ? AND status = ?",
        )
        .bind(WalletOperationStatus::Completed.to_string())
        .bind(txid)
        .bind(&operation_id.0)
        .bind(&under_review),
        // The send did not happen and is not to be attempted again.
        ManualReviewOutcome::Failed { reason } => sqlx::query(
            "UPDATE wallet_operations \
             SET status = ?, failure_json = ?, updated_at = unixepoch() \
             WHERE operation_id = ? AND status = ?",
        )
        .bind(WalletOperationStatus::Failed.to_string())
        .bind(
            serde_json::to_string(&AdminFailure {
                code: "manual_review_failed".to_owned(),
                message: reason.clone(),
                occurred_at: now_timestamp(),
                federation_id: None,
                item_id: None,
            })
            .map_err(internal_error)?,
        )
        .bind(&operation_id.0)
        .bind(&under_review),
        // The send did not happen and may be made again. Returning the
        // operation to `pending` is what lifts the never-auto-resubmit rule:
        // it applies to `in_doubt` and `manual_review_required` precisely
        // because nobody has established what happened, and here somebody has.
        ManualReviewOutcome::SafeToRetry => sqlx::query(
            "UPDATE wallet_operations \
             SET status = ?, txid = NULL, tx_vout = NULL, confirmation_count = NULL, \
                 failure_json = NULL, submitted_at = NULL, completed_at = NULL, \
                 updated_at = unixepoch() \
             WHERE operation_id = ? AND status = ?",
        )
        .bind(WalletOperationStatus::Pending.to_string())
        .bind(&operation_id.0)
        .bind(&under_review),
    };
    let result = result.execute(&mut **tx).await.map_err(internal_error)?;
    Ok(result.rows_affected() > 0)
}

/// What an operator concluded about a wallet send held for manual review.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ManualReviewOutcome {
    /// The send settled on chain as the named transaction.
    Completed { txid: String },

    /// The send did not happen, and is not to be retried.
    Failed { reason: String },

    /// The send did not happen, and may be attempted again.
    SafeToRetry,
}

pub(crate) async fn mark_operation_failed(
    database: &Database,
    operation_id: &WalletOperationId,
    detail: &str,
) -> ServiceResult<WalletOperation> {
    let failure = AdminFailure {
        code: "wallet_submission_failed".to_owned(),
        message: detail.to_owned(),
        occurred_at: now_timestamp(),
        federation_id: None,
        item_id: None,
    };
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, failure_json = ?, updated_at = unixepoch() \
         WHERE operation_id = ? AND status IN (?, ?)",
    )
    .bind(WalletOperationStatus::Failed.to_string())
    .bind(serde_json::to_string(&failure).map_err(internal_error)?)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(WalletOperationStatus::InDoubt.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    get_wallet_operation(database, operation_id).await
}

/// Resets a wallet operation to `pending` and clears its broadcast/outcome
/// material, inside the caller's transaction. Used by admin manual
/// operations.
pub(crate) async fn reset_wallet_operation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    operation_id: &WalletOperationId,
) -> ServiceResult<()> {
    // Both watermarks are cleared with the rest of the send. Their writers all
    // guard on the column being NULL, so a row reset and sent again would keep
    // the stamp from the first send and could never be charged for the second.
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, txid = NULL, tx_vout = NULL, confirmation_count = NULL, failure_json = NULL, \
             submitted_at = NULL, completed_at = NULL, \
             settled_tick = NULL, released_tick = NULL, \
             updated_at = unixepoch() \
         WHERE operation_id = ?",
    )
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(&operation_id.0)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

/// Marks a wallet operation `cancelled` with the given reason, inside the
/// caller's transaction. Used by admin manual operations.
pub(crate) async fn cancel_wallet_operation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    operation_id: &WalletOperationId,
    reason: &str,
) -> ServiceResult<()> {
    let failure = AdminFailure {
        code: "cancelled".to_owned(),
        message: reason.to_owned(),
        occurred_at: now_timestamp(),
        federation_id: None,
        item_id: None,
    };
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, failure_json = ?, updated_at = unixepoch() \
         WHERE operation_id = ?",
    )
    .bind(WalletOperationStatus::Cancelled.to_string())
    .bind(serde_json::to_string(&failure).map_err(internal_error)?)
    .bind(&operation_id.0)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn apply_sync_update(
    database: &Database,
    sync: &WalletOperationSync,
) -> ServiceResult<WalletOperation> {
    let failure_json = sync
        .detail
        .as_ref()
        .map(|detail| {
            serde_json::to_string(&AdminFailure {
                code: "wallet_sync".to_owned(),
                message: detail.clone(),
                occurred_at: now_timestamp(),
                federation_id: None,
                item_id: None,
            })
            .map_err(internal_error)
        })
        .transpose()?;
    // An operation leaving the pending-settlement set records the observation
    // count current at that moment, so admission can tell whether any persisted
    // balance was read after this settlement was seen.
    let settles = !PENDING_SETTLEMENT_STATUSES
        .iter()
        .any(|status| status.to_string() == sync.status.to_string());
    sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, \
             txid = CASE WHEN tx_vout IS NULL THEN COALESCE(?, txid) ELSE txid END, \
             confirmation_count = COALESCE(?, confirmation_count), \
             amount_sats = CASE WHEN ? IS NULL THEN amount_sats ELSE ? END, \
             failure_json = CASE WHEN ? IS NULL THEN failure_json ELSE ? END, \
             completed_at = CASE WHEN ? THEN COALESCE(completed_at, unixepoch()) ELSE completed_at END, \
             settled_tick = CASE WHEN ? AND settled_tick IS NULL \
               THEN (SELECT tick FROM wallet_observation_ticks WHERE id = 1) \
               ELSE settled_tick END, \
             updated_at = unixepoch() \
         WHERE operation_id = ? AND status IN (?, ?, ?, ?)",
    )
    .bind(sync.status.to_string())
    .bind(sync.txid.as_deref())
    .bind(sync.confirmation_count.map(i64::from))
    .bind(sync.amount.map(to_i64_sats).transpose()?)
    .bind(sync.amount.map(to_i64_sats).transpose()?)
    .bind(sync.detail.as_deref())
    .bind(failure_json)
    .bind(matches!(sync.status, SyncedWalletStatus::Completed))
    .bind(settles)
    .bind(&sync.operation_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(WalletOperationStatus::Broadcast.to_string())
    .bind(WalletOperationStatus::Confirmed.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    get_wallet_operation(database, &sync.operation_id).await
}

/// Selects and exclusively claims exact chain-output evidence while holding
/// SQLite's write lock. The same output therefore cannot race into two wallet
/// operations, even when independent sync attempts observed it concurrently.
pub(crate) async fn claim_chain_evidence(
    database: &Database,
    operation_id: &WalletOperationId,
    observed_outputs: &[ChainOutputEvidence],
    required_confirmations: u32,
) -> ServiceResult<ChainEvidenceClaim> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let row = sqlx::query(wallet_operation_select_sql())
        .bind(&operation_id.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found("wallet operation not found"))?;
    let operation = wallet_operation_from_row(&row)?;
    if !PENDING_SETTLEMENT_STATUSES.contains(&operation.status)
        || operation.status == WalletOperationStatus::ManualReviewRequired
    {
        tx.commit().await.map_err(internal_error)?;
        return Ok(ChainEvidenceClaim::NoMatch);
    }
    let Some(expected_address) = operation.address.as_deref() else {
        tx.commit().await.map_err(internal_error)?;
        return Ok(ChainEvidenceClaim::NoMatch);
    };

    let mut candidates = Vec::new();
    for output in observed_outputs {
        if output.address.as_deref() != Some(expected_address)
            || output.amount_sats == 0
            || (operation.amount.0 != 0 && output.amount_sats != operation.amount.0)
            || operation
                .txid
                .as_deref()
                .is_some_and(|txid| txid != output.txid)
            || operation.tx_vout.is_some_and(|vout| vout != output.vout)
            || candidates.iter().any(|candidate: &&ChainOutputEvidence| {
                candidate.txid == output.txid && candidate.vout == output.vout
            })
        {
            continue;
        }

        let owner: Option<String> = sqlx::query_scalar(
            "SELECT operation_id FROM wallet_operations WHERE txid = ? AND tx_vout = ?",
        )
        .bind(&output.txid)
        .bind(i64::from(output.vout))
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_error)?;
        if owner.as_deref().is_none_or(|owner| owner == operation_id.0) {
            candidates.push(output);
        }
    }

    let [evidence] = candidates.as_slice() else {
        let result = if candidates.len() > 1 {
            ChainEvidenceClaim::Ambiguous {
                candidate_count: candidates.len(),
            }
        } else {
            ChainEvidenceClaim::NoMatch
        };
        tx.commit().await.map_err(internal_error)?;
        return Ok(result);
    };

    let status = if evidence.confirmations >= required_confirmations {
        SyncedWalletStatus::Completed
    } else if evidence.confirmations > 0 {
        SyncedWalletStatus::Confirmed
    } else {
        SyncedWalletStatus::Broadcast
    };
    // Chain evidence is the settlement writer for allocation funding sends —
    // `sync_operations` returns nothing for them — so the observation watermark
    // has to be stamped here too. Without it a funding send settled with no
    // recorded watermark, and admission had no way to tell whether the balance
    // it was reading predated the debit.
    let settles = matches!(status, SyncedWalletStatus::Completed);
    let update = sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, txid = ?, tx_vout = ?, confirmation_count = ?, \
             amount_sats = CASE WHEN amount_sats = 0 THEN ? ELSE amount_sats END, \
             completed_at = CASE WHEN ? THEN COALESCE(completed_at, unixepoch()) ELSE completed_at END, \
             settled_tick = CASE WHEN ? AND settled_tick IS NULL \
               THEN (SELECT tick FROM wallet_observation_ticks WHERE id = 1) \
               ELSE settled_tick END, \
             updated_at = unixepoch() \
         WHERE operation_id = ? AND status IN (?, ?, ?, ?) \
           AND (txid IS NULL OR txid = ?) AND (tx_vout IS NULL OR tx_vout = ?)",
    )
    .bind(status.to_string())
    .bind(&evidence.txid)
    .bind(i64::from(evidence.vout))
    .bind(i64::from(evidence.confirmations))
    .bind(i64::try_from(evidence.amount_sats).map_err(internal_error)?)
    .bind(matches!(status, SyncedWalletStatus::Completed))
    .bind(settles)
    .bind(&operation_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(WalletOperationStatus::Broadcast.to_string())
    .bind(WalletOperationStatus::Confirmed.to_string())
    .bind(&evidence.txid)
    .bind(i64::from(evidence.vout))
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    if update.rows_affected() != 1 {
        tx.commit().await.map_err(internal_error)?;
        return Ok(ChainEvidenceClaim::NoMatch);
    }
    let row = sqlx::query(wallet_operation_select_sql())
        .bind(&operation_id.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_error)?;
    let operation = wallet_operation_from_row(&row)?;
    tx.commit().await.map_err(internal_error)?;
    Ok(ChainEvidenceClaim::Applied(Box::new(operation)))
}

pub(crate) async fn get_wallet_operation(
    database: &Database,
    operation_id: &WalletOperationId,
) -> ServiceResult<WalletOperation> {
    let row = sqlx::query(wallet_operation_select_sql())
        .bind(&operation_id.0)
        .fetch_optional(database.pool())
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found("wallet operation not found"))?;
    wallet_operation_from_row(&row)
}

#[cfg(test)]
pub(crate) async fn wallet_operation_for_item(
    database: &Database,
    operation_type: WalletOperationType,
    item_id: &ItemId,
) -> ServiceResult<Option<WalletOperation>> {
    let row = sqlx::query(
        "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
         address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
         FROM wallet_operations WHERE operation_type = ? AND item_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(operation_type.to_string())
    .bind(&item_id.0)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    row.as_ref().map(wallet_operation_from_row).transpose()
}

pub(crate) async fn wallet_operation_for_item_tx(
    tx: &mut Transaction<'_, Sqlite>,
    operation_type: WalletOperationType,
    item_id: &ItemId,
) -> ServiceResult<Option<WalletOperation>> {
    let row = sqlx::query(
        "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
         address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
         FROM wallet_operations WHERE operation_type = ? AND item_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(operation_type.to_string())
    .bind(&item_id.0)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?;
    row.as_ref().map(wallet_operation_from_row).transpose()
}

pub(crate) async fn wallet_operations_for_federation(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<Vec<WalletOperation>> {
    let rows = sqlx::query(
        "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
         address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
         FROM wallet_operations WHERE federation_id = ? \
         ORDER BY created_at ASC, operation_id ASC",
    )
    .bind(&federation_id.0)
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;
    rows.iter().map(wallet_operation_from_row).collect()
}

pub(crate) async fn active_wallet_operations(
    database: &Database,
) -> ServiceResult<Vec<WalletOperation>> {
    let mut builder = QueryBuilder::new(
        "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
         address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
         FROM wallet_operations WHERE ",
    );
    push_in_list(&mut builder, "status", PENDING_SETTLEMENT_STATUSES);
    builder.push(" ORDER BY updated_at ASC, operation_id ASC");
    let rows = builder
        .build()
        .fetch_all(database.pool())
        .await
        .map_err(internal_error)?;
    rows.iter().map(wallet_operation_from_row).collect()
}

pub(crate) async fn list_wallet_operations(
    database: &Database,
    request: WalletOperationPageRequest,
) -> ServiceResult<ListResponse<WalletOperationSummary>> {
    let page = OffsetPage::from_page(&request.page, "wallet operation")?;

    let mut builder = QueryBuilder::new(
        "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
         address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
         FROM wallet_operations WHERE 1 = 1",
    );
    if let Some(status) = request.status_filter {
        builder.push(" AND status = ");
        builder.push_bind(status.to_string());
    }
    if let Some(time_range) = request.time_range {
        if let Some(from) = time_range.from {
            builder.push(" AND created_at >= ");
            builder.push_bind(timestamp_to_i64(from)?);
        }
        if let Some(to) = time_range.to {
            builder.push(" AND created_at < ");
            builder.push_bind(timestamp_to_i64(to)?);
        }
    }
    builder.push(" ORDER BY created_at DESC, operation_id DESC LIMIT ");
    builder.push_bind(page.fetch_limit());
    builder.push(" OFFSET ");
    builder.push_bind(page.offset());

    let rows = builder
        .build()
        .fetch_all(database.pool())
        .await
        .map_err(internal_error)?;
    page.list_response(rows, |row| wallet_operation_summary_from_row(&row))
}

pub(crate) async fn wallet_accounting_sums(
    database: &Database,
) -> ServiceResult<WalletAccountingSums> {
    let pending_incoming = sum_wallet_operations(
        database,
        &[WalletOperationType::Deposit],
        PENDING_SETTLEMENT_STATUSES,
    )
    .await?;
    let pending_outgoing = sum_wallet_operations(
        database,
        &[WalletOperationType::Withdrawal],
        PENDING_SETTLEMENT_STATUSES,
    )
    .await?;
    let mut builder =
        QueryBuilder::new("SELECT SUM(reserved_amount_sats) FROM allocation_items WHERE ");
    push_in_list(&mut builder, "status", &RESERVING_ITEM_STATUSES);
    let in_flight_allocations: Option<i64> = builder
        .build_query_scalar()
        .fetch_one(database.pool())
        .await
        .map_err(internal_error)?;
    Ok(WalletAccountingSums {
        pending_incoming: Sats(pending_incoming),
        pending_outgoing: Sats(pending_outgoing),
        in_flight_allocations: Sats(in_flight_allocations.unwrap_or_default().max(0) as u64),
    })
}

pub(crate) async fn active_reserved_amount_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> ServiceResult<Sats> {
    let mut builder =
        QueryBuilder::new("SELECT SUM(reserved_amount_sats) FROM allocation_items WHERE ");
    push_in_list(&mut builder, "status", &RESERVING_ITEM_STATUSES);
    let amount: Option<i64> = builder
        .build_query_scalar()
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(Sats(amount.unwrap_or_default().max(0) as u64))
}

/// Outgoing wallet operations whose debit is not yet known to be in the
/// observed balance.
///
/// A send stops being subtracted from available capacity only once its debit is
/// proven to be in that balance. Settling is not that proof: the sync pass reads
/// the balance before it applies settlements, so the balance persisted alongside
/// a settlement can predate the debit. A strictly later observation — one whose
/// backend read provably followed this settlement, the sync task running its
/// passes in series — is. A NULL watermark is an operation that settled before
/// this rule existed and is treated as already included.
///
/// Every outgoing type counts, not only operator withdrawals. Allocation
/// funding sends debit the same wallet, and the interval that matters is
/// precisely the one where nothing else covers them: between the item going
/// terminal, which drops its reservation, and an observation known to include
/// the send. A funding operation whose item is *still* reserving is skipped,
/// because [`active_reserved_amount_tx`] already subtracts that item's
/// reservation and counting both would refuse capacity the provider has.
///
/// The types are listed rather than the filter dropped. `Deposit` operations
/// live in the same table and move the balance the other way; summing them here
/// would charge the provider for money arriving.
pub(crate) async fn active_wallet_withdrawal_amount_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> ServiceResult<Sats> {
    stamp_released_item_operations_tx(tx).await?;

    let mut builder = QueryBuilder::new("SELECT SUM(amount_sats) FROM wallet_operations WHERE ");
    push_in_list(&mut builder, "operation_type", OUTGOING_OPERATION_TYPES);
    builder.push(" AND (");
    push_in_list(&mut builder, "status", PENDING_SETTLEMENT_STATUSES);
    // `released_tick` wins where it is set, because it records when
    // the charge could first be seen. Falling back to the settle stamp keeps
    // the never-excluded case exactly as it was.
    builder.push(
        " OR (settled_tick IS NOT NULL \
         AND COALESCE(released_tick, settled_tick) >= \
         (SELECT COALESCE(read_tick, 0) \
          FROM wallet_balance_observations WHERE id = 1))) \
         AND (item_id IS NULL OR item_id NOT IN (SELECT item_id FROM allocation_items WHERE ",
    );
    push_in_list(&mut builder, "status", &RESERVING_ITEM_STATUSES);
    builder.push("))");
    let amount: Option<i64> = builder
        .build_query_scalar()
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(Sats(amount.unwrap_or_default().max(0) as u64))
}

/// Stamps the observation count at which an item-linked withdrawal first became
/// visible to the charge above.
///
/// A settled operation whose item still reserves is excluded from that query,
/// so its settle stamp expires unobserved and the debit leaves the reserved
/// term and the unsettled term together. This records the count current when
/// the exclusion lifts, so the charge runs from a moment the row was actually
/// being charged.
///
/// It runs here rather than at each site that moves an item out of a reserving
/// status. Four production sites do that — completion, failure, cancellation,
/// and abandonment — and an enumeration of writers is the thing this branch's
/// re-derivations have got wrong most often. Evaluating it where the exclusion
/// is evaluated cannot fall out of step with it.
///
/// The plain count. No `+ 1` compensation is needed: observations record when
/// their read began, so the comparison is against a read point directly and an
/// observation that had already read the backend cannot release a row stamped
/// after that read.
async fn stamp_released_item_operations_tx(tx: &mut Transaction<'_, Sqlite>) -> ServiceResult<()> {
    let mut builder = QueryBuilder::new(
        "UPDATE wallet_operations SET released_tick = \
         (SELECT tick FROM wallet_observation_ticks WHERE id = 1) \
         WHERE released_tick IS NULL AND settled_tick IS NOT NULL \
         AND item_id IS NOT NULL AND ",
    );
    push_in_list(&mut builder, "operation_type", OUTGOING_OPERATION_TYPES);
    builder.push(" AND item_id NOT IN (SELECT item_id FROM allocation_items WHERE ");
    push_in_list(&mut builder, "status", &RESERVING_ITEM_STATUSES);
    builder.push(")");
    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(())
}

/// When a backend balance read began, in observation-count terms.
///
/// Capture it with [`begin_balance_read`] *before* asking the backend for a
/// balance, and hand it to [`upsert_wallet_balance_observation`] when the reply
/// arrives. The pair is what lets a settled withdrawal be released against a
/// balance that provably saw it, rather than against one that merely landed
/// later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BalanceReadPoint(i64);

/// Takes the read point to hand to [`upsert_wallet_balance_observation`].
///
/// Call this *before* the backend read. Calling it afterwards would record the
/// moment the reply arrived, which is the write order this exists to stop
/// standing in for read order.
pub(crate) async fn begin_balance_read(database: &Database) -> ServiceResult<BalanceReadPoint> {
    let tick: i64 = sqlx::query_scalar(
        "UPDATE wallet_observation_ticks SET tick = tick + 1 WHERE id = 1 RETURNING tick",
    )
    .fetch_one(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(BalanceReadPoint(tick))
}

pub(crate) async fn upsert_wallet_balance_observation(
    database: &Database,
    balance: &WalletBackendBalance,
    read_at: BalanceReadPoint,
) -> ServiceResult<()> {
    // `read_tick` records the tick this observation's backend read began at.
    // `active_wallet_withdrawal_amount_tx` compares against it: a settled
    // withdrawal stops being subtracted only once a persisted balance exists
    // whose read began after it settled.
    //
    // The `WHERE` is a monotonic guard. Three callers persist observations and
    // they are not serialised, so a slow reply can arrive after a fresher one.
    // Without the guard it would overwrite a newer balance with an older — and
    // therefore higher — one, which is the overcommit direction. A stale reply
    // is discarded instead, and the count does not advance for it.
    sqlx::query(
        "INSERT INTO wallet_balance_observations \
         (id, network, spendable_sats, source_json, observed_at, read_tick) \
         VALUES (1, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           network = excluded.network, \
           spendable_sats = excluded.spendable_sats, \
           source_json = excluded.source_json, \
           observed_at = excluded.observed_at, \
           read_tick = excluded.read_tick \
         WHERE excluded.read_tick >= wallet_balance_observations.read_tick",
    )
    .bind(balance.network.to_string())
    .bind(to_i64_sats(balance.spendable)?)
    .bind(serde_json::to_string(balance).map_err(internal_error)?)
    .bind(timestamp_to_i64(balance.observed_at)?)
    .bind(read_at.0)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

/// Records an observation whose read and write are adjacent.
///
/// Test-only. It models the serial case — read the backend, persist it, nothing
/// in between — which is what most tests want. A test that needs the concurrent
/// case must call [`begin_balance_read`] itself, before whatever it wants to
/// interleave, or it will not be testing read order at all.
#[cfg(test)]
pub(crate) async fn observe_balance_serially(
    database: &Database,
    balance: &WalletBackendBalance,
) -> ServiceResult<()> {
    let read_at = begin_balance_read(database).await?;
    upsert_wallet_balance_observation(database, balance, read_at).await
}

pub(crate) async fn latest_wallet_balance_observation(
    database: &Database,
) -> ServiceResult<Option<WalletBalanceObservation>> {
    let row = sqlx::query(
        "SELECT network, spendable_sats, observed_at FROM wallet_balance_observations WHERE id = 1",
    )
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(row.map(|row| WalletBalanceObservation {
        network: row.get("network"),
        spendable: Sats(row.get::<i64, _>("spendable_sats").max(0) as u64),
        observed_at: Timestamp(row.get::<i64, _>("observed_at").max(0) as u64),
    }))
}

pub(crate) async fn latest_wallet_balance_observation_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> ServiceResult<Option<WalletBalanceObservation>> {
    let row = sqlx::query(
        "SELECT network, spendable_sats, observed_at FROM wallet_balance_observations WHERE id = 1",
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(row.map(|row| WalletBalanceObservation {
        network: row.get("network"),
        spendable: Sats(row.get::<i64, _>("spendable_sats").max(0) as u64),
        observed_at: Timestamp(row.get::<i64, _>("observed_at").max(0) as u64),
    }))
}

async fn sum_wallet_operations(
    database: &Database,
    operation_types: &[WalletOperationType],
    statuses: &[WalletOperationStatus],
) -> ServiceResult<u64> {
    let mut builder = QueryBuilder::new("SELECT SUM(amount_sats) FROM wallet_operations WHERE ");
    push_in_list(&mut builder, "operation_type", operation_types);
    builder.push(" AND ");
    push_in_list(&mut builder, "status", statuses);
    let amount: Option<i64> = builder
        .build_query_scalar()
        .fetch_one(database.pool())
        .await
        .map_err(internal_error)?;
    Ok(amount.unwrap_or_default().max(0) as u64)
}

fn wallet_operation_select_sql() -> &'static str {
    "SELECT operation_id, operation_type, status, amount_sats, federation_id, item_id, \
     address, txid, tx_vout, confirmation_count, failure_json, created_at, updated_at \
     FROM wallet_operations WHERE operation_id = ?"
}

fn wallet_operation_from_row(row: &SqliteRow) -> ServiceResult<WalletOperation> {
    Ok(WalletOperation {
        operation_id: WalletOperationId(row.get("operation_id")),
        operation_type: parse_operation_type(row.get::<String, _>("operation_type").as_str())?,
        amount: Sats(row.get::<i64, _>("amount_sats").max(0) as u64),
        address: row.get("address"),
        txid: row.get("txid"),
        tx_vout: row
            .get::<Option<i64>, _>("tx_vout")
            .map(|value| value.max(0) as u32),
        status: parse_operation_status(row.get::<String, _>("status").as_str())?,
        confirmation_count: row
            .get::<Option<i64>, _>("confirmation_count")
            .map(|value| value.max(0) as u32),
        federation_id: row
            .get::<Option<String>, _>("federation_id")
            .map(FederationId),
        item_id: row.get::<Option<String>, _>("item_id").map(ItemId),
        created_at: Timestamp(row.get::<i64, _>("created_at").max(0) as u64),
        updated_at: Timestamp(row.get::<i64, _>("updated_at").max(0) as u64),
        failure: row
            .get::<Option<String>, _>("failure_json")
            .map(|json| serde_json::from_str(&json).map_err(internal_error))
            .transpose()?,
    })
}

fn wallet_operation_summary_from_row(row: &SqliteRow) -> ServiceResult<WalletOperationSummary> {
    Ok(WalletOperationSummary {
        operation_id: WalletOperationId(row.get("operation_id")),
        operation_type: parse_operation_type(row.get::<String, _>("operation_type").as_str())?,
        amount: Sats(row.get::<i64, _>("amount_sats").max(0) as u64),
        status: parse_operation_status(row.get::<String, _>("status").as_str())?,
        federation_id: row
            .get::<Option<String>, _>("federation_id")
            .map(FederationId),
        created_at: Timestamp(row.get::<i64, _>("created_at").max(0) as u64),
        updated_at: Timestamp(row.get::<i64, _>("updated_at").max(0) as u64),
    })
}

fn parse_operation_type(value: &str) -> ServiceResult<WalletOperationType> {
    value
        .parse()
        .map_err(|_| internal_error(format!("unknown wallet operation type {value:?}")))
}

fn parse_operation_status(value: &str) -> ServiceResult<WalletOperationStatus> {
    value
        .parse()
        .map_err(|_| internal_error(format!("unknown wallet operation status {value:?}")))
}

fn u64_to_i64(value: u64) -> ServiceResult<i64> {
    i64::try_from(value).map_err(|_| invalid_argument("value exceeds SQLite i64 range"))
}

fn i64_to_u64(value: i64) -> ServiceResult<u64> {
    u64::try_from(value).map_err(|_| internal_error("negative wallet operation value"))
}

fn timestamp_to_i64(timestamp: Timestamp) -> ServiceResult<i64> {
    i64::try_from(timestamp.0).map_err(|_| invalid_argument("timestamp exceeds SQLite i64 range"))
}
