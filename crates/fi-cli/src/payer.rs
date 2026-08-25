//! Crash-safe aggregate reservation and recovery for the development FI wallet.
//!
//! Everything here acts on behalf of the Federation Initiator paying a
//! quoted issuance set: fund the FMan's quoted outputs and collect the
//! aggregate blinded signatures ([`Wallet::pay_reserved_locked_v1`] /
//! [`Wallet::pay_reserved_locked_v2`]), prepare the FI-owned refund outputs a paid
//! presentation must carry ([`Wallet::prepare_refund_v1`] /
//! [`Wallet::prepare_refund_v2`]), and — after a refusal — submit the
//! FMan-signed refund transaction and reissue its outputs into ordinary
//! balance ([`Wallet::submit_refund_v1`] / [`Wallet::submit_refund_v2`]).
//!
//! The FMan daemon never calls these; its payee side remains in
//! `fman-fedimint`. This module is deliberately local to `fi-cli`, the
//! development consumer of `fi-client`.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use anyhow::Context as _;
use bitcoin_hashes::{Hash as _, sha256};
use fedimint_api_client::api::{FederationApiExt as _, ServerError};
use fedimint_api_client::query::FilterMapThreshold;
use fedimint_client::ClientHandleArc;
use fedimint_client_module::TransactionUpdates;
use fedimint_client_module::module::ClientModule as _;
use fedimint_client_module::transaction::FeeQuoteRequest;
use fedimint_core::config::FederationId;
use fedimint_core::core::{DynOutput, ModuleInstanceId, OperationId};
use fedimint_core::db::Database;
use fedimint_core::db::IDatabaseTransactionOpsCore as _;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::ApiRequestErased;
use fedimint_core::module::{AmountUnit, Amounts};
use fedimint_core::{
    Amount, IdxRange, NumPeersExt as _, OutPoint, OutPointRange, TieredMulti, TransactionId,
};
use fedimint_mint_client::output::verify_blind_share;
use fedimint_mint_client::{MintClientModule, OOBNotes};
use fedimint_mint_common::MintOutput;
use fedimint_mint_common::endpoint_constants::AWAIT_OUTPUT_OUTCOME_ENDPOINT;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};

use crate::wallet::{Wallet, WalletError};
use fedi_decentralized_service_fleet_manager::QuoteId;
use fi_client::PaymentReservationId;
use locked_payments::refund::{PreparedRefund, PreparedRefundV2};
use locked_payments::{locked_payment, locked_payment_v2, mint_v2_module};

const LOCKED_PAYMENT_V1_OPERATION_TYPE: &str = "fman-locked-payment";
const LOCKED_PAYMENT_V2_OPERATION_TYPE: &str = "fman-locked-payment-v2";
const LOCKED_PAYMENT_OPERATION_ID_DOMAIN: &[u8] = b"fman/locked-payment-operation";
const PAYMENT_RESERVATION_KEY_PREFIX: u8 = 2;
const PAYMENT_RESERVATION_VERSION: u16 = 4;
const PAYMENT_RESERVATION_TX_MAX_ATTEMPTS: usize = 10;
const REJECTED_INPUT_REFUND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Proof that exact aggregate reservation failed its balance comparison
/// before the wallet created or observed a same-id journal.
///
/// The FI adapter downcasts this marker through `anyhow` context to distinguish
/// the one error that may return selected formation to payer selection. Every
/// other planning, binding, storage, or commit error remains ambiguous and
/// therefore keeps the formation recoverable.
#[derive(Debug, thiserror::Error)]
#[error("payment wallet cannot preserve exact aggregate holds and required reserve")]
pub(crate) struct InsufficientLockedPaymentFundsWithoutReservation;

fn reservation_db(database: &Database) -> Database {
    database.clone()
}

/// Wallet-owned durable capability for one exact aggregate of locked payments.
///
/// The opaque key is the hash of fi-client's deterministic reservation id.
/// Debug output deliberately omits it.
#[derive(Clone)]
pub struct LockedPaymentReservation {
    key: Vec<u8>,
    reservation_id: PaymentReservationId,
}

impl std::fmt::Debug for LockedPaymentReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LockedPaymentReservation")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Decodable, Encodable, Eq, PartialEq)]
struct StoredLockedPaymentReservation {
    version: u16,
    federation_id: FederationId,
    required_reserve_msats: u64,
    members: Vec<StoredLockedPaymentMember>,
}

#[derive(Clone, Copy, Debug, Decodable, Encodable, Eq, PartialEq)]
struct StoredLockedPaymentMember {
    quote_id: [u8; 32],
    plan_hash: [u8; 32],
    debit_msats: u64,
    state: StoredLockedPaymentMemberState,
}

/// Canonical semantic identity of an aggregate before wallet debits are
/// planned. This deliberately cannot be serialized as a reservation journal:
/// a durable [`StoredLockedPaymentReservation`] must contain positive debit
/// allocations for every member.
#[derive(Clone, Debug, Eq, PartialEq)]
struct LockedPaymentReservationBinding {
    federation_id: FederationId,
    required_reserve: Amount,
    members: Vec<LockedPaymentMemberBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LockedPaymentMemberBinding {
    quote_id: QuoteId,
    plan_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug, Decodable, Encodable, Eq, PartialEq)]
enum StoredLockedPaymentMemberState {
    Held,
    Started,
    Terminal,
    Released,
}

/// Wallet-issued proof that one exact reservation member is terminally safe
/// to release.
///
/// Only successful funding-rejection recovery or signed-refund settlement can
/// construct this value. Its opaque fields bind the durable aggregate journal
/// and quote; caller-supplied ids alone are never release authority.
pub struct LockedPaymentTerminalRelease {
    key: Vec<u8>,
    quote_id: QuoteId,
    plan_hash: [u8; 32],
    debit_msats: u64,
}

impl std::fmt::Debug for LockedPaymentTerminalRelease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LockedPaymentTerminalRelease")
            .finish_non_exhaustive()
    }
}

/// Result of reissuing one signed refund into the payer's ordinary balance.
///
/// The restored amount is informational. The opaque release proof is the
/// authority that lets `fi-client` release exactly this aggregate member.
pub struct SettledLockedPaymentRefund {
    amount: Amount,
    release_proof: LockedPaymentTerminalRelease,
}

impl SettledLockedPaymentRefund {
    /// Split the settlement into its restored value and release authority.
    #[must_use]
    pub fn into_parts(self) -> (Amount, LockedPaymentTerminalRelease) {
        (self.amount, self.release_proof)
    }
}

impl LockedPaymentReservation {
    /// Borrow the deterministic FI reservation id without exposing wallet
    /// journal keys.
    #[must_use]
    pub fn reservation_id(&self) -> &PaymentReservationId {
        &self.reservation_id
    }
}

fn payment_reservation_key(reservation_id: &PaymentReservationId) -> Vec<u8> {
    let digest = sha256::Hash::hash(reservation_id.as_str().as_bytes()).to_byte_array();
    let mut key = vec![PAYMENT_RESERVATION_KEY_PREFIX];
    key.extend_from_slice(&digest);
    key
}

fn validate_payment_reservation(
    existing: &StoredLockedPaymentReservation,
    expected: &LockedPaymentReservationBinding,
) -> anyhow::Result<()> {
    validate_payment_reservation_shape(existing)?;
    anyhow::ensure!(
        existing.federation_id == expected.federation_id,
        "payment reservation belongs to another federation"
    );
    anyhow::ensure!(
        existing.required_reserve_msats == expected.required_reserve.msats,
        "payment reservation required balance floor changed"
    );
    anyhow::ensure!(
        existing.members.len() == expected.members.len(),
        "payment reservation member count changed"
    );
    anyhow::ensure!(
        existing
            .members
            .iter()
            .zip(&expected.members)
            .all(|(left, right)| left.quote_id == right.quote_id.0),
        "payment reservation quote order changed"
    );
    anyhow::ensure!(
        existing
            .members
            .iter()
            .zip(&expected.members)
            .all(|(left, right)| left.plan_hash == right.plan_hash),
        "payment reservation output plan changed"
    );
    Ok(())
}

fn expected_payment_reservation(
    federation_id: FederationId,
    requirements: &[LockedPaymentPreflight],
    required_reserve: Amount,
) -> anyhow::Result<LockedPaymentReservationBinding> {
    anyhow::ensure!(!requirements.is_empty(), "payment reservation is empty");
    let mut unique_quotes = std::collections::BTreeSet::new();
    anyhow::ensure!(
        requirements
            .iter()
            .all(|requirement| unique_quotes.insert(requirement.quote_id().0)),
        "payment reservation contains duplicate quote ids"
    );
    Ok(LockedPaymentReservationBinding {
        federation_id,
        required_reserve,
        members: requirements
            .iter()
            .map(|requirement| LockedPaymentMemberBinding {
                quote_id: requirement.quote_id(),
                plan_hash: requirement.plan_hash(),
            })
            .collect(),
    })
}

fn validate_payment_reservation_shape(
    stored: &StoredLockedPaymentReservation,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored.version == PAYMENT_RESERVATION_VERSION && !stored.members.is_empty(),
        "payment reservation journal is invalid"
    );
    let mut quote_ids = std::collections::BTreeSet::new();
    anyhow::ensure!(
        stored
            .members
            .iter()
            .all(|member| member.debit_msats > 0 && quote_ids.insert(member.quote_id)),
        "payment reservation members must have unique quote ids and nonzero debit allocations"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum LockedPaymentGeneration {
    MintV1,
    MintV2,
}

/// Non-secret recovery data stored atomically with a locked-payment
/// transaction. Fedimint rejects a second submission with the same operation
/// ID, so an interrupted caller uses this range to await and collect the
/// original outputs instead of spending again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPaymentOperationMeta {
    generation: LockedPaymentGeneration,
    mint_module: ModuleInstanceId,
    binding_hash: [u8; 32],
    output_range: OutPointRange,
    /// Primary-module change appended after the quoted foreign outputs.
    change_range: OutPointRange,
}

impl LockedPaymentOperationMeta {
    fn new(
        generation: LockedPaymentGeneration,
        mint_module: ModuleInstanceId,
        binding_hash: [u8; 32],
        output_range: OutPointRange,
        change_range: OutPointRange,
    ) -> Self {
        Self {
            generation,
            mint_module,
            binding_hash,
            output_range,
            change_range,
        }
    }

    fn validate(
        self,
        generation: LockedPaymentGeneration,
        mint_module: ModuleInstanceId,
        binding_hash: [u8; 32],
        output_count: usize,
    ) -> anyhow::Result<LockedPaymentRanges> {
        anyhow::ensure!(
            self.generation == generation,
            "locked-payment operation belongs to a different mint generation"
        );
        anyhow::ensure!(
            self.mint_module == mint_module,
            "locked-payment operation belongs to a different mint module"
        );
        anyhow::ensure!(
            self.binding_hash == binding_hash,
            "locked-payment operation belongs to a different quote"
        );
        anyhow::ensure!(
            self.output_range.start_idx() == 0 && self.output_range.count() == output_count,
            "locked-payment operation has a different quoted output range"
        );
        anyhow::ensure!(
            self.change_range.txid() == self.output_range.txid()
                && self.change_range.start_idx()
                    == u64::try_from(output_count).context("too many locked-payment outputs")?,
            "locked-payment operation has an invalid primary change range"
        );
        Ok(LockedPaymentRanges {
            outputs: self.output_range,
            change: self.change_range,
        })
    }

    /// Validate a current operation-log row without needing the quote payload
    /// that was already consumed by formation. The reopened accounting path
    /// uses this to exercise the same exact ranges and change-finality wait as
    /// ordinary accepted-payment recovery.
    fn recovery_ranges_for_accounting(
        self,
        primary_module: ModuleInstanceId,
    ) -> anyhow::Result<LockedPaymentRanges> {
        let generation = self.generation;
        let mint_module = self.mint_module;
        let binding_hash = self.binding_hash;
        let output_count = self.output_range.count();
        anyhow::ensure!(
            mint_module == primary_module,
            "setup-payment operation belongs to a non-primary mint module"
        );
        self.validate(generation, mint_module, binding_hash, output_count)
    }
}

/// Durable-wallet outcome when recovering one exact quote-bound payment.
///
/// `Rejected` is terminal: the federation's transaction state machine proved
/// that the funding transaction did not enter consensus, every exact payer
/// input was restored by accepted automatic refund transactions, and all
/// resulting payer outputs are spendable. The caller may safely abandon that
/// quote and obtain a new one.
#[must_use]
pub enum LockedPaymentRecovery<T> {
    /// No wallet operation exists for this exact quote and issuance.
    Absent,
    /// The original transaction was accepted, its payer change is spendable,
    /// and its payment evidence is reconstructed in `T`.
    Funded(T),
    /// The original transaction was rejected and its inputs are spendable.
    Rejected(LockedPaymentTerminalRelease),
}

/// Independently observed value movement from the reopened Fedimint operation
/// log. The paid `defe` reproduction uses this diagnostic to reconcile its
/// one-note input without defining fees as an unexplained balance residual.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaymentWalletAccounting {
    pub(crate) received_input_msats: u64,
    pub(crate) receive_fee_msats: u64,
    pub(crate) setup_output_msats: u64,
    pub(crate) setup_fee_msats: u64,
    pub(crate) setup_transaction_count: u64,
}

/// Exact value requirement for one quote-bound locked payment.
///
/// The quote id is used only as deterministic, per-payment operation context
/// while the wallet asks its primary module to select inputs in a
/// non-committable transaction. No operation or payment state is persisted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockedPaymentPreflight {
    quote_id: QuoteId,
    outputs: LockedPaymentPreflightOutputs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LockedPaymentPreflightOutputs {
    MintV1(Vec<locked_payment::IssuanceRequest>),
    MintV2 {
        module: ModuleInstanceId,
        issuance: Vec<locked_payment_v2::IssuanceRequest>,
    },
}

impl LockedPaymentPreflight {
    /// Bind one mint-v1 foreign output bundle to its semantic quote id.
    #[must_use]
    pub fn mint_v1(quote_id: QuoteId, issuance: Vec<locked_payment::IssuanceRequest>) -> Self {
        Self {
            quote_id,
            outputs: LockedPaymentPreflightOutputs::MintV1(issuance),
        }
    }

    /// Bind one mint-v2 foreign output bundle to its semantic quote id.
    #[must_use]
    pub fn mint_v2(
        quote_id: QuoteId,
        module: ModuleInstanceId,
        issuance: Vec<locked_payment_v2::IssuanceRequest>,
    ) -> Self {
        Self {
            quote_id,
            outputs: LockedPaymentPreflightOutputs::MintV2 { module, issuance },
        }
    }

    /// Exact protocol quote whose future payment this check represents.
    #[must_use]
    pub fn quote_id(&self) -> QuoteId {
        self.quote_id
    }

    fn plan_hash(&self) -> [u8; 32] {
        let mut bytes = b"fman/locked-payment-plan/v1\0".to_vec();
        bytes.extend_from_slice(&self.quote_id.0);
        match &self.outputs {
            LockedPaymentPreflightOutputs::MintV1(issuance) => {
                bytes.push(1);
                bytes.extend_from_slice(&(issuance.len() as u64).to_be_bytes());
                for request in issuance {
                    bytes.extend_from_slice(&request.amount.msats.to_be_bytes());
                    append_plan_field(&mut bytes, &request.blind_nonce.consensus_encode_to_vec());
                }
            }
            LockedPaymentPreflightOutputs::MintV2 { module, issuance } => {
                bytes.push(2);
                bytes.extend_from_slice(&module.consensus_encode_to_vec());
                bytes.extend_from_slice(&(issuance.len() as u64).to_be_bytes());
                for request in issuance {
                    append_plan_field(&mut bytes, &request.denomination.consensus_encode_to_vec());
                    append_plan_field(&mut bytes, &request.blind_nonce.consensus_encode_to_vec());
                    bytes.extend_from_slice(&request.tweak);
                }
            }
        }
        sha256::Hash::hash(&bytes).to_byte_array()
    }
}

fn append_plan_field(target: &mut Vec<u8>, field: &[u8]) {
    target.extend_from_slice(&(field.len() as u64).to_be_bytes());
    target.extend_from_slice(field);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlannedLockedPayment {
    /// Logical net cost of an independent dry run for the exact transaction.
    /// Physical notes are not reserved and the cost is re-quoted at submission.
    debit_msats: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LockedPaymentHoldSummary {
    pub(crate) held_msats: u64,
    pub(crate) required_reserve_msats: u64,
}

enum LockedPaymentTransactionStatus {
    Accepted,
    Rejected(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LockedPaymentRanges {
    outputs: OutPointRange,
    change: OutPointRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LockedPaymentOperation {
    Existing {
        operation_id: OperationId,
        ranges: LockedPaymentRanges,
    },
    New {
        operation_id: OperationId,
    },
}

async fn finalize_reserved_transaction(
    client: &ClientHandleArc,
    operation_id: OperationId,
    operation_type: &str,
    generation: LockedPaymentGeneration,
    mint_module: ModuleInstanceId,
    binding_hash: [u8; 32],
    output_count: usize,
    builder: fedimint_client_module::transaction::TransactionBuilder,
) -> anyhow::Result<OutPointRange> {
    client
        .db()
        .autocommit(
            |dbtx, _| {
                let builder = builder.clone();
                Box::pin(async move {
                    let range = client
                        .finalize_and_submit_transaction_dbtx(
                            dbtx,
                            operation_id,
                            operation_type,
                            move |change| {
                                let range = quoted_output_range(change, output_count)
                                    .expect("output count was already representable");
                                LockedPaymentOperationMeta::new(
                                    generation,
                                    mint_module,
                                    binding_hash,
                                    range,
                                    change,
                                )
                            },
                            builder,
                        )
                        .await?;
                    Ok::<_, anyhow::Error>(range)
                })
            },
            Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
        )
        .await
        .map_err(|error| anyhow::anyhow!("fund locked payment: {error:?}"))
}

impl Wallet {
    async fn plan_locked_payments(
        &self,
        federation_id: FederationId,
        requirements: &[LockedPaymentPreflight],
    ) -> anyhow::Result<Vec<PlannedLockedPayment>> {
        let client = self.client(federation_id).await?;
        let mut planned = Vec::with_capacity(requirements.len());
        for requirement in requirements {
            let (output_amount, output_fee, operation_id) = match &requirement.outputs {
                LockedPaymentPreflightOutputs::MintV1(issuance) => {
                    let mint = client
                        .get_first_module::<MintClientModule>()
                        .context("mint module")?;
                    let (output_amount, output_fee) =
                        mint_v1_output_amount_and_fee(&mint, issuance)?;
                    let binding_hash =
                        sha256::Hash::hash(&requirement.quote_id().0).to_byte_array();
                    (
                        output_amount,
                        output_fee,
                        locked_payment_v1_operation_id(mint.id, issuance, binding_hash),
                    )
                }
                LockedPaymentPreflightOutputs::MintV2 { module, issuance } => {
                    let mint = mint_v2_module(&client, *module)?;
                    let (output_amount, output_fee) =
                        mint_v2_output_amount_and_fee(mint, issuance)?;
                    let binding_hash =
                        sha256::Hash::hash(&requirement.quote_id().0).to_byte_array();
                    (
                        output_amount,
                        output_fee,
                        locked_payment_v2_operation_id(*module, issuance, binding_hash),
                    )
                }
            };
            let debit =
                locked_payment_net_debit(&client, operation_id, output_amount, output_fee).await?;
            anyhow::ensure!(debit.msats > 0, "locked-payment debit is empty");
            planned.push(PlannedLockedPayment {
                debit_msats: debit.msats,
            });
        }
        Ok(planned)
    }

    pub(crate) async fn locked_payment_hold_summary(
        &self,
        federation_id: FederationId,
    ) -> anyhow::Result<LockedPaymentHoldSummary> {
        let database = reservation_db(&self.database);
        let mut dbtx = database.begin_transaction_nc().await;
        let mut rows = dbtx
            .raw_find_by_prefix(&[PAYMENT_RESERVATION_KEY_PREFIX])
            .await?;
        let mut summary = LockedPaymentHoldSummary::default();
        while let Some((_key, bytes)) = rows.next().await {
            let stored = StoredLockedPaymentReservation::consensus_decode_whole(
                &bytes,
                &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
            )?;
            validate_payment_reservation_shape(&stored)?;
            if stored.federation_id != federation_id {
                continue;
            }
            let held_msats = stored
                .members
                .iter()
                .filter(|member| member.state == StoredLockedPaymentMemberState::Held)
                .try_fold(0u64, |sum, member| sum.checked_add(member.debit_msats))
                .context("locked-payment hold total overflow")?;
            summary.held_msats = summary
                .held_msats
                .checked_add(held_msats)
                .context("wallet hold total overflow")?;
            if held_msats > 0 {
                summary.required_reserve_msats = summary
                    .required_reserve_msats
                    .max(stored.required_reserve_msats);
            }
        }
        Ok(summary)
    }

    /// Idempotently journal one exact aggregate after proving it fundable.
    ///
    /// The wallet root is process-exclusive, and every locked payment started
    /// through this wallet must present the returned capability. The record
    /// survives a crash and binds the ordered quote set; reusing an id for a
    /// different aggregate fails closed.
    pub async fn reserve_locked_payments(
        &self,
        federation_id: FederationId,
        reservation_id: &PaymentReservationId,
        requirements: &[LockedPaymentPreflight],
    ) -> anyhow::Result<LockedPaymentReservation> {
        self.reserve_locked_payments_with_reserve(
            federation_id,
            reservation_id,
            requirements,
            Amount::ZERO,
        )
        .await
    }

    /// Reserve the exact aggregate while preserving a caller-owned wallet
    /// floor. Existing same-id reservations are reconstructed without
    /// re-planning already-started members; a new reservation independently
    /// dry-runs every exact transaction under the wallet-wide spend guard and
    /// persists those logical net-debit allocations without selecting notes.
    pub async fn reserve_locked_payments_with_reserve(
        &self,
        federation_id: FederationId,
        reservation_id: &PaymentReservationId,
        requirements: &[LockedPaymentPreflight],
        required_reserve: Amount,
    ) -> anyhow::Result<LockedPaymentReservation> {
        let _spend_guard = self.spend_guard.lock().await;
        let key = payment_reservation_key(reservation_id);
        let expected = expected_payment_reservation(federation_id, requirements, required_reserve)?;
        let database = reservation_db(&self.database);
        // Reconstruct a same-id durable journal before asking the wallet to
        // fund the complete original aggregate. Some members may already have
        // consumed their value; charging them again would strand an exactly
        // funded recovery.
        if let Some(bytes) = database
            .begin_transaction_nc()
            .await
            .raw_get_bytes(&key)
            .await?
        {
            let existing = StoredLockedPaymentReservation::consensus_decode_whole(
                &bytes,
                &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
            )?;
            validate_payment_reservation(&existing, &expected)?;
            return Ok(LockedPaymentReservation {
                key,
                reservation_id: reservation_id.clone(),
            });
        }

        let planned = self
            .plan_locked_payments(federation_id, requirements)
            .await?;
        let client = self.client(federation_id).await?;
        let balance = client.get_balance_for_btc().await?;
        let holds = self.locked_payment_hold_summary(federation_id).await?;
        let new_debit_msats = planned
            .iter()
            .try_fold(0u64, |sum, member| sum.checked_add(member.debit_msats))
            .context("locked-payment aggregate debit overflow")?;
        let required_msats = holds
            .held_msats
            .checked_add(new_debit_msats)
            .and_then(|value| {
                value.checked_add(holds.required_reserve_msats.max(required_reserve.msats))
            })
            .context("locked-payment aggregate hold overflow")?;
        if balance.msats < required_msats {
            return Err(InsufficientLockedPaymentFundsWithoutReservation.into());
        }
        let stored = StoredLockedPaymentReservation {
            version: PAYMENT_RESERVATION_VERSION,
            federation_id: expected.federation_id,
            required_reserve_msats: expected.required_reserve.msats,
            members: expected
                .members
                .iter()
                .zip(&planned)
                .map(|(member, planned)| StoredLockedPaymentMember {
                    quote_id: member.quote_id.0,
                    plan_hash: member.plan_hash,
                    debit_msats: planned.debit_msats,
                    state: StoredLockedPaymentMemberState::Held,
                })
                .collect(),
        };
        let expected_for_tx = expected.clone();
        let key_for_tx = key.clone();
        database
            .autocommit(
                move |dbtx, _| {
                    let key = key_for_tx.clone();
                    let stored = stored.clone();
                    let expected = expected_for_tx.clone();
                    Box::pin(async move {
                        if let Some(bytes) = dbtx.raw_get_bytes(&key).await? {
                            let existing = StoredLockedPaymentReservation::consensus_decode_whole(
                                &bytes,
                                &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
                            )?;
                            validate_payment_reservation(&existing, &expected)?;
                        } else {
                            dbtx.raw_insert_bytes(&key, &stored.consensus_encode_to_vec())
                                .await?;
                        }
                        Ok::<_, anyhow::Error>(())
                    })
                },
                Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error| anyhow::anyhow!("persist payment reservation: {error:?}"))?;
        Ok(LockedPaymentReservation {
            key,
            reservation_id: reservation_id.clone(),
        })
    }

    /// Recover an exact aggregate journal without creating or funding one.
    ///
    /// `Ok(None)` is an authoritative local-database absence. Any existing
    /// same-id row must match the payer, the caller's `required_reserve`
    /// floor, ordered quote ids, and exact output-plan hashes or recovery
    /// fails closed.
    pub async fn recover_locked_payment_reservation(
        &self,
        federation_id: FederationId,
        reservation_id: &PaymentReservationId,
        requirements: &[LockedPaymentPreflight],
        required_reserve: Amount,
    ) -> anyhow::Result<Option<LockedPaymentReservation>> {
        let _spend_guard = self.spend_guard.lock().await;
        let key = payment_reservation_key(reservation_id);
        let expected = expected_payment_reservation(federation_id, requirements, required_reserve)?;
        let database = reservation_db(&self.database);
        let Some(bytes) = database
            .begin_transaction_nc()
            .await
            .raw_get_bytes(&key)
            .await?
        else {
            return Ok(None);
        };
        let existing = StoredLockedPaymentReservation::consensus_decode_whole(
            &bytes,
            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
        )?;
        validate_payment_reservation(&existing, &expected)?;
        Ok(Some(LockedPaymentReservation {
            key,
            reservation_id: reservation_id.clone(),
        }))
    }

    /// Prove this aggregate has not started and release its held value.
    ///
    /// A value-free tombstone remains so a crash before the FI's matching
    /// checkpoint can reconstruct and replay the release idempotently.
    pub async fn release_locked_payment_reservation(
        &self,
        reservation: LockedPaymentReservation,
    ) -> anyhow::Result<()> {
        let _spend_guard = self.spend_guard.lock().await;
        let database = reservation_db(&self.database);
        database
            .autocommit(
                move |dbtx, _| {
                    let key = reservation.key.clone();
                    Box::pin(async move {
                        let bytes = dbtx
                            .raw_get_bytes(&key)
                            .await?
                            .context("payment reservation is missing")?;
                        let mut stored = StoredLockedPaymentReservation::consensus_decode_whole(
                            &bytes,
                            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
                        )?;
                        validate_payment_reservation_shape(&stored)?;
                        let all_held = stored
                            .members
                            .iter()
                            .all(|member| member.state == StoredLockedPaymentMemberState::Held);
                        let all_released = stored
                            .members
                            .iter()
                            .all(|member| member.state == StoredLockedPaymentMemberState::Released);
                        anyhow::ensure!(
                            all_held || all_released,
                            "payment reservation has started outputs"
                        );
                        if all_held {
                            for member in &mut stored.members {
                                member.state = StoredLockedPaymentMemberState::Released;
                            }
                        }
                        // Retain a value-free tombstone. If the wallet commit
                        // succeeds but the FI crashes before clearing its own
                        // checkpoint, reconstruct-and-release must not create a
                        // fresh hold or depend on the wallet still being funded.
                        dbtx.raw_insert_bytes(&key, &stored.consensus_encode_to_vec())
                            .await?;
                        Ok::<_, anyhow::Error>(())
                    })
                },
                Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error| anyhow::anyhow!("release payment reservation: {error:?}"))?;
        Ok(())
    }

    /// Atomically bind one output-starting call to its aggregate capability.
    pub(crate) async fn start_reserved_locked_payment(
        &self,
        reservation: &LockedPaymentReservation,
        quote_id: QuoteId,
    ) -> anyhow::Result<()> {
        let database = reservation_db(&self.database);
        let key = reservation.key.clone();
        database
            .autocommit(
                move |dbtx, _| {
                    let key = key.clone();
                    Box::pin(async move {
                        let bytes = dbtx
                            .raw_get_bytes(&key)
                            .await?
                            .context("payment reservation is missing")?;
                        let mut stored = StoredLockedPaymentReservation::consensus_decode_whole(
                            &bytes,
                            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
                        )?;
                        validate_payment_reservation_shape(&stored)?;
                        let member = stored
                            .members
                            .iter_mut()
                            .find(|member| member.quote_id == quote_id.0)
                            .context("quote does not belong to payment reservation")?;
                        anyhow::ensure!(
                            member.state != StoredLockedPaymentMemberState::Released,
                            "quote was released from payment reservation"
                        );
                        if member.state == StoredLockedPaymentMemberState::Held {
                            member.state = StoredLockedPaymentMemberState::Started;
                        }
                        dbtx.raw_insert_bytes(&key, &stored.consensus_encode_to_vec())
                            .await?;
                        Ok::<_, anyhow::Error>(())
                    })
                },
                Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error| anyhow::anyhow!("start reserved payment: {error:?}"))?;
        Ok(())
    }

    /// Persist wallet-owned proof that one member reached terminal rejection
    /// or completed signed-refund settlement.
    async fn record_locked_payment_terminal(
        &self,
        reservation_id: &PaymentReservationId,
        quote_id: QuoteId,
    ) -> anyhow::Result<LockedPaymentTerminalRelease> {
        let key = payment_reservation_key(reservation_id);
        let database = reservation_db(&self.database);
        let key_for_tx = key.clone();
        let (plan_hash, debit_msats) = database
            .autocommit(
                move |dbtx, _| {
                    let key = key_for_tx.clone();
                    Box::pin(async move {
                        let bytes = dbtx
                            .raw_get_bytes(&key)
                            .await?
                            .context("payment reservation is missing")?;
                        let mut stored = StoredLockedPaymentReservation::consensus_decode_whole(
                            &bytes,
                            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
                        )?;
                        validate_payment_reservation_shape(&stored)?;
                        let member = stored
                            .members
                            .iter_mut()
                            .find(|member| member.quote_id == quote_id.0)
                            .context("quote does not belong to payment reservation")?;
                        anyhow::ensure!(
                            matches!(
                                member.state,
                                StoredLockedPaymentMemberState::Started
                                    | StoredLockedPaymentMemberState::Terminal
                                    | StoredLockedPaymentMemberState::Released
                            ),
                            "unstarted payment cannot have terminal wallet proof"
                        );
                        if member.state == StoredLockedPaymentMemberState::Started {
                            member.state = StoredLockedPaymentMemberState::Terminal;
                        }
                        let binding = (member.plan_hash, member.debit_msats);
                        dbtx.raw_insert_bytes(&key, &stored.consensus_encode_to_vec())
                            .await?;
                        Ok(binding)
                    })
                },
                Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error| anyhow::anyhow!("record terminal payment proof: {error:?}"))?;
        Ok(LockedPaymentTerminalRelease {
            key,
            quote_id,
            plan_hash,
            debit_msats,
        })
    }

    /// Reconstruct terminal release authority from one exact durable journal
    /// member before recovery opens a joined client or performs network work.
    async fn recover_terminal_locked_payment(
        &self,
        federation_id: FederationId,
        reservation_id: &PaymentReservationId,
        requirement: &LockedPaymentPreflight,
    ) -> anyhow::Result<Option<LockedPaymentTerminalRelease>> {
        let key = payment_reservation_key(reservation_id);
        let bytes = reservation_db(&self.database)
            .begin_transaction_nc()
            .await
            .raw_get_bytes(&key)
            .await?
            .context("payment reservation is missing")?;
        let stored = StoredLockedPaymentReservation::consensus_decode_whole(
            &bytes,
            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
        )?;
        validate_payment_reservation_shape(&stored)?;
        anyhow::ensure!(
            stored.federation_id == federation_id,
            "payment reservation belongs to another federation"
        );
        let member = stored
            .members
            .iter()
            .find(|member| member.quote_id == requirement.quote_id().0)
            .context("quote does not belong to payment reservation")?;
        anyhow::ensure!(
            member.plan_hash == requirement.plan_hash(),
            "reserved payment output plan changed"
        );

        Ok(matches!(
            member.state,
            StoredLockedPaymentMemberState::Terminal | StoredLockedPaymentMemberState::Released
        )
        .then(|| LockedPaymentTerminalRelease {
            key,
            quote_id: requirement.quote_id(),
            plan_hash: member.plan_hash,
            debit_msats: member.debit_msats,
        }))
    }

    /// Release one terminally rejected/refunded member without affecting
    /// aggregate siblings. Repeating a completed release is harmless.
    pub async fn release_locked_payment_member(
        &self,
        proof: LockedPaymentTerminalRelease,
    ) -> anyhow::Result<()> {
        let _spend_guard = self.spend_guard.lock().await;
        let database = reservation_db(&self.database);
        database
            .autocommit(
                move |dbtx, _| {
                    let key = proof.key.clone();
                    Box::pin(async move {
                        let bytes = dbtx
                            .raw_get_bytes(&key)
                            .await?
                            .context("payment reservation is missing")?;
                        let mut stored = StoredLockedPaymentReservation::consensus_decode_whole(
                            &bytes,
                            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
                        )?;
                        validate_payment_reservation_shape(&stored)?;
                        let member = stored
                            .members
                            .iter_mut()
                            .find(|member| member.quote_id == proof.quote_id.0)
                            .context("terminal proof does not belong to payment reservation")?;
                        anyhow::ensure!(
                            member.plan_hash == proof.plan_hash
                                && member.debit_msats == proof.debit_msats,
                            "terminal proof payment binding changed"
                        );
                        anyhow::ensure!(
                            matches!(
                                member.state,
                                StoredLockedPaymentMemberState::Terminal
                                    | StoredLockedPaymentMemberState::Released
                            ),
                            "payment is prepared or ambiguous, not terminally releasable"
                        );
                        member.state = StoredLockedPaymentMemberState::Released;
                        // Started is not terminal: another member may still
                        // be Prepared or ambiguous and needs this aggregate
                        // journal for exact crash recovery. Journal rows remain
                        // as value-free tombstones after every release.
                        dbtx.raw_insert_bytes(&key, &stored.consensus_encode_to_vec())
                            .await?;
                        Ok(())
                    })
                },
                Some(PAYMENT_RESERVATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error| anyhow::anyhow!("release terminal payment member: {error:?}"))?;
        Ok(())
    }

    async fn validate_reserved_payment_start(
        &self,
        reservation: &LockedPaymentReservation,
        federation_id: FederationId,
        requirement: &LockedPaymentPreflight,
    ) -> anyhow::Result<u64> {
        let database = reservation_db(&self.database);
        let bytes = database
            .begin_transaction_nc()
            .await
            .raw_get_bytes(&reservation.key)
            .await?
            .context("payment reservation is missing")?;
        let stored = StoredLockedPaymentReservation::consensus_decode_whole(
            &bytes,
            &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
        )?;
        validate_payment_reservation_shape(&stored)?;
        anyhow::ensure!(
            stored.federation_id == federation_id,
            "payment reservation belongs to another federation"
        );
        let member = stored
            .members
            .iter()
            .find(|member| member.quote_id == requirement.quote_id().0)
            .context("quote does not belong to payment reservation")?;
        anyhow::ensure!(
            member.state == StoredLockedPaymentMemberState::Held,
            "reserved payment already started or was released; recover it instead"
        );
        anyhow::ensure!(
            member.plan_hash == requirement.plan_hash(),
            "reserved payment output plan changed"
        );
        Ok(member.debit_msats)
    }

    async fn maximum_reserved_payment_net_debit(
        &self,
        federation_id: FederationId,
        member_allocation_msats: u64,
    ) -> anyhow::Result<u64> {
        let client = self.client(federation_id).await?;
        let balance = client.get_balance_for_btc().await?;
        let holds = self.locked_payment_hold_summary(federation_id).await?;
        let other_held_msats = holds
            .held_msats
            .checked_sub(member_allocation_msats)
            .context("reserved payment is missing from the wallet hold total")?;
        let protected_msats = other_held_msats
            .checked_add(holds.required_reserve_msats)
            .context("wallet hold total overflow")?;
        anyhow::ensure!(
            balance.msats >= protected_msats,
            "payment wallet can no longer preserve exact holds and reserve"
        );
        Ok(balance.msats - protected_msats)
    }

    /// Pay mint-v1 outputs whose blinded nonces belong to another party.
    /// The wallet auto-funds the output-only builder, waits for consensus,
    /// then obtains and verifies a threshold of shares for every quoted
    /// outpoint before returning aggregate signatures. `quote_id` is the
    /// stable canonical quote identifier used for exact retry identity.
    pub async fn pay_reserved_locked_v1(
        &self,
        reservation: &LockedPaymentReservation,
        federation_id: FederationId,
        issuance: &[locked_payment::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<Vec<tbs::BlindedSignature>, WalletError> {
        self.submit_locked_v1(reservation, federation_id, issuance, quote_id)
            .await
    }

    /// A rejected funding transaction is replaceable only after Fedimint's
    /// automatic mint-input refund transactions have restored every exact
    /// input as spendable wallet outputs. Until then the durable member stays
    /// `Started`, so restart recovery cannot short-circuit into replacement.
    async fn recover_rejected_locked_payment<T>(
        &self,
        client: &ClientHandleArc,
        operation_id: OperationId,
        rejected_txid: TransactionId,
        mint_module: ModuleInstanceId,
        reservation_id: &PaymentReservationId,
        quote_id: QuoteId,
    ) -> Result<LockedPaymentRecovery<T>, WalletError> {
        let original =
            locked_payment_created_transaction_by_txid(client, operation_id, rejected_txid).await?;
        let updates = client.transaction_updates(operation_id).await;
        tokio::time::timeout(
            REJECTED_INPUT_REFUND_TIMEOUT,
            await_rejected_input_refunds(
                operation_id,
                original,
                mint_module,
                updates,
                |outpoints| async move {
                    client
                        .await_primary_bitcoin_module_outputs(operation_id, outpoints)
                        .await
                },
            ),
        )
        .await
        .context("timed out waiting for rejected payment inputs to become spendable")??;
        self.record_locked_payment_terminal(reservation_id, quote_id)
            .await
            .map(LockedPaymentRecovery::Rejected)
            .map_err(Into::into)
    }

    /// Reopen and audit exact accepted setup-payment and mint-v2 receive
    /// transactions. This is diagnostic accounting only; it neither decides
    /// formation policy nor creates, replaces, or releases a payment.
    pub(crate) async fn payment_accounting(
        &self,
        federation_id: FederationId,
    ) -> Result<PaymentWalletAccounting, WalletError> {
        const OPERATION_PAGE_SIZE: usize = 1_000;

        let client = self.client(federation_id).await?;
        let primary_module = client
            .primary_module_for_unit(AmountUnit::BITCOIN)
            .context("payment federation has no primary Bitcoin module")?
            .0;
        let mut accounting = PaymentWalletAccounting {
            received_input_msats: 0,
            receive_fee_msats: 0,
            setup_output_msats: 0,
            setup_fee_msats: 0,
            setup_transaction_count: 0,
        };
        let mut cursor = None;
        loop {
            let page = client
                .operation_log()
                .paginate_operations_rev(OPERATION_PAGE_SIZE, cursor)
                .await;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            let complete = page.len() < OPERATION_PAGE_SIZE;
            for (key, operation) in page {
                match operation.operation_module_kind() {
                    kind if kind == LOCKED_PAYMENT_V1_OPERATION_TYPE
                        || kind == LOCKED_PAYMENT_V2_OPERATION_TYPE =>
                    {
                        let metadata = operation
                            .try_meta::<LockedPaymentOperationMeta>()
                            .context("setup-payment operation lacks exact accounting metadata")?;
                        let ranges = metadata.recovery_ranges_for_accounting(primary_module)?;
                        let transaction =
                            recover_accepted_locked_payment(&client, key.operation_id, ranges)
                                .await?;
                        let (input_msats, output_msats) =
                            primary_transaction_amounts(&transaction, primary_module)?;
                        accounting.setup_fee_msats = checked_add_accounting(
                            accounting.setup_fee_msats,
                            input_msats
                                .checked_sub(output_msats)
                                .context("accepted setup-payment outputs exceed payer inputs")?,
                        )?;
                        accounting.setup_output_msats = checked_add_accounting(
                            accounting.setup_output_msats,
                            primary_range_amount(&transaction, primary_module, ranges.outputs)?,
                        )?;
                        accounting.setup_transaction_count = accounting
                            .setup_transaction_count
                            .checked_add(1)
                            .context("too many setup-payment transactions")?;
                    }
                    kind if kind == fedimint_mintv2_common::KIND.as_str() => {
                        let Ok(metadata) =
                            operation.try_meta::<fedimint_mintv2_client::MintOperationMeta>()
                        else {
                            continue;
                        };
                        let fedimint_mintv2_client::MintOperationMeta::Receive {
                            change_outpoint_range,
                            ..
                        } = metadata
                        else {
                            continue;
                        };
                        let transaction = locked_payment_created_transaction_by_txid(
                            &client,
                            key.operation_id,
                            change_outpoint_range.txid(),
                        )
                        .await?;
                        require_locked_payment_accepted(
                            &client,
                            key.operation_id,
                            transaction.tx_hash(),
                        )
                        .await?;
                        client
                            .await_primary_bitcoin_module_outputs(
                                key.operation_id,
                                change_outpoint_range.into_iter().collect(),
                            )
                            .await
                            .context("finalize audited mint-v2 receive change")?;
                        let (input_msats, output_msats) =
                            primary_transaction_amounts(&transaction, primary_module)?;
                        accounting.received_input_msats =
                            checked_add_accounting(accounting.received_input_msats, input_msats)?;
                        accounting.receive_fee_msats = checked_add_accounting(
                            accounting.receive_fee_msats,
                            input_msats
                                .checked_sub(output_msats)
                                .context("accepted mint-v2 receive outputs exceed inputs")?,
                        )?;
                    }
                    _ => {}
                }
            }
            if complete {
                break;
            }
        }
        Ok(accounting)
    }

    async fn submit_locked_v1(
        &self,
        reservation: &LockedPaymentReservation,
        federation_id: FederationId,
        issuance: &[locked_payment::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<Vec<tbs::BlindedSignature>, WalletError> {
        let spend_guard = self.spend_guard.lock().await;
        let member_allocation_msats = self
            .validate_reserved_payment_start(
                reservation,
                federation_id,
                &LockedPaymentPreflight::mint_v1(QuoteId(quote_id), issuance.to_vec()),
            )
            .await?;
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let binding_hash = sha256::Hash::hash(&quote_id).to_byte_array();
        let operation_id = locked_payment_v1_operation_id(mint.id, issuance, binding_hash);
        let quoted_bundle = locked_payment::foreign_output_bundle(mint.id, issuance);
        let output_count = quoted_bundle.outputs().len();
        let (operation_id, ranges) = match locked_payment_operation(
            &client,
            operation_id,
            LOCKED_PAYMENT_V1_OPERATION_TYPE,
            LockedPaymentGeneration::MintV1,
            mint.id,
            binding_hash,
            output_count,
        )
        .await?
        {
            LockedPaymentOperation::Existing {
                operation_id,
                ranges,
            } => (operation_id, ranges),
            LockedPaymentOperation::New { operation_id } => {
                let maximum_net_debit_msats = self
                    .maximum_reserved_payment_net_debit(federation_id, member_allocation_msats)
                    .await?;
                let (output_amount, output_fee) = mint_v1_output_amount_and_fee(&mint, issuance)?;
                let net_debit =
                    locked_payment_net_debit(&client, operation_id, output_amount, output_fee)
                        .await?;
                if net_debit.msats > maximum_net_debit_msats {
                    return Err(anyhow::anyhow!(
                        "current locked-payment cost would consume another hold or the reserve"
                    )
                    .into());
                }
                let submit = finalize_reserved_transaction(
                    &client,
                    operation_id,
                    LOCKED_PAYMENT_V1_OPERATION_TYPE,
                    LockedPaymentGeneration::MintV1,
                    mint.id,
                    binding_hash,
                    output_count,
                    fedimint_client_module::transaction::TransactionBuilder::new()
                        .with_outputs(quoted_bundle.clone()),
                )
                .await;
                match submit {
                    Ok(change) => (
                        operation_id,
                        LockedPaymentRanges {
                            outputs: quoted_output_range(change, output_count)?,
                            change,
                        },
                    ),
                    Err(submit_error) => match locked_payment_operation(
                        &client,
                        operation_id,
                        LOCKED_PAYMENT_V1_OPERATION_TYPE,
                        LockedPaymentGeneration::MintV1,
                        mint.id,
                        binding_hash,
                        output_count,
                    )
                    .await?
                    {
                        LockedPaymentOperation::Existing {
                            operation_id,
                            ranges,
                        } => (operation_id, ranges),
                        LockedPaymentOperation::New { .. } => {
                            return Err(submit_error)
                                .context("fund and submit locked payment")
                                .map_err(Into::into);
                        }
                    },
                }
            }
        };
        self.start_reserved_locked_payment(reservation, QuoteId(quote_id))
            .await?;
        drop(spend_guard);
        // TransactionBuilder retains the explicitly supplied foreign-output
        // bundle first and finalize_transaction appends any balancing change,
        // so the recovered quoted range starts at zero in its original order.
        recover_accepted_locked_payment(&client, operation_id, ranges).await?;
        collect_locked_v1_signatures(&client, &mint, operation_id, ranges.outputs, issuance).await
    }

    /// Recover a previously submitted mint-v1 locked payment without ever
    /// creating or funding a transaction.
    pub async fn recover_locked_v1(
        &self,
        federation_id: FederationId,
        issuance: &[locked_payment::IssuanceRequest],
        quote_id: [u8; 32],
        reservation_id: &PaymentReservationId,
    ) -> Result<LockedPaymentRecovery<Vec<tbs::BlindedSignature>>, WalletError> {
        let requirement = LockedPaymentPreflight::mint_v1(QuoteId(quote_id), issuance.to_vec());
        if let Some(proof) = self
            .recover_terminal_locked_payment(federation_id, reservation_id, &requirement)
            .await?
        {
            return Ok(LockedPaymentRecovery::Rejected(proof));
        }
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let binding_hash = sha256::Hash::hash(&quote_id).to_byte_array();
        let operation_id = locked_payment_v1_operation_id(mint.id, issuance, binding_hash);
        let LockedPaymentOperation::Existing {
            operation_id,
            ranges,
        } = locked_payment_operation(
            &client,
            operation_id,
            LOCKED_PAYMENT_V1_OPERATION_TYPE,
            LockedPaymentGeneration::MintV1,
            mint.id,
            binding_hash,
            issuance.len(),
        )
        .await?
        else {
            return Ok(LockedPaymentRecovery::Absent);
        };
        {
            let _spend_guard = self.spend_guard.lock().await;
            self.start_reserved_locked_payment(
                &LockedPaymentReservation {
                    key: payment_reservation_key(reservation_id),
                    reservation_id: reservation_id.clone(),
                },
                QuoteId(quote_id),
            )
            .await?;
        }
        match locked_payment_transaction_status(
            client.transaction_updates(operation_id).await,
            ranges.outputs.txid(),
        )
        .await
        {
            LockedPaymentTransactionStatus::Accepted => {
                recover_accepted_locked_payment(&client, operation_id, ranges).await?;
                collect_locked_v1_signatures(&client, &mint, operation_id, ranges.outputs, issuance)
                    .await
                    .map(LockedPaymentRecovery::Funded)
            }
            LockedPaymentTransactionStatus::Rejected(_) => {
                self.recover_rejected_locked_payment(
                    &client,
                    operation_id,
                    ranges.outputs.txid(),
                    mint.id,
                    reservation_id,
                    QuoteId(quote_id),
                )
                .await
            }
        }
    }

    /// Pay mint-v2 outputs whose blinded nonces belong to another party and
    /// collect the aggregate blinded signatures needed by that party.
    /// `quote_id` is the stable canonical quote identifier used for exact
    /// retry identity.
    pub async fn pay_reserved_locked_v2(
        &self,
        reservation: &LockedPaymentReservation,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        issuance: &[locked_payment_v2::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<(Vec<tbs::BlindedSignature>, OutPointRange), WalletError> {
        self.submit_locked_v2(reservation, federation_id, mint_module, issuance, quote_id)
            .await
    }

    async fn submit_locked_v2(
        &self,
        reservation: &LockedPaymentReservation,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        issuance: &[locked_payment_v2::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<(Vec<tbs::BlindedSignature>, OutPointRange), WalletError> {
        let spend_guard = self.spend_guard.lock().await;
        let member_allocation_msats = self
            .validate_reserved_payment_start(
                reservation,
                federation_id,
                &LockedPaymentPreflight::mint_v2(QuoteId(quote_id), mint_module, issuance.to_vec()),
            )
            .await?;
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        let binding_hash = sha256::Hash::hash(&quote_id).to_byte_array();
        let operation_id = locked_payment_v2_operation_id(mint_module, issuance, binding_hash);
        let quoted_bundle = locked_payment_v2::foreign_output_bundle(mint_module, issuance);
        let output_count = quoted_bundle.outputs().len();
        let (operation_id, ranges) = match locked_payment_operation(
            &client,
            operation_id,
            LOCKED_PAYMENT_V2_OPERATION_TYPE,
            LockedPaymentGeneration::MintV2,
            mint_module,
            binding_hash,
            output_count,
        )
        .await?
        {
            LockedPaymentOperation::Existing {
                operation_id,
                ranges,
            } => (operation_id, ranges),
            LockedPaymentOperation::New { operation_id } => {
                let maximum_net_debit_msats = self
                    .maximum_reserved_payment_net_debit(federation_id, member_allocation_msats)
                    .await?;
                let (output_amount, output_fee) = mint_v2_output_amount_and_fee(mint, issuance)?;
                let net_debit =
                    locked_payment_net_debit(&client, operation_id, output_amount, output_fee)
                        .await?;
                if net_debit.msats > maximum_net_debit_msats {
                    return Err(anyhow::anyhow!(
                        "current locked-payment cost would consume another hold or the reserve"
                    )
                    .into());
                }
                let submit = finalize_reserved_transaction(
                    &client,
                    operation_id,
                    LOCKED_PAYMENT_V2_OPERATION_TYPE,
                    LockedPaymentGeneration::MintV2,
                    mint_module,
                    binding_hash,
                    output_count,
                    fedimint_client_module::transaction::TransactionBuilder::new()
                        .with_outputs(quoted_bundle.clone()),
                )
                .await;
                match submit {
                    Ok(change) => (
                        operation_id,
                        LockedPaymentRanges {
                            outputs: quoted_output_range(change, output_count)?,
                            change,
                        },
                    ),
                    Err(submit_error) => match locked_payment_operation(
                        &client,
                        operation_id,
                        LOCKED_PAYMENT_V2_OPERATION_TYPE,
                        LockedPaymentGeneration::MintV2,
                        mint_module,
                        binding_hash,
                        output_count,
                    )
                    .await?
                    {
                        LockedPaymentOperation::Existing {
                            operation_id,
                            ranges,
                        } => (operation_id, ranges),
                        LockedPaymentOperation::New { .. } => {
                            return Err(submit_error)
                                .context("fund and submit mint-v2 locked payment")
                                .map_err(Into::into);
                        }
                    },
                }
            }
        };
        self.start_reserved_locked_payment(reservation, QuoteId(quote_id))
            .await?;
        drop(spend_guard);
        recover_accepted_locked_payment(&client, operation_id, ranges).await?;
        let signatures = collect_locked_v2_signatures(mint, ranges.outputs, issuance).await?;
        Ok((signatures, ranges.outputs))
    }

    /// Recover a previously submitted mint-v2 locked payment without ever
    /// creating or funding a transaction.
    pub async fn recover_locked_v2(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        issuance: &[locked_payment_v2::IssuanceRequest],
        quote_id: [u8; 32],
        reservation_id: &PaymentReservationId,
    ) -> Result<LockedPaymentRecovery<(Vec<tbs::BlindedSignature>, OutPointRange)>, WalletError>
    {
        let requirement =
            LockedPaymentPreflight::mint_v2(QuoteId(quote_id), mint_module, issuance.to_vec());
        if let Some(proof) = self
            .recover_terminal_locked_payment(federation_id, reservation_id, &requirement)
            .await?
        {
            return Ok(LockedPaymentRecovery::Rejected(proof));
        }
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        let binding_hash = sha256::Hash::hash(&quote_id).to_byte_array();
        let operation_id = locked_payment_v2_operation_id(mint_module, issuance, binding_hash);
        let LockedPaymentOperation::Existing {
            operation_id,
            ranges,
        } = locked_payment_operation(
            &client,
            operation_id,
            LOCKED_PAYMENT_V2_OPERATION_TYPE,
            LockedPaymentGeneration::MintV2,
            mint_module,
            binding_hash,
            issuance.len(),
        )
        .await?
        else {
            return Ok(LockedPaymentRecovery::Absent);
        };
        {
            let _spend_guard = self.spend_guard.lock().await;
            self.start_reserved_locked_payment(
                &LockedPaymentReservation {
                    key: payment_reservation_key(reservation_id),
                    reservation_id: reservation_id.clone(),
                },
                QuoteId(quote_id),
            )
            .await?;
        }
        match locked_payment_transaction_status(
            client.transaction_updates(operation_id).await,
            ranges.outputs.txid(),
        )
        .await
        {
            LockedPaymentTransactionStatus::Accepted => {
                recover_accepted_locked_payment(&client, operation_id, ranges).await?;
                let signatures =
                    collect_locked_v2_signatures(mint, ranges.outputs, issuance).await?;
                Ok(LockedPaymentRecovery::Funded((signatures, ranges.outputs)))
            }
            LockedPaymentTransactionStatus::Rejected(_) => {
                self.recover_rejected_locked_payment(
                    &client,
                    operation_id,
                    ranges.outputs.txid(),
                    mint_module,
                    reservation_id,
                    QuoteId(quote_id),
                )
                .await
            }
        }
    }

    /// Submit the FMan-signed raw refund transaction, collect its issuance,
    /// and reissue the resulting notes into the ordinary mint wallet state.
    pub async fn submit_refund_v1(
        &self,
        federation_id: FederationId,
        raw_transaction: &[u8],
        prepared: PreparedRefund,
        reservation_id: &PaymentReservationId,
        quote_id: QuoteId,
    ) -> Result<SettledLockedPaymentRefund, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let transaction = fedimint_core::transaction::Transaction::consensus_decode_whole(
            raw_transaction,
            client.decoders(),
        )
        .context("decode refund transaction")?;
        validate_refund_outputs(&transaction.outputs, mint.id, prepared.issuance())?;
        let txid = transaction.tx_hash();
        let outcome = client.api().submit_transaction(transaction).await;
        let fedimint_core::transaction::TransactionSubmissionOutcome(submission) = outcome
            .try_into_inner(client.decoders())
            .context("decode refund submission outcome")?;
        let submitted_txid =
            submission.map_err(|error| anyhow::anyhow!("refund rejected: {error}"))?;
        if submitted_txid != txid {
            return Err(
                anyhow::anyhow!("refund submission answered for a different transaction").into(),
            );
        }
        // Fedimint treats an exact resubmission as success after this
        // transaction has already been accepted. Re-collecting these
        // deterministic notes and passing them to `receive` resumes the same
        // reissue operation instead of crediting a second time.
        client.api().await_transaction(txid).await;

        let context = mint.context();
        let api = client.api();
        let mut shares_by_output = Vec::with_capacity(prepared.issuance().len());
        for (out_idx, request) in prepared.issuance().iter().enumerate() {
            let decoder = context.mint_decoder.clone();
            let peer_keys = context.peer_tbs_pks.clone();
            let amount = request.amount;
            let message = request.blind_nonce.0;
            let shares = api
                .request_with_strategy_retry(
                    FilterMapThreshold::new(
                        move |peer, outcome| {
                            verify_blind_share(
                                peer, &outcome, amount, message, &decoder, &peer_keys,
                            )
                            .map_err(ServerError::InvalidResponse)
                        },
                        api.all_peers().to_num_peers(),
                    ),
                    AWAIT_OUTPUT_OUTCOME_ENDPOINT.to_owned(),
                    ApiRequestErased::new(OutPoint {
                        txid,
                        out_idx: out_idx as u64,
                    }),
                )
                .await;
            shares_by_output.push(
                shares
                    .into_iter()
                    .map(|(peer, share)| (peer.to_usize() as u64, share))
                    .collect(),
            );
        }
        let keys = context.tbs_pks.iter().map(|(a, k)| (a, *k)).collect();
        let signatures = locked_payment::aggregate_payment_signatures(
            prepared.issuance(),
            &shares_by_output,
            &keys,
        )?;
        let notes = locked_payment::verify_payment(
            prepared.issuance(),
            prepared.secrets(),
            &signatures,
            &keys,
        )?;
        let tiered = notes
            .iter()
            .map(|note| (note.amount, note.client_spendable_note()))
            .collect::<TieredMulti<_>>();
        let amount = tiered.total_amount();
        let oob = OOBNotes::new(federation_id.to_prefix(), tiered);
        self.receive(federation_id, &oob.to_string()).await?;
        let proof = self
            .record_locked_payment_terminal(reservation_id, quote_id)
            .await?;
        Ok(SettledLockedPaymentRefund {
            amount,
            release_proof: proof,
        })
    }

    /// Submit a signed mint-v2 refund transaction and reissue the resulting
    /// FI-owned notes into the ordinary wallet balance.
    pub async fn submit_refund_v2(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        raw_transaction: &[u8],
        prepared: PreparedRefundV2,
        reservation_id: &PaymentReservationId,
        quote_id: QuoteId,
    ) -> Result<SettledLockedPaymentRefund, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        let transaction = fedimint_core::transaction::Transaction::consensus_decode_whole(
            raw_transaction,
            client.decoders(),
        )
        .context("decode mint-v2 refund transaction")?;
        validate_refund_outputs_v2(&transaction.outputs, mint_module, prepared.issuance())?;
        let txid = transaction.tx_hash();
        let outcome = client.api().submit_transaction(transaction).await;
        let fedimint_core::transaction::TransactionSubmissionOutcome(submission) = outcome
            .try_into_inner(client.decoders())
            .context("decode mint-v2 refund submission outcome")?;
        let submitted_txid =
            submission.map_err(|error| anyhow::anyhow!("refund rejected: {error}"))?;
        if submitted_txid != txid {
            return Err(anyhow::anyhow!(
                "mint-v2 refund submission answered for a different transaction"
            )
            .into());
        }
        // Exact resubmission is accepted idempotently by Fedimint. The
        // deterministically reconstructed notes then use mint-v2's idempotent
        // receive operation, so replay returns the same amount without a
        // second wallet credit.
        let count = u64::try_from(prepared.issuance().len()).context("too many refund outputs")?;
        let range = OutPointRange::new(txid, IdxRange::from(0..count));
        let signatures = mint
            .await_output_signatures(
                range,
                prepared
                    .issuance()
                    .iter()
                    .map(|request| (request.denomination, request.blind_nonce))
                    .collect(),
            )
            .await
            .context("finalize mint-v2 refund outputs")?;
        let notes = mint
            .finalize_external_issuance(prepared.private(), &signatures)
            .context("verify mint-v2 refund notes")?;
        let amount = self.receive_v2_notes(federation_id, mint, notes).await?;
        let proof = self
            .record_locked_payment_terminal(reservation_id, quote_id)
            .await?;
        Ok(SettledLockedPaymentRefund {
            amount,
            release_proof: proof,
        })
    }
}

#[cfg(test)]
fn exact_foreign_output_cost(
    outputs: impl IntoIterator<Item = anyhow::Result<(Amount, Amount)>>,
) -> anyhow::Result<Amount> {
    let (amount, fee) = exact_foreign_output_amount_and_fee(outputs)?;
    amount
        .checked_add(fee)
        .context("exact foreign-output preflight amount overflow")
}

fn exact_foreign_output_amount_and_fee(
    outputs: impl IntoIterator<Item = anyhow::Result<(Amount, Amount)>>,
) -> anyhow::Result<(Amount, Amount)> {
    outputs.into_iter().try_fold(
        (Amount::ZERO, Amount::ZERO),
        |(total_amount, total_fee), output| {
            let (amount, fee) = output?;
            Ok((
                total_amount
                    .checked_add(amount)
                    .context("exact foreign-output amount overflow")?,
                total_fee
                    .checked_add(fee)
                    .context("exact foreign-output fee overflow")?,
            ))
        },
    )
}

fn mint_v1_output_amount_and_fee(
    mint: &MintClientModule,
    issuance: &[locked_payment::IssuanceRequest],
) -> anyhow::Result<(Amount, Amount)> {
    exact_foreign_output_amount_and_fee(issuance.iter().map(|request| {
        let output = MintOutput::new_v0(request.amount, request.blind_nonce);
        let fee = mint
            .output_fee(&Amounts::new_bitcoin(request.amount), &output)
            .context("mint-v1 output fee unavailable")?
            .get_bitcoin();
        Ok((request.amount, fee))
    }))
}

fn mint_v2_output_amount_and_fee(
    mint: &fedimint_mintv2_client::MintClientModule,
    issuance: &[locked_payment_v2::IssuanceRequest],
) -> anyhow::Result<(Amount, Amount)> {
    exact_foreign_output_amount_and_fee(issuance.iter().map(|request| {
        let amount = request.denomination.amount();
        let output = fedimint_mintv2_common::MintOutput::new_v0(
            request.denomination,
            request.blind_nonce,
            request.tweak,
        );
        let fee = mint
            .output_fee(&Amounts::new_bitcoin(amount), &output)
            .context("mint-v2 output fee unavailable")?
            .get_bitcoin();
        Ok((amount, fee))
    }))
}

async fn locked_payment_net_debit(
    client: &ClientHandleArc,
    operation_id: OperationId,
    output_amount: Amount,
    output_fee: Amount,
) -> anyhow::Result<Amount> {
    let fee = client
        .fee_quote(
            operation_id,
            FeeQuoteRequest {
                input_amount: Amounts::ZERO,
                output_amount: Amounts::new_bitcoin(output_amount),
                input_fee: Amounts::ZERO,
                output_fee: Amounts::new_bitcoin(output_fee),
            },
        )
        .await
        .context("quote locked-payment transaction fee")?
        .total()
        .get_bitcoin();
    output_amount
        .checked_add(fee)
        .context("locked-payment net debit overflow")
}

fn locked_payment_v1_operation_id(
    mint_module: ModuleInstanceId,
    issuance: &[locked_payment::IssuanceRequest],
    binding_hash: [u8; 32],
) -> OperationId {
    let issuance = locked_payment_v1_issuance_bytes(issuance);
    locked_payment_operation_id(
        LockedPaymentGeneration::MintV1,
        mint_module,
        binding_hash,
        &issuance,
    )
}

fn locked_payment_v2_operation_id(
    mint_module: ModuleInstanceId,
    issuance: &[locked_payment_v2::IssuanceRequest],
    binding_hash: [u8; 32],
) -> OperationId {
    let issuance = locked_payment_v2_issuance_bytes(issuance);
    locked_payment_operation_id(
        LockedPaymentGeneration::MintV2,
        mint_module,
        binding_hash,
        &issuance,
    )
}

fn locked_payment_operation_id(
    generation: LockedPaymentGeneration,
    mint_module: ModuleInstanceId,
    binding_hash: [u8; 32],
    issuance: &[u8],
) -> OperationId {
    let generation = match generation {
        LockedPaymentGeneration::MintV1 => 1,
        LockedPaymentGeneration::MintV2 => 2,
    };
    let mut bytes = Vec::with_capacity(LOCKED_PAYMENT_OPERATION_ID_DOMAIN.len() + 1 + 2 + 32 + 32);
    bytes.extend(LOCKED_PAYMENT_OPERATION_ID_DOMAIN);
    bytes.push(generation);
    bytes.extend(mint_module.consensus_encode_to_vec());
    bytes.extend(binding_hash);
    bytes.extend(sha256::Hash::hash(issuance).to_byte_array());
    OperationId(sha256::Hash::hash(&bytes).to_byte_array())
}

fn locked_payment_v1_issuance_bytes(issuance: &[locked_payment::IssuanceRequest]) -> Vec<u8> {
    issuance
        .iter()
        .flat_map(|request| (request.amount.msats, request.blind_nonce).consensus_encode_to_vec())
        .collect()
}

fn locked_payment_v2_issuance_bytes(issuance: &[locked_payment_v2::IssuanceRequest]) -> Vec<u8> {
    issuance
        .iter()
        .flat_map(|request| {
            (request.denomination, request.blind_nonce, request.tweak).consensus_encode_to_vec()
        })
        .collect()
}

async fn locked_payment_transaction_status(
    updates: TransactionUpdates,
    txid: TransactionId,
) -> LockedPaymentTransactionStatus {
    match updates.await_tx_accepted(txid).await {
        Ok(()) => LockedPaymentTransactionStatus::Accepted,
        Err(reason) => LockedPaymentTransactionStatus::Rejected(reason),
    }
}

async fn require_locked_payment_accepted(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    txid: TransactionId,
) -> Result<(), WalletError> {
    match locked_payment_transaction_status(client.transaction_updates(operation_id).await, txid)
        .await
    {
        LockedPaymentTransactionStatus::Accepted => Ok(()),
        LockedPaymentTransactionStatus::Rejected(reason) => {
            Err(anyhow::anyhow!("locked payment rejected: {reason}").into())
        }
    }
}

async fn await_locked_payment_change(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    change_range: OutPointRange,
) -> Result<(), WalletError> {
    client
        .await_primary_bitcoin_module_outputs(operation_id, change_range.into_iter().collect())
        .await
        .context("finalize locked-payment change")
        .map_err(Into::into)
}

/// Recover one exact accepted transaction and await its persisted payer-change
/// range. This is the common restart boundary used by payment recovery and by
/// the reopened real-wallet accounting reproduction.
async fn recover_accepted_locked_payment(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    ranges: LockedPaymentRanges,
) -> Result<fedimint_core::transaction::Transaction, WalletError> {
    let transaction =
        locked_payment_created_transaction_by_txid(client, operation_id, ranges.outputs.txid())
            .await?;
    require_locked_payment_accepted(client, operation_id, transaction.tx_hash()).await?;
    await_locked_payment_change(client, operation_id, ranges.change).await?;
    Ok(transaction)
}

fn checked_add_accounting(total: u64, amount: u64) -> anyhow::Result<u64> {
    total
        .checked_add(amount)
        .context("payment accounting overflow")
}

fn primary_input_amount(
    input: &fedimint_core::core::DynInput,
    primary_module: ModuleInstanceId,
) -> anyhow::Result<u64> {
    anyhow::ensure!(
        input.module_instance_id() == primary_module,
        "audited transaction contains a non-primary input"
    );
    if let Some(input) = input
        .as_any()
        .downcast_ref::<fedimint_mint_common::MintInput>()
    {
        return match input {
            fedimint_mint_common::MintInput::V0(input) => Ok(input.amount.msats),
            _ => anyhow::bail!("unsupported mint-v1 input variant in accounting"),
        };
    }
    if let Some(input) = input
        .as_any()
        .downcast_ref::<fedimint_mintv2_common::MintInput>()
    {
        return match input {
            fedimint_mintv2_common::MintInput::V0(input) => Ok(input.note.amount().msats),
            _ => anyhow::bail!("unsupported mint-v2 input variant in accounting"),
        };
    }
    anyhow::bail!("unsupported primary input type in payment accounting")
}

fn primary_output_amount(
    output: &DynOutput,
    primary_module: ModuleInstanceId,
) -> anyhow::Result<u64> {
    anyhow::ensure!(
        output.module_instance_id() == primary_module,
        "audited transaction contains a non-primary output"
    );
    if let Some(output) = output
        .as_any()
        .downcast_ref::<fedimint_mint_common::MintOutput>()
    {
        return match output {
            fedimint_mint_common::MintOutput::V0(output) => Ok(output.amount.msats),
            _ => anyhow::bail!("unsupported mint-v1 output variant in accounting"),
        };
    }
    if let Some(output) = output
        .as_any()
        .downcast_ref::<fedimint_mintv2_common::MintOutput>()
    {
        return match output {
            fedimint_mintv2_common::MintOutput::V0(output) => Ok(output.amount().msats),
            _ => anyhow::bail!("unsupported mint-v2 output variant in accounting"),
        };
    }
    anyhow::bail!("unsupported primary output type in payment accounting")
}

fn primary_transaction_amounts(
    transaction: &fedimint_core::transaction::Transaction,
    primary_module: ModuleInstanceId,
) -> anyhow::Result<(u64, u64)> {
    let inputs = transaction.inputs.iter().try_fold(0u64, |total, input| {
        checked_add_accounting(total, primary_input_amount(input, primary_module)?)
    })?;
    let outputs = transaction.outputs.iter().try_fold(0u64, |total, output| {
        checked_add_accounting(total, primary_output_amount(output, primary_module)?)
    })?;
    Ok((inputs, outputs))
}

fn primary_range_amount(
    transaction: &fedimint_core::transaction::Transaction,
    primary_module: ModuleInstanceId,
    range: OutPointRange,
) -> anyhow::Result<u64> {
    anyhow::ensure!(
        range.txid() == transaction.tx_hash(),
        "accounting range belongs to another transaction"
    );
    range.into_iter().try_fold(0u64, |total, outpoint| {
        let out_idx = usize::try_from(outpoint.out_idx)
            .context("accounting output index does not fit usize")?;
        let output = transaction
            .outputs
            .get(out_idx)
            .context("accounting output range exceeds transaction")?;
        checked_add_accounting(total, primary_output_amount(output, primary_module)?)
    })
}

fn primary_input_multiset(
    transaction: &fedimint_core::transaction::Transaction,
    mint_module: ModuleInstanceId,
) -> anyhow::Result<BTreeMap<Vec<u8>, u64>> {
    transaction
        .inputs
        .iter()
        .try_fold(BTreeMap::<Vec<u8>, u64>::new(), |mut inputs, input| {
            anyhow::ensure!(
                input.module_instance_id() == mint_module,
                "locked-payment refund transaction contains a non-payer input"
            );
            let count = inputs.entry(input.consensus_encode_to_vec()).or_default();
            *count = count
                .checked_add(1)
                .context("locked-payment input multiplicity overflow")?;
            Ok(inputs)
        })
}

fn multiset_is_subset(
    candidate: &BTreeMap<Vec<u8>, u64>,
    original: &BTreeMap<Vec<u8>, u64>,
) -> bool {
    candidate.iter().all(|(input, count)| {
        original
            .get(input)
            .is_some_and(|original_count| count <= original_count)
    })
}

/// Follow every automatic refund transaction created under the rejected
/// operation until accepted candidates cover the exact original input
/// multiset and all their primary outputs are spendable. Update history is
/// replayed after restart and may be unordered or duplicated, so candidates
/// are joined by txid and finalized idempotently.
async fn await_rejected_input_refunds<F, Fut>(
    operation_id: OperationId,
    original: fedimint_core::transaction::Transaction,
    mint_module: ModuleInstanceId,
    mut updates: TransactionUpdates,
    mut await_outputs: F,
) -> anyhow::Result<()>
where
    F: FnMut(Vec<OutPoint>) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let original_txid = original.tx_hash();
    let original_inputs = primary_input_multiset(&original, mint_module)?;
    anyhow::ensure!(
        !original_inputs.is_empty(),
        "rejected locked payment contains no payer inputs"
    );

    let mut transactions = BTreeMap::new();
    let mut accepted = BTreeSet::new();
    let mut rejected = BTreeSet::new();
    let mut counted = BTreeSet::new();
    let mut recovered = BTreeMap::<Vec<u8>, u64>::new();
    let mut refund_outpoints = Vec::new();
    while let Some(update) = updates.update_stream.next().await {
        anyhow::ensure!(
            update.operation_id == operation_id,
            "locked-payment refund stream returned another operation"
        );
        match update.state {
            fedimint_client_module::transaction::TxSubmissionStates::Created(transaction) => {
                transactions.insert(transaction.tx_hash(), transaction);
            }
            fedimint_client_module::transaction::TxSubmissionStates::Accepted(txid) => {
                anyhow::ensure!(
                    !rejected.contains(&txid),
                    "refund transaction has contradictory terminal states"
                );
                accepted.insert(txid);
            }
            fedimint_client_module::transaction::TxSubmissionStates::Rejected(txid, _) => {
                anyhow::ensure!(
                    !accepted.contains(&txid),
                    "refund transaction has contradictory terminal states"
                );
                rejected.insert(txid);
            }
            fedimint_client_module::transaction::TxSubmissionStates::NonRetryableError(_) => {}
        }

        let ready = accepted
            .iter()
            .filter(|txid| **txid != original_txid && !counted.contains(*txid))
            .filter_map(|txid| transactions.get(txid).cloned().map(|tx| (*txid, tx)))
            .collect::<Vec<_>>();
        for (txid, transaction) in ready {
            let candidate_inputs = primary_input_multiset(&transaction, mint_module)?;
            anyhow::ensure!(
                !candidate_inputs.is_empty()
                    && multiset_is_subset(&candidate_inputs, &original_inputs),
                "accepted operation sibling is not an exact rejected-input refund"
            );
            anyhow::ensure!(
                !transaction.outputs.is_empty()
                    && transaction
                        .outputs
                        .iter()
                        .all(|output| output.module_instance_id() == mint_module),
                "accepted rejected-input refund has non-primary or empty outputs"
            );
            for (input, count) in &candidate_inputs {
                let recovered_count = recovered.entry(input.clone()).or_default();
                *recovered_count = recovered_count
                    .checked_add(*count)
                    .context("recovered input multiplicity overflow")?;
            }
            anyhow::ensure!(
                multiset_is_subset(&recovered, &original_inputs),
                "accepted refund transactions overlap original inputs"
            );
            refund_outpoints.extend((0..transaction.outputs.len()).map(|out_idx| OutPoint {
                txid,
                out_idx: out_idx as u64,
            }));
            counted.insert(txid);
            if recovered == original_inputs {
                await_outputs(refund_outpoints).await?;
                return Ok(());
            }
        }
    }
    anyhow::bail!("rejected-input refund stream ended before restoring every input")
}

async fn collect_locked_v1_signatures(
    client: &fedimint_client::ClientHandleArc,
    mint: &MintClientModule,
    operation_id: OperationId,
    range: OutPointRange,
    issuance: &[locked_payment::IssuanceRequest],
) -> Result<Vec<tbs::BlindedSignature>, WalletError> {
    tracing::debug!(?operation_id, txid = %range.txid(), "locked payment accepted; collecting output shares");

    let context = mint.context();
    // `await_output_outcome` is a global endpoint even though it decodes a
    // mint outcome. Prefixing it with the mint module id retries a nonexistent
    // endpoint forever.
    let api = client.api();
    let mut all_shares = Vec::with_capacity(issuance.len());
    for (out_idx, request) in issuance.iter().enumerate() {
        let decoder = context.mint_decoder.clone();
        let peer_keys = context.peer_tbs_pks.clone();
        let amount = request.amount;
        let message = request.blind_nonce.0;
        let shares = api
            .request_with_strategy_retry(
                FilterMapThreshold::new(
                    move |peer, outcome| {
                        verify_blind_share(peer, &outcome, amount, message, &decoder, &peer_keys)
                            .map_err(ServerError::InvalidResponse)
                    },
                    api.all_peers().to_num_peers(),
                ),
                AWAIT_OUTPUT_OUTCOME_ENDPOINT.to_owned(),
                ApiRequestErased::new(OutPoint {
                    txid: range.txid(),
                    out_idx: out_idx as u64,
                }),
            )
            .await;
        tracing::debug!(?operation_id, txid = %range.txid(), out_idx, "collected locked payment output shares");
        all_shares.push(
            shares
                .into_iter()
                .map(|(peer, share)| (peer.to_usize() as u64, share))
                .collect(),
        );
    }
    let mint_keys = context
        .tbs_pks
        .iter()
        .map(|(amount, key)| (amount, *key))
        .collect();
    locked_payment::aggregate_payment_signatures(issuance, &all_shares, &mint_keys)
        .map_err(Into::into)
}

async fn collect_locked_v2_signatures(
    mint: &fedimint_mintv2_client::MintClientModule,
    range: OutPointRange,
    issuance: &[locked_payment_v2::IssuanceRequest],
) -> Result<Vec<tbs::BlindedSignature>, WalletError> {
    mint.await_output_signatures(
        range,
        issuance
            .iter()
            .map(|request| (request.denomination, request.blind_nonce))
            .collect(),
    )
    .await
    .context("finalize mint-v2 locked outputs")
    .map_err(Into::into)
}

fn quoted_output_range(
    change: OutPointRange,
    output_count: usize,
) -> anyhow::Result<OutPointRange> {
    let output_count = u64::try_from(output_count).context("too many locked-payment outputs")?;
    Ok(OutPointRange::new(
        change.txid(),
        IdxRange::from(0..output_count),
    ))
}

async fn locked_payment_operation(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    operation_type: &str,
    generation: LockedPaymentGeneration,
    mint_module: ModuleInstanceId,
    binding_hash: [u8; 32],
    output_count: usize,
) -> anyhow::Result<LockedPaymentOperation> {
    let ranges = existing_locked_payment_ranges(
        client,
        operation_id,
        operation_type,
        generation,
        mint_module,
        binding_hash,
        output_count,
    )
    .await?;
    Ok(match ranges {
        Some(ranges) => LockedPaymentOperation::Existing {
            operation_id,
            ranges,
        },
        None => LockedPaymentOperation::New { operation_id },
    })
}

async fn existing_locked_payment_ranges(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    operation_type: &str,
    generation: LockedPaymentGeneration,
    mint_module: ModuleInstanceId,
    binding_hash: [u8; 32],
    output_count: usize,
) -> anyhow::Result<Option<LockedPaymentRanges>> {
    let Some(operation) = client.operation_log().get_operation(operation_id).await else {
        return Ok(None);
    };
    anyhow::ensure!(
        operation.operation_module_kind() == operation_type,
        "operation {operation_id:?} already exists with a different type; refusing to spend again"
    );
    let metadata = operation
        .try_meta::<LockedPaymentOperationMeta>()
        .with_context(|| {
            format!(
                "operation {operation_id:?} has no compatible resumability metadata; refusing to spend again"
            )
        })?;
    metadata
        .validate(generation, mint_module, binding_hash, output_count)
        .with_context(|| {
            format!(
                "operation {operation_id:?} has incompatible resumability metadata; refusing to spend again"
            )
        })
        .map(Some)
}

async fn locked_payment_created_transaction_by_txid(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    expected_txid: TransactionId,
) -> anyhow::Result<fedimint_core::transaction::Transaction> {
    locked_payment_created_transaction_matching(client, operation_id, |transaction| {
        transaction.tx_hash() == expected_txid
    })
    .await
}

async fn locked_payment_created_transaction_matching(
    client: &fedimint_client::ClientHandleArc,
    operation_id: OperationId,
    matches: impl Fn(&fedimint_core::transaction::Transaction) -> bool,
) -> anyhow::Result<fedimint_core::transaction::Transaction> {
    locked_payment_created_transaction_from_updates(
        operation_id,
        client.transaction_updates(operation_id).await,
        matches,
    )
    .await
}

async fn locked_payment_created_transaction_from_updates(
    operation_id: OperationId,
    mut updates: TransactionUpdates,
    matches: impl Fn(&fedimint_core::transaction::Transaction) -> bool,
) -> anyhow::Result<fedimint_core::transaction::Transaction> {
    while let Some(update) = updates.update_stream.next().await {
        anyhow::ensure!(
            update.operation_id == operation_id,
            "operation {operation_id:?} returned another operation's state; refusing to spend again"
        );
        match update.state {
            fedimint_client_module::transaction::TxSubmissionStates::Created(transaction)
                if matches(&transaction) =>
            {
                return Ok(transaction);
            }
            fedimint_client_module::transaction::TxSubmissionStates::Created(_)
            | fedimint_client_module::transaction::TxSubmissionStates::Accepted(_)
            | fedimint_client_module::transaction::TxSubmissionStates::Rejected(_, _)
            | fedimint_client_module::transaction::TxSubmissionStates::NonRetryableError(_) => {}
        }
    }
    anyhow::bail!(
        "operation {operation_id:?} has no matching Created transaction state; refusing to spend again"
    )
}

fn validate_refund_outputs(
    outputs: &[DynOutput],
    mint_module: ModuleInstanceId,
    issuance: &[locked_payment::IssuanceRequest],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        outputs.len() == issuance.len(),
        "refund transaction output count differs from prepared issuance"
    );
    for (output, expected) in outputs.iter().zip(issuance) {
        anyhow::ensure!(
            output.module_instance_id() == mint_module
                && output
                    .as_any()
                    .downcast_ref::<MintOutput>()
                    .is_some_and(|output| {
                        output == &MintOutput::new_v0(expected.amount, expected.blind_nonce)
                    }),
            "refund transaction outputs differ from prepared issuance"
        );
    }
    Ok(())
}

fn validate_refund_outputs_v2(
    outputs: &[DynOutput],
    mint_module: ModuleInstanceId,
    issuance: &[locked_payment_v2::IssuanceRequest],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        outputs.len() == issuance.len(),
        "mint-v2 refund transaction output count differs from prepared issuance"
    );
    for (output, expected) in outputs.iter().zip(issuance) {
        anyhow::ensure!(
            output.module_instance_id() == mint_module
                && output
                    .as_any()
                    .downcast_ref::<fedimint_mintv2_common::MintOutput>()
                    .is_some_and(|output| {
                        output
                            == &fedimint_mintv2_common::MintOutput::new_v0(
                                expected.denomination,
                                expected.blind_nonce,
                                expected.tweak,
                            )
                    }),
            "mint-v2 refund transaction outputs differ from prepared issuance"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bitcoin_hashes::Hash as _;
    use fedimint_client_module::transaction::{TxSubmissionStates, TxSubmissionStatesSM};
    use fedimint_core::TransactionId;
    use fedimint_core::core::IntoDynInstance as _;
    use fedimint_core::transaction::{Transaction, TransactionSignature};
    use fedimint_mint_common::{BlindNonce, MintInput, Nonce, Note};
    use futures::StreamExt as _;

    use super::*;

    fn reservation_preflight(byte: u8) -> LockedPaymentPreflight {
        LockedPaymentPreflight::mint_v1(QuoteId([byte; 32]), Vec::new())
    }

    fn reservation_id(label: &str) -> PaymentReservationId {
        let digest = sha256::Hash::hash(label.as_bytes()).to_byte_array();
        let encoded = digest
            .into_iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        serde_json::from_value(serde_json::json!(encoded))
            .expect("test reservation id is a canonical digest")
    }

    async fn test_wallet() -> (tempfile::TempDir, Wallet) {
        let temp = tempfile::tempdir().unwrap();
        let wallet = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        (temp, wallet)
    }

    async fn seed_reservation(
        wallet: &Wallet,
        reservation_id: &PaymentReservationId,
        quote_ids: Vec<[u8; 32]>,
        started: Vec<bool>,
    ) {
        let stored = StoredLockedPaymentReservation {
            version: PAYMENT_RESERVATION_VERSION,
            federation_id: FederationId::dummy(),
            required_reserve_msats: 0,
            members: quote_ids
                .into_iter()
                .zip(started)
                .map(|(quote_id, started)| StoredLockedPaymentMember {
                    quote_id,
                    plan_hash: reservation_preflight(quote_id[0]).plan_hash(),
                    debit_msats: 1_000,
                    state: if started {
                        StoredLockedPaymentMemberState::Started
                    } else {
                        StoredLockedPaymentMemberState::Held
                    },
                })
                .collect(),
        };
        let database = reservation_db(&wallet.database);
        let mut tx = database.begin_transaction().await;
        tx.raw_insert_bytes(
            &payment_reservation_key(reservation_id),
            &stored.consensus_encode_to_vec(),
        )
        .await
        .unwrap();
        tx.commit_tx().await;
    }

    async fn load_reservation(
        wallet: &Wallet,
        reservation_id: &PaymentReservationId,
    ) -> Option<StoredLockedPaymentReservation> {
        let database = reservation_db(&wallet.database);
        let bytes = database
            .begin_transaction_nc()
            .await
            .raw_get_bytes(&payment_reservation_key(reservation_id))
            .await
            .unwrap()?;
        Some(
            StoredLockedPaymentReservation::consensus_decode_whole(
                &bytes,
                &fedimint_core::module::registry::ModuleDecoderRegistry::default(),
            )
            .unwrap(),
        )
    }

    async fn store_exact_reservation(
        wallet: &Wallet,
        reservation_id: &PaymentReservationId,
        federation_id: FederationId,
        members: Vec<(LockedPaymentPreflight, StoredLockedPaymentMemberState, u64)>,
    ) {
        let stored = StoredLockedPaymentReservation {
            version: PAYMENT_RESERVATION_VERSION,
            federation_id,
            required_reserve_msats: 0,
            members: members
                .into_iter()
                .map(
                    |(preflight, state, debit_msats)| StoredLockedPaymentMember {
                        quote_id: preflight.quote_id().0,
                        plan_hash: preflight.plan_hash(),
                        debit_msats,
                        state,
                    },
                )
                .collect(),
        };
        let database = reservation_db(&wallet.database);
        let mut tx = database.begin_transaction().await;
        tx.raw_insert_bytes(
            &payment_reservation_key(reservation_id),
            &stored.consensus_encode_to_vec(),
        )
        .await
        .unwrap();
        tx.commit_tx().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn terminal_recovery_survives_reopen_and_short_circuits_unjoined_v1_v2_clients() {
        let temp = tempfile::tempdir().unwrap();
        let federation_id = FederationId::dummy();
        let v1_id = reservation_id("terminal-v1-reopen");
        let v1 = reservation_preflight(1);
        let wallet = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        store_exact_reservation(
            &wallet,
            &v1_id,
            federation_id,
            vec![(v1, StoredLockedPaymentMemberState::Terminal, 1_000)],
        )
        .await;
        drop(wallet);

        let reopened = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        let LockedPaymentRecovery::Rejected(proof) = reopened
            .recover_locked_v1(federation_id, &[], [1; 32], &v1_id)
            .await
            .expect("terminal v1 journal recovers before opening an unjoined client")
        else {
            panic!("terminal v1 journal must return release authority");
        };
        reopened.release_locked_payment_member(proof).await.unwrap();
        let LockedPaymentRecovery::Rejected(proof) = reopened
            .recover_locked_v1(federation_id, &[], [1; 32], &v1_id)
            .await
            .expect("released v1 tombstone remains recoverable")
        else {
            panic!("released v1 journal must return release authority");
        };
        reopened.release_locked_payment_member(proof).await.unwrap();

        let module = ModuleInstanceId::from(7u16);
        let v2_id = reservation_id("terminal-v2-unjoined");
        let v2_issuance = vec![locked_payment_v2::IssuanceRequest {
            denomination: locked_payment_v2::denomination_from_amount(1_024).unwrap(),
            blind_nonce: tbs::BlindedMessage(bls12_381::G1Affine::generator()),
            tweak: [8; 16],
        }];
        store_exact_reservation(
            &reopened,
            &v2_id,
            federation_id,
            vec![(
                LockedPaymentPreflight::mint_v2(QuoteId([2; 32]), module, v2_issuance.clone()),
                StoredLockedPaymentMemberState::Released,
                2_000,
            )],
        )
        .await;
        assert!(matches!(
            reopened
                .recover_locked_v2(federation_id, module, &v2_issuance, [2; 32], &v2_id)
                .await
                .expect("released v2 tombstone recovers without a joined client"),
            LockedPaymentRecovery::Rejected(_)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn terminal_recovery_rejects_v1_v2_plan_and_debit_binding_mismatches() {
        let (_temp, wallet) = test_wallet().await;
        let federation_id = FederationId::dummy();
        let v1_id = reservation_id("terminal-v1-mismatch");
        store_exact_reservation(
            &wallet,
            &v1_id,
            federation_id,
            vec![(
                reservation_preflight(3),
                StoredLockedPaymentMemberState::Terminal,
                1_000,
            )],
        )
        .await;
        let changed_v1 = vec![locked_payment::IssuanceRequest {
            amount: Amount::from_msats(1_000),
            blind_nonce: BlindNonce(tbs::BlindedMessage(bls12_381::G1Affine::generator())),
        }];
        let error = match wallet
            .recover_locked_v1(federation_id, &changed_v1, [3; 32], &v1_id)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("mismatched v1 recovery must fail closed"),
        };
        assert!(
            error.to_string().contains("output plan changed"),
            "{error:#}"
        );

        let module = ModuleInstanceId::from(7u16);
        let v2_id = reservation_id("terminal-v2-mismatch");
        let v2 = vec![locked_payment_v2::IssuanceRequest {
            denomination: locked_payment_v2::denomination_from_amount(1_024).unwrap(),
            blind_nonce: tbs::BlindedMessage(bls12_381::G1Affine::generator()),
            tweak: [4; 16],
        }];
        store_exact_reservation(
            &wallet,
            &v2_id,
            federation_id,
            vec![(
                LockedPaymentPreflight::mint_v2(QuoteId([4; 32]), module, v2.clone()),
                StoredLockedPaymentMemberState::Terminal,
                2_000,
            )],
        )
        .await;
        let mut changed_v2 = v2;
        changed_v2[0].tweak = [5; 16];
        let error = match wallet
            .recover_locked_v2(federation_id, module, &changed_v2, [4; 32], &v2_id)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("mismatched v2 recovery must fail closed"),
        };
        assert!(
            error.to_string().contains("output plan changed"),
            "{error:#}"
        );

        let proof = wallet
            .recover_terminal_locked_payment(federation_id, &v1_id, &reservation_preflight(3))
            .await
            .unwrap()
            .unwrap();
        let mut stored = load_reservation(&wallet, &v1_id).await.unwrap();
        stored.members[0].debit_msats += 1;
        let database = reservation_db(&wallet.database);
        let mut tx = database.begin_transaction().await;
        tx.raw_insert_bytes(
            &payment_reservation_key(&v1_id),
            &stored.consensus_encode_to_vec(),
        )
        .await
        .unwrap();
        tx.commit_tx().await;
        let error = wallet
            .release_locked_payment_member(proof)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("payment binding changed"),
            "{error:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn existing_partial_reservation_reconstructs_before_full_fundability() {
        let (_temp, wallet) = test_wallet().await;
        for (reservation_id, started) in [
            (reservation_id("one-consumed"), vec![true, false, false]),
            (
                reservation_id("n-minus-one-consumed"),
                vec![true, true, false],
            ),
        ] {
            seed_reservation(
                &wallet,
                &reservation_id,
                vec![[1; 32], [2; 32], [3; 32]],
                started,
            )
            .await;
            let reservation = wallet
                .reserve_locked_payments(
                    FederationId::dummy(),
                    &reservation_id,
                    &[
                        reservation_preflight(1),
                        reservation_preflight(2),
                        reservation_preflight(3),
                    ],
                )
                .await
                .expect("existing journal reconstructs without a joined/funded client");
            assert_eq!(reservation.reservation_id(), &reservation_id);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fee_bearing_reservation_reconstructs_and_releases_before_outputs() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("fee-bearing-rollback");
        let blind_nonce = BlindNonce(tbs::BlindedMessage(bls12_381::G1Affine::generator()));
        let preflight = LockedPaymentPreflight::mint_v1(
            QuoteId([1; 32]),
            vec![locked_payment::IssuanceRequest {
                amount: Amount::from_msats(1_000),
                blind_nonce,
            }],
        );
        let database = reservation_db(&wallet.database);
        let mut tx = database.begin_transaction().await;
        tx.raw_insert_bytes(
            &payment_reservation_key(&reservation_id),
            &StoredLockedPaymentReservation {
                version: PAYMENT_RESERVATION_VERSION,
                federation_id: FederationId::dummy(),
                required_reserve_msats: 0,
                members: vec![StoredLockedPaymentMember {
                    quote_id: [1; 32],
                    plan_hash: preflight.plan_hash(),
                    debit_msats: 1_000,
                    state: StoredLockedPaymentMemberState::Held,
                }],
            }
            .consensus_encode_to_vec(),
        )
        .await
        .unwrap();
        tx.commit_tx().await;
        let reservation = wallet
            .reserve_locked_payments(FederationId::dummy(), &reservation_id, &[preflight])
            .await
            .expect("exact fee-bearing journal reconstructs");

        wallet
            .release_locked_payment_reservation(reservation)
            .await
            .expect("unstarted aggregate rolls back");
        assert_eq!(
            load_reservation(&wallet, &reservation_id)
                .await
                .unwrap()
                .members[0]
                .state,
            StoredLockedPaymentMemberState::Released,
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_reservation_id_with_different_exact_quotes_fails_closed() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("reservation-mismatch");
        seed_reservation(&wallet, &reservation_id, vec![[1; 32]], vec![false]).await;

        assert!(
            wallet
                .reserve_locked_payments(
                    FederationId::dummy(),
                    &reservation_id,
                    &[reservation_preflight(2)],
                )
                .await
                .is_err()
        );
        assert_eq!(
            load_reservation(&wallet, &reservation_id)
                .await
                .unwrap()
                .members[0]
                .quote_id,
            [1; 32],
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recover_existing_reservation_is_exact_and_never_creates_a_journal() {
        let (_temp, wallet) = test_wallet().await;
        let absent_id = reservation_id("recover-reservation-absent");
        assert!(
            wallet
                .recover_locked_payment_reservation(
                    FederationId::dummy(),
                    &absent_id,
                    &[reservation_preflight(1)],
                    Amount::ZERO,
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(load_reservation(&wallet, &absent_id).await.is_none());

        let existing_id = reservation_id("recover-reservation-existing");
        seed_reservation(&wallet, &existing_id, vec![[1; 32]], vec![false]).await;
        assert!(
            wallet
                .recover_locked_payment_reservation(
                    FederationId::dummy(),
                    &existing_id,
                    &[reservation_preflight(1)],
                    Amount::ZERO,
                )
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            load_reservation(&wallet, &existing_id)
                .await
                .unwrap()
                .members[0]
                .state,
            StoredLockedPaymentMemberState::Held,
        );

        let error = wallet
            .recover_locked_payment_reservation(
                FederationId::dummy(),
                &existing_id,
                &[reservation_preflight(2)],
                Amount::ZERO,
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("quote order changed"),
            "{error:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_semantic_quote_ids_fail_before_wallet_planning() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("duplicate-quotes");
        let error = wallet
            .reserve_locked_payments(
                FederationId::dummy(),
                &reservation_id,
                &[reservation_preflight(1), reservation_preflight(1)],
            )
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("duplicate quote ids"),
            "{error:#}"
        );
        assert!(load_reservation(&wallet, &reservation_id).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_quote_with_a_different_output_plan_fails_closed() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("plan-mismatch");
        seed_reservation(&wallet, &reservation_id, vec![[1; 32]], vec![false]).await;
        let blind_nonce = BlindNonce(tbs::BlindedMessage(bls12_381::G1Affine::generator()));
        let changed = LockedPaymentPreflight::mint_v1(
            QuoteId([1; 32]),
            vec![locked_payment::IssuanceRequest {
                amount: Amount::from_msats(1_000),
                blind_nonce,
            }],
        );

        let error = wallet
            .reserve_locked_payments(FederationId::dummy(), &reservation_id, &[changed])
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("output plan changed"),
            "{error:#}"
        );
        assert_eq!(
            load_reservation(&wallet, &reservation_id)
                .await
                .unwrap()
                .members[0]
                .state,
            StoredLockedPaymentMemberState::Held,
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn partial_start_journal_survives_wallet_drop_and_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let reservation_id = reservation_id("reopen-partial-start");
        let wallet = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        seed_reservation(
            &wallet,
            &reservation_id,
            vec![[1; 32], [2; 32]],
            vec![false, false],
        )
        .await;
        let reservation = LockedPaymentReservation {
            key: payment_reservation_key(&reservation_id),
            reservation_id: reservation_id.clone(),
        };
        wallet
            .start_reserved_locked_payment(&reservation, QuoteId([1; 32]))
            .await
            .unwrap();
        drop(wallet);

        let reopened = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        let reconstructed = reopened
            .reserve_locked_payments(
                FederationId::dummy(),
                &reservation_id,
                &[reservation_preflight(1), reservation_preflight(2)],
            )
            .await
            .expect("partial journal reconstructs without planning consumed value again");
        reopened
            .start_reserved_locked_payment(&reconstructed, QuoteId([2; 32]))
            .await
            .unwrap();
        assert!(
            load_reservation(&reopened, &reservation_id)
                .await
                .unwrap()
                .members
                .iter()
                .all(|member| member.state == StoredLockedPaymentMemberState::Started)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nonzero_reserve_floor_journal_recovers_after_wallet_drop_and_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let reservation_id = reservation_id("reopen-nonzero-floor");
        let required_reserve = Amount::from_msats(5_000);
        let preflight = reservation_preflight(1);
        let wallet = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        {
            // Scope the derived handle so dropping the wallet below releases
            // the reservation database before the same directory reopens.
            let database = reservation_db(&wallet.database);
            let mut tx = database.begin_transaction().await;
            tx.raw_insert_bytes(
                &payment_reservation_key(&reservation_id),
                &StoredLockedPaymentReservation {
                    version: PAYMENT_RESERVATION_VERSION,
                    federation_id: FederationId::dummy(),
                    required_reserve_msats: required_reserve.msats,
                    members: vec![StoredLockedPaymentMember {
                        quote_id: preflight.quote_id().0,
                        plan_hash: preflight.plan_hash(),
                        debit_msats: 1_000,
                        state: StoredLockedPaymentMemberState::Held,
                    }],
                }
                .consensus_encode_to_vec(),
            )
            .await
            .unwrap();
            tx.commit_tx().await;
        }
        wallet
            .reserve_locked_payments_with_reserve(
                FederationId::dummy(),
                &reservation_id,
                std::slice::from_ref(&preflight),
                required_reserve,
            )
            .await
            .expect("fee-bearing journal with a floor reconstructs");
        drop(wallet);

        let reopened = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        let recovered = reopened
            .recover_locked_payment_reservation(
                FederationId::dummy(),
                &reservation_id,
                std::slice::from_ref(&preflight),
                required_reserve,
            )
            .await
            .expect("matching floor recovers the durable journal")
            .expect("journal exists after reopen");
        assert_eq!(recovered.reservation_id(), &reservation_id);

        let error = reopened
            .recover_locked_payment_reservation(
                FederationId::dummy(),
                &reservation_id,
                std::slice::from_ref(&preflight),
                Amount::ZERO,
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("required balance floor changed"),
            "{error:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn guardian_fee_remittance_refuses_held_locked_payment_value() {
        let (_temp, wallet) = test_wallet().await;
        seed_reservation(
            &wallet,
            &reservation_id("remittance-hold"),
            vec![[1; 32]],
            vec![false],
        )
        .await;
        let account: stability_pool_common::Account = serde_json::from_value(serde_json::json!({
            "acc_type": "BtcDepositor",
            "pub_keys": ["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],
            "threshold": 1
        }))
        .unwrap();

        let error = wallet
            .deposit_to_btc_balance(
                FederationId::dummy(),
                account.id(),
                Amount::from_msats(1_000),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("locked-payment value is reserved"),
            "{error:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whole_release_tombstone_survives_a_lost_fi_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let reservation_id = reservation_id("reopen-released-aggregate");
        let wallet = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        seed_reservation(
            &wallet,
            &reservation_id,
            vec![[1; 32], [2; 32]],
            vec![false, false],
        )
        .await;
        wallet
            .release_locked_payment_reservation(LockedPaymentReservation {
                key: payment_reservation_key(&reservation_id),
                reservation_id: reservation_id.clone(),
            })
            .await
            .unwrap();
        drop(wallet);

        let reopened = Wallet::open(temp.path().to_owned(), &crate::WalletSecret([42; 64]))
            .await
            .unwrap();
        let reconstructed = reopened
            .reserve_locked_payments(
                FederationId::dummy(),
                &reservation_id,
                &[reservation_preflight(1), reservation_preflight(2)],
            )
            .await
            .expect("released tombstone reconstructs without a joined or funded client");
        reopened
            .release_locked_payment_reservation(reconstructed)
            .await
            .expect("release replay is idempotent after lost FI checkpoint");
        assert!(
            load_reservation(&reopened, &reservation_id)
                .await
                .unwrap()
                .members
                .iter()
                .all(|member| member.state == StoredLockedPaymentMemberState::Released)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_member_starts_preserve_every_flag() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("concurrent-starts");
        seed_reservation(
            &wallet,
            &reservation_id,
            vec![[1; 32], [2; 32]],
            vec![false, false],
        )
        .await;
        let reservation = LockedPaymentReservation {
            key: payment_reservation_key(&reservation_id),
            reservation_id: reservation_id.clone(),
        };

        let (first, second) = tokio::join!(
            wallet.start_reserved_locked_payment(&reservation, QuoteId([1; 32])),
            wallet.start_reserved_locked_payment(&reservation, QuoteId([2; 32])),
        );
        first.unwrap();
        second.unwrap();
        assert!(
            load_reservation(&wallet, &reservation_id)
                .await
                .unwrap()
                .members
                .iter()
                .all(|member| member.state == StoredLockedPaymentMemberState::Started)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prepared_or_ambiguous_member_cannot_be_released_without_wallet_terminal_proof() {
        let (_temp, wallet) = test_wallet().await;
        let reservation_id = reservation_id("terminal-proof");
        seed_reservation(
            &wallet,
            &reservation_id,
            vec![[1; 32], [2; 32]],
            vec![true, true],
        )
        .await;

        let forged = LockedPaymentTerminalRelease {
            key: payment_reservation_key(&reservation_id),
            quote_id: QuoteId([1; 32]),
            plan_hash: [0; 32],
            debit_msats: 1,
        };
        assert!(wallet.release_locked_payment_member(forged).await.is_err());
        assert!(
            wallet
                .release_locked_payment_reservation(LockedPaymentReservation {
                    key: payment_reservation_key(&reservation_id),
                    reservation_id: reservation_id.clone(),
                })
                .await
                .is_err(),
            "a started aggregate cannot be released wholesale",
        );
        assert!(
            load_reservation(&wallet, &reservation_id)
                .await
                .unwrap()
                .members
                .iter()
                .all(|member| member.state == StoredLockedPaymentMemberState::Started)
        );

        let proof = wallet
            .record_locked_payment_terminal(&reservation_id, QuoteId([1; 32]))
            .await
            .unwrap();
        wallet.release_locked_payment_member(proof).await.unwrap();
        let stored = load_reservation(&wallet, &reservation_id).await.unwrap();
        assert_eq!(
            stored.members[0].state,
            StoredLockedPaymentMemberState::Released
        );
        assert_eq!(
            stored.members[1].state,
            StoredLockedPaymentMemberState::Started
        );
        assert!(
            wallet
                .release_locked_payment_reservation(LockedPaymentReservation {
                    key: payment_reservation_key(&reservation_id),
                    reservation_id: reservation_id.clone(),
                })
                .await
                .is_err(),
            "a terminal sibling cannot turn a post-output journal into a whole release",
        );

        // Replaying terminal observation and release is idempotent.
        let proof = wallet
            .record_locked_payment_terminal(&reservation_id, QuoteId([1; 32]))
            .await
            .unwrap();
        wallet.release_locked_payment_member(proof).await.unwrap();
    }

    #[test]
    fn exact_preflight_cost_includes_every_foreign_output_fee() {
        let first = exact_foreign_output_cost([
            Ok((Amount::from_msats(100), Amount::from_msats(3))),
            Ok((Amount::from_msats(50), Amount::from_msats(2))),
        ])
        .unwrap();
        let second =
            exact_foreign_output_cost([Ok((Amount::from_msats(25), Amount::from_msats(4)))])
                .unwrap();

        assert_eq!(first, Amount::from_msats(155));
        assert_eq!(second, Amount::from_msats(29));
        assert_eq!(first + second, Amount::from_msats(184));
    }

    fn transaction_updates(
        operation_id: OperationId,
        state: TxSubmissionStates,
    ) -> TransactionUpdates {
        transaction_update_history(operation_id, [state])
    }

    fn transaction_update_history(
        operation_id: OperationId,
        states: impl IntoIterator<Item = TxSubmissionStates>,
    ) -> TransactionUpdates {
        let states = states.into_iter().collect::<Vec<_>>();
        TransactionUpdates {
            update_stream: futures::stream::iter(states.into_iter().map(move |state| {
                TxSubmissionStatesSM {
                    operation_id,
                    state,
                }
            }))
            .boxed(),
        }
    }

    fn refund_test_input(mint_module: ModuleInstanceId, byte: u8) -> fedimint_core::core::DynInput {
        let secret = fedimint_core::secp256k1::SecretKey::from_slice(&[byte; 32]).unwrap();
        let note = Note {
            nonce: Nonce(fedimint_core::secp256k1::PublicKey::from_secret_key(
                fedimint_core::secp256k1::SECP256K1,
                &secret,
            )),
            signature: tbs::Signature(bls12_381::G1Affine::generator()),
        };
        MintInput::new_v0(Amount::from_msats(1_024), note).into_dyn(mint_module)
    }

    fn refund_test_transaction(
        mint_module: ModuleInstanceId,
        inputs: Vec<fedimint_core::core::DynInput>,
        output_amounts: &[u64],
        nonce: u8,
    ) -> Transaction {
        let blind_nonce = BlindNonce(tbs::BlindedMessage(bls12_381::G1Affine::generator()));
        Transaction {
            inputs,
            outputs: output_amounts
                .iter()
                .map(|amount| {
                    MintOutput::new_v0(Amount::from_msats(*amount), blind_nonce)
                        .into_dyn(mint_module)
                })
                .collect(),
            nonce: [nonce; 8],
            signatures: TransactionSignature::NaiveMultisig(Vec::new()),
        }
    }

    #[tokio::test]
    async fn created_transaction_recovery_ignores_refund_replay_order() {
        let operation_id = OperationId([20; 32]);
        let mint_module = ModuleInstanceId::from(7u16);
        let input = refund_test_input(mint_module, 1);
        let original = refund_test_transaction(mint_module, vec![input.clone()], &[1_000, 20], 1);
        let refund = refund_test_transaction(mint_module, vec![input], &[1_020], 2);
        let history = || {
            transaction_update_history(
                operation_id,
                [
                    TxSubmissionStates::Created(refund.clone()),
                    TxSubmissionStates::Accepted(refund.tx_hash()),
                    TxSubmissionStates::Created(original.clone()),
                    TxSubmissionStates::Rejected(original.tx_hash(), "rejected".to_owned()),
                ],
            )
        };

        let recovered = locked_payment_created_transaction_from_updates(
            operation_id,
            history(),
            |transaction| transaction.tx_hash() == original.tx_hash(),
        )
        .await
        .unwrap();
        assert_eq!(recovered, original);
    }

    #[tokio::test]
    async fn rejected_input_refund_replays_unordered_duplicate_history_after_reopen() {
        let operation_id = OperationId([21; 32]);
        let mint_module = ModuleInstanceId::from(7u16);
        let input = refund_test_input(mint_module, 1);
        let original = refund_test_transaction(mint_module, vec![input.clone()], &[1_000], 1);
        let original_txid = original.tx_hash();
        let refund = refund_test_transaction(mint_module, vec![input], &[1_020], 2);
        let refund_txid = refund.tx_hash();
        let history = || {
            transaction_update_history(
                operation_id,
                [
                    TxSubmissionStates::Accepted(refund_txid),
                    TxSubmissionStates::Rejected(original_txid, "rejected".to_owned()),
                    TxSubmissionStates::Created(refund.clone()),
                    TxSubmissionStates::Created(refund.clone()),
                    TxSubmissionStates::Accepted(refund_txid),
                ],
            )
        };

        for _reopen in 0..2 {
            let awaited = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
            let observed = awaited.clone();
            await_rejected_input_refunds(
                operation_id,
                original.clone(),
                mint_module,
                history(),
                move |outpoints| {
                    let observed = observed.clone();
                    async move {
                        observed.lock().await.push(outpoints);
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();
            assert_eq!(
                awaited.lock().await.as_slice(),
                &[vec![OutPoint {
                    txid: refund_txid,
                    out_idx: 0,
                }]]
            );
        }
    }

    #[tokio::test]
    async fn rejected_v1_bundle_refund_waits_for_accepted_per_note_fallback_union() {
        let operation_id = OperationId([22; 32]);
        let mint_module = ModuleInstanceId::from(7u16);
        let first = refund_test_input(mint_module, 1);
        let second = refund_test_input(mint_module, 2);
        let original = refund_test_transaction(
            mint_module,
            vec![first.clone(), second.clone()],
            &[2_000],
            3,
        );
        let bundle = refund_test_transaction(
            mint_module,
            vec![first.clone(), second.clone()],
            &[2_040],
            4,
        );
        let first_refund = refund_test_transaction(mint_module, vec![first], &[1_020], 5);
        let second_refund = refund_test_transaction(mint_module, vec![second], &[1_020], 6);
        let first_txid = first_refund.tx_hash();
        let second_txid = second_refund.tx_hash();
        let updates = transaction_update_history(
            operation_id,
            [
                TxSubmissionStates::Created(bundle.clone()),
                TxSubmissionStates::Rejected(bundle.tx_hash(), "bundle rejected".to_owned()),
                TxSubmissionStates::Accepted(first_txid),
                TxSubmissionStates::Created(second_refund),
                TxSubmissionStates::Created(first_refund),
                TxSubmissionStates::Accepted(second_txid),
            ],
        );
        let awaited = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let observed = awaited.clone();

        await_rejected_input_refunds(
            operation_id,
            original,
            mint_module,
            updates,
            move |outpoints| {
                let observed = observed.clone();
                async move {
                    observed.lock().await.push(outpoints);
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        let mut outpoints = awaited.lock().await[0].clone();
        outpoints.sort_by_key(|outpoint| outpoint.txid);
        let mut expected = vec![
            OutPoint {
                txid: first_txid,
                out_idx: 0,
            },
            OutPoint {
                txid: second_txid,
                out_idx: 0,
            },
        ];
        expected.sort_by_key(|outpoint| outpoint.txid);
        assert_eq!(outpoints, expected);
    }

    #[tokio::test]
    async fn partial_or_overlapping_rejected_input_refunds_fail_closed() {
        let operation_id = OperationId([23; 32]);
        let mint_module = ModuleInstanceId::from(7u16);
        let first = refund_test_input(mint_module, 1);
        let second = refund_test_input(mint_module, 2);
        let original =
            refund_test_transaction(mint_module, vec![first.clone(), second], &[2_000], 7);
        let duplicate_a = refund_test_transaction(mint_module, vec![first.clone()], &[1_020], 8);
        let duplicate_b = refund_test_transaction(mint_module, vec![first], &[1_020], 9);
        let updates = transaction_update_history(
            operation_id,
            [
                TxSubmissionStates::Created(duplicate_a.clone()),
                TxSubmissionStates::Accepted(duplicate_a.tx_hash()),
                TxSubmissionStates::Created(duplicate_b.clone()),
                TxSubmissionStates::Accepted(duplicate_b.tx_hash()),
            ],
        );

        let error =
            await_rejected_input_refunds(operation_id, original, mint_module, updates, |_| async {
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("overlap"), "{error:#}");
    }

    #[tokio::test]
    async fn rejected_input_refund_output_failure_remains_retryable() {
        let operation_id = OperationId([24; 32]);
        let mint_module = ModuleInstanceId::from(7u16);
        let input = refund_test_input(mint_module, 1);
        let original = refund_test_transaction(mint_module, vec![input.clone()], &[1_000], 10);
        let refund = refund_test_transaction(mint_module, vec![input], &[1_020], 11);
        let updates = transaction_update_history(
            operation_id,
            [
                TxSubmissionStates::Created(refund.clone()),
                TxSubmissionStates::Accepted(refund.tx_hash()),
            ],
        );

        let error =
            await_rejected_input_refunds(operation_id, original, mint_module, updates, |_| async {
                anyhow::bail!("outputs not spendable")
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not spendable"), "{error:#}");
    }

    #[tokio::test]
    async fn rejected_mint_v1_payment_is_a_terminal_recovery_outcome() {
        let operation_id = OperationId([1; 32]);
        let txid = TransactionId::from_byte_array([2; 32]);
        let status = locked_payment_transaction_status(
            transaction_updates(
                operation_id,
                TxSubmissionStates::Rejected(txid, "mint-v1 rejected".to_owned()),
            ),
            txid,
        )
        .await;

        assert!(matches!(
            status,
            LockedPaymentTransactionStatus::Rejected(reason)
                if reason == "mint-v1 rejected"
        ));
    }

    #[tokio::test]
    async fn rejected_mint_v2_payment_is_observed_before_output_collection() {
        let operation_id = OperationId([3; 32]);
        let txid = TransactionId::from_byte_array([4; 32]);
        let status = locked_payment_transaction_status(
            transaction_updates(
                operation_id,
                TxSubmissionStates::Rejected(txid, "mint-v2 rejected".to_owned()),
            ),
            txid,
        )
        .await;

        assert!(matches!(
            status,
            LockedPaymentTransactionStatus::Rejected(reason)
                if reason == "mint-v2 rejected"
        ));
    }

    #[test]
    fn refund_outputs_must_match_prepared_issuance_in_order() {
        let mint_module = ModuleInstanceId::from(7u16);
        let blind_nonce = BlindNonce(tbs::BlindedMessage(bls12_381::G1Affine::generator()));
        let issuance = vec![
            locked_payment::IssuanceRequest {
                amount: Amount::from_msats(1_000),
                blind_nonce,
            },
            locked_payment::IssuanceRequest {
                amount: Amount::from_msats(2_000),
                blind_nonce,
            },
        ];
        let outputs = issuance
            .iter()
            .map(|request| {
                MintOutput::new_v0(request.amount, request.blind_nonce).into_dyn(mint_module)
            })
            .collect::<Vec<_>>();
        validate_refund_outputs(&outputs, mint_module, &issuance).unwrap();

        let mut reordered = outputs.clone();
        reordered.swap(0, 1);
        assert!(validate_refund_outputs(&reordered, mint_module, &issuance).is_err());
    }

    #[test]
    fn locked_payment_metadata_recovers_exact_payment_and_change_ranges() {
        let mint_module = ModuleInstanceId::from(7u16);
        let range = OutPointRange::new(
            TransactionId::from_byte_array([3; 32]),
            IdxRange::from(0..2),
        );
        let change_range = OutPointRange::new(range.txid(), IdxRange::from(2..3));
        let binding_hash = [4; 32];
        let metadata = LockedPaymentOperationMeta::new(
            LockedPaymentGeneration::MintV2,
            mint_module,
            binding_hash,
            range,
            change_range,
        );
        let mut encoded = serde_json::to_value(&metadata).unwrap();
        let decoded: LockedPaymentOperationMeta = serde_json::from_value(encoded.clone()).unwrap();

        assert_eq!(
            decoded
                .clone()
                .validate(
                    LockedPaymentGeneration::MintV2,
                    mint_module,
                    binding_hash,
                    2
                )
                .unwrap(),
            LockedPaymentRanges {
                outputs: range,
                change: change_range,
            }
        );
        encoded
            .as_object_mut()
            .unwrap()
            .remove("change_range")
            .unwrap();
        assert!(
            serde_json::from_value::<LockedPaymentOperationMeta>(encoded).is_err(),
            "change range is required for exact same-schema recovery"
        );
        assert!(
            decoded
                .clone()
                .validate(
                    LockedPaymentGeneration::MintV1,
                    mint_module,
                    binding_hash,
                    2
                )
                .is_err()
        );
        assert!(
            decoded
                .clone()
                .validate(
                    LockedPaymentGeneration::MintV2,
                    ModuleInstanceId::from(8u16),
                    binding_hash,
                    2
                )
                .is_err()
        );
        assert!(
            decoded
                .clone()
                .validate(
                    LockedPaymentGeneration::MintV2,
                    mint_module,
                    binding_hash,
                    3
                )
                .is_err()
        );
        assert!(
            decoded
                .validate(LockedPaymentGeneration::MintV2, mint_module, [5; 32], 2)
                .is_err()
        );
    }

    #[test]
    fn locked_payment_operation_identity_is_exactly_bound() {
        let mint_module = ModuleInstanceId::from(7u16);
        let issuance = b"canonical issuance";
        let binding = sha256::Hash::hash(b"quote-id").to_byte_array();
        let operation_id = locked_payment_operation_id(
            LockedPaymentGeneration::MintV2,
            mint_module,
            binding,
            issuance,
        );

        assert_eq!(
            operation_id,
            locked_payment_operation_id(
                LockedPaymentGeneration::MintV2,
                mint_module,
                binding,
                issuance,
            )
        );
        assert_ne!(
            operation_id,
            locked_payment_operation_id(
                LockedPaymentGeneration::MintV2,
                mint_module,
                sha256::Hash::hash(b"other-quote").to_byte_array(),
                issuance,
            )
        );
        assert_ne!(
            operation_id,
            locked_payment_operation_id(
                LockedPaymentGeneration::MintV1,
                mint_module,
                binding,
                issuance,
            )
        );
        assert_ne!(
            operation_id,
            locked_payment_operation_id(
                LockedPaymentGeneration::MintV2,
                ModuleInstanceId::from(8u16),
                binding,
                issuance,
            )
        );
        assert_ne!(
            operation_id,
            locked_payment_operation_id(
                LockedPaymentGeneration::MintV2,
                mint_module,
                binding,
                b"other issuance",
            )
        );
    }
}
