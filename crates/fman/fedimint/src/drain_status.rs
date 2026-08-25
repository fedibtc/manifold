//! Read-only, fail-closed wallet drain projection.

use crate::payout_job::Payout;
use crate::payout_job::PayoutRequestId;
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::{
    OutgoingOperation, OutgoingRail, OutgoingState, WalletDrainQuery, WalletDrainStatus,
};
use anyhow::Context as _;
use fedimint_client::ClientHandleArc;
use fedimint_client::db::{
    ChronologicalOperationLogKey, ChronologicalOperationLogKeyPrefix, OperationLogKey,
};
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::db::IDatabaseTransactionOpsCoreTyped as _;
use fedimint_ln_client::{
    InternalPayState, LightningClientModule, LightningOperationMeta as LightningV1OperationMeta,
    LightningOperationMetaVariant, LnPayState,
};
use fedimint_ln_common::config::FeeToAmount as _;
use fedimint_lnv2_client::{
    LightningClientModule as LightningV2ClientModule,
    LightningOperationMeta as LightningV2OperationMeta, SendOperationState,
};
use fman_core::wallet::Msats;
use futures::StreamExt as _;

#[async_trait::async_trait]
trait OperationSource: Sync {
    async fn all(
        &self,
    ) -> anyhow::Result<
        Vec<(
            ChronologicalOperationLogKey,
            fedimint_client_module::oplog::OperationLogEntry,
        )>,
    >;
}

struct ClientOperationSource<'a>(&'a ClientHandleArc);

#[async_trait::async_trait]
impl OperationSource for ClientOperationSource<'_> {
    async fn all(
        &self,
    ) -> anyhow::Result<
        Vec<(
            ChronologicalOperationLogKey,
            fedimint_client_module::oplog::OperationLogEntry,
        )>,
    > {
        let mut dbtx = self.0.db().begin_transaction_nc().await;
        let keys = dbtx
            .find_by_prefix(&ChronologicalOperationLogKeyPrefix)
            .await
            .map(|(key, ())| key)
            .collect::<Vec<_>>()
            .await;
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            let entry = dbtx
                .get_value(&OperationLogKey {
                    operation_id: key.operation_id,
                })
                .await
                .context("chronological operation key has no operation entry")?;
            entries.push((key, entry));
        }
        Ok(entries)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WalletObservation {
    active_operations: std::collections::HashSet<OperationId>,
    available: Result<Msats, WalletDrainQuery>,
    outgoing: Result<Vec<OutgoingOperation>, WalletDrainQuery>,
}

/// Read all wallet facts needed to make a fail-closed destruction decision.
pub(crate) async fn wallet_drain_status(client: &ClientHandleArc) -> WalletDrainStatus {
    let before = observe_wallet(client).await;
    let economically_sweepable = match before.available {
        Ok(Msats(0)) => Ok(Msats(0)),
        Ok(amount) => economically_sweepable(client, Amount::from_msats(amount.0))
            .await
            .map(|amount| Msats(amount.msats))
            .map_err(|_| WalletDrainQuery::EconomicallySweepable),
        Err(_) => Err(WalletDrainQuery::EconomicallySweepable),
    };
    let after = observe_wallet(client).await;
    validated_status(before, economically_sweepable, after)
}

fn validated_status(
    before: WalletObservation,
    economically_sweepable: Result<Msats, WalletDrainQuery>,
    after: WalletObservation,
) -> WalletDrainStatus {
    if before != after {
        return WalletDrainStatus::unknown(WalletDrainQuery::InconsistentSnapshot);
    }
    WalletDrainStatus::new(
        before.available,
        economically_sweepable,
        before.outgoing,
        before.active_operations.len(),
    )
}

async fn observe_wallet(client: &ClientHandleArc) -> WalletObservation {
    let active_operations = client.get_active_operations().await;
    let available = client
        .get_balance_for_btc()
        .await
        .map(|amount| Msats(amount.msats))
        .map_err(|_| WalletDrainQuery::AvailableEcash);
    let outgoing = outgoing_operations(client, &active_operations)
        .await
        .map_err(|_| WalletDrainQuery::OutgoingOperations);
    WalletObservation {
        active_operations,
        available,
        outgoing,
    }
}

async fn economically_sweepable(
    client: &ClientHandleArc,
    balance: Amount,
) -> anyhow::Result<Amount> {
    if let Ok(lightning) = client.get_first_module::<LightningV2ClientModule>() {
        match lightning.select_gateway(None).await {
            Ok((gateway, _)) => {
                return lightning
                    .spendable_amount(balance, Some(gateway))
                    .await
                    .context("compute Lightning v2 sweepable amount");
            }
            Err(_) if client.get_first_module::<LightningClientModule>().is_ok() => {}
            Err(error) => return Err(error).context("select Lightning v2 gateway"),
        }
    }

    let lightning = client
        .get_first_module::<LightningClientModule>()
        .context("federation has no Lightning client module")?;
    let gateway = crate::select_v1_gateway(client, &lightning).await?;
    crate::payout_native::validate_v1_gateway_fees(balance, &gateway.fees)?;
    let minimum_recipient = Amount::from_msats(1);
    let minimum_contract = minimum_recipient + gateway.fees.to_amount(&minimum_recipient);
    if cannot_cover_minimum_contract(balance, minimum_contract) {
        return Ok(Amount::ZERO);
    }
    lightning
        .spendable_amount(balance, Some(gateway))
        .await
        .context("compute Lightning v1 sweepable amount")
}

fn cannot_cover_minimum_contract(balance: Amount, minimum_contract: Amount) -> bool {
    balance < minimum_contract
}

async fn outgoing_operations(
    client: &ClientHandleArc,
    active_operations: &std::collections::HashSet<OperationId>,
) -> anyhow::Result<Vec<OutgoingOperation>> {
    outgoing_operations_from(&ClientOperationSource(client), active_operations).await
}

async fn outgoing_operations_from(
    source: &dyn OperationSource,
    active_operations: &std::collections::HashSet<OperationId>,
) -> anyhow::Result<Vec<OutgoingOperation>> {
    let mut outgoing = Vec::new();
    for (key, entry) in source.all().await? {
        let active = active_operations.contains(&key.operation_id);
        if let Some(operation) = outgoing_operation(key.operation_id, &entry, active)? {
            outgoing.push(operation);
        }
    }

    Ok(outgoing)
}

/// Read one exact FMan payout from durable operation metadata and cached state.
pub(crate) async fn payout_status(
    client: &ClientHandleArc,
    operation_id: OperationId,
    request_id: Option<&PayoutRequestId>,
    destination: Option<&str>,
) -> anyhow::Result<OutgoingOperation> {
    let active = client.get_active_operations().await.contains(&operation_id);
    let mut dbtx = client.db().begin_transaction_nc().await;
    let entry = dbtx
        .get_value(&OperationLogKey { operation_id })
        .await
        .context("native payout operation does not exist")?;
    let outgoing = outgoing_operation(operation_id, &entry, active)?
        .context("native operation is not an FMan payout")?;
    if let (Some(request_id), Some(destination)) = (request_id, destination) {
        anyhow::ensure!(
            payout_request_amount(&entry, request_id, destination)?.is_some(),
            "native payout operation belongs to another request or destination"
        );
    }
    Ok(outgoing)
}

/// Read the exact returned operation's v1 payout binding without scanning
/// active state machines or any other operation.
pub(crate) async fn v1_payout_request_amount(
    client: &ClientHandleArc,
    operation_id: OperationId,
    request_id: &PayoutRequestId,
    destination: &str,
) -> anyhow::Result<u64> {
    let mut dbtx = client.db().begin_transaction_nc().await;
    let entry = dbtx
        .get_value(&OperationLogKey { operation_id })
        .await
        .context("returned Lightning v1 payout operation does not exist")?;
    anyhow::ensure!(
        entry.operation_module_kind() == "ln",
        "returned Lightning v1 payout operation belongs to another rail"
    );
    payout_request_amount(&entry, request_id, destination)?
        .context("returned Lightning v1 payout operation belongs to another request")
}

/// Find the unique native payout carrying the caller's request metadata.
pub(crate) async fn payout_for_request(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
) -> anyhow::Result<Option<Payout>> {
    let mut found = None;
    for (key, entry) in ClientOperationSource(client).all().await? {
        let Some(amount_msat) = payout_request_amount(&entry, request_id, destination)? else {
            continue;
        };
        anyhow::ensure!(
            found.is_none(),
            "multiple native payouts carry the same FMan request id"
        );
        found = Some(Payout {
            operation_id: PayoutOperationId::parse(&key.operation_id.fmt_full().to_string())
                .expect("Fedimint formats a canonical operation id"),
            amount_msat,
        });
    }
    Ok(found)
}

fn payout_request_amount(
    entry: &fedimint_client_module::oplog::OperationLogEntry,
    request_id: &PayoutRequestId,
    destination: &str,
) -> anyhow::Result<Option<u64>> {
    Ok(match entry.operation_module_kind() {
        "ln" => {
            let metadata = entry
                .try_meta::<LightningV1OperationMeta>()
                .context("decode Lightning v1 operation metadata")?;
            let LightningOperationMetaVariant::Pay(payment) = metadata.variant else {
                return Ok(None);
            };
            if !has_request_id(&metadata.extra_meta, request_id) {
                return Ok(None);
            }
            ensure_destination_binding(&metadata.extra_meta, destination)?;
            Some(
                payment
                    .invoice
                    .amount_milli_satoshis()
                    .context("Lightning v1 payout invoice has no amount")?,
            )
        }
        "lnv2" => {
            let metadata = entry
                .try_meta::<LightningV2OperationMeta>()
                .context("decode Lightning v2 operation metadata")?;
            let LightningV2OperationMeta::Send(payment) = metadata else {
                return Ok(None);
            };
            if !has_request_id(&payment.custom_meta, request_id) {
                return Ok(None);
            }
            ensure_destination_binding(&payment.custom_meta, destination)?;
            Some(match &payment.invoice {
                fedimint_lnv2_common::LightningInvoice::Bolt11(invoice) => invoice
                    .amount_milli_satoshis()
                    .context("Lightning v2 payout invoice has no amount")?,
            })
        }
        _ => None,
    })
}

fn outgoing_operation(
    operation_id: OperationId,
    entry: &fedimint_client_module::oplog::OperationLogEntry,
    active: bool,
) -> anyhow::Result<Option<OutgoingOperation>> {
    Ok(match entry.operation_module_kind() {
        "ln" => {
            let metadata = entry
                .try_meta::<LightningV1OperationMeta>()
                .context("decode Lightning v1 operation metadata")?;
            let LightningOperationMetaVariant::Pay(payment) = metadata.variant else {
                return Ok(None);
            };
            if !is_fman_payout(&metadata.extra_meta) {
                return Ok(None);
            }
            let state = v1_state(entry, payment.is_internal_payment, active)?;
            let recipient_amount_msat = payment
                .invoice
                .amount_milli_satoshis()
                .context("Lightning v1 payout invoice has no amount")?;
            let contract_amount_msat = recipient_amount_msat.saturating_add(payment.fee.msats);
            Some(operation(
                operation_id,
                OutgoingRail::Lnv1,
                state,
                recipient_amount_msat,
                contract_amount_msat,
                active,
            ))
        }
        "lnv2" => {
            let metadata = entry
                .try_meta::<LightningV2OperationMeta>()
                .context("decode Lightning v2 operation metadata")?;
            let LightningV2OperationMeta::Send(payment) = metadata else {
                return Ok(None);
            };
            if !is_fman_payout(&payment.custom_meta) {
                return Ok(None);
            }
            let state = v2_state(entry, active)?;
            let recipient_amount_msat = match &payment.invoice {
                fedimint_lnv2_common::LightningInvoice::Bolt11(invoice) => invoice
                    .amount_milli_satoshis()
                    .context("Lightning v2 payout invoice has no amount")?,
            };
            Some(operation(
                operation_id,
                OutgoingRail::Lnv2,
                state,
                recipient_amount_msat,
                payment.contract.amount.msats,
                active,
            ))
        }
        _ => None,
    })
}

fn is_fman_payout(metadata: &serde_json::Value) -> bool {
    metadata.get("purpose").and_then(serde_json::Value::as_str) == Some("fman-payout")
}

fn has_request_id(metadata: &serde_json::Value, request_id: &PayoutRequestId) -> bool {
    is_fman_payout(metadata)
        && metadata
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            == Some(request_id.as_str())
}

fn ensure_destination_binding(
    metadata: &serde_json::Value,
    destination: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        metadata
            .get("destination")
            .and_then(serde_json::Value::as_str)
            == Some(destination),
        "native payout request id is bound to another destination"
    );
    Ok(())
}

fn v1_state(
    entry: &fedimint_client_module::oplog::OperationLogEntry,
    internal: bool,
    active: bool,
) -> anyhow::Result<OutgoingState> {
    if internal {
        let outcome = entry
            .try_outcome::<InternalPayState>()
            .context("decode internal Lightning v1 operation outcome")?;
        Ok(match outcome {
            Some(InternalPayState::Preimage(_)) => OutgoingState::Succeeded,
            Some(
                InternalPayState::RefundSuccess { .. } | InternalPayState::FundingFailed { .. },
            ) => OutgoingState::FailedOrRefunded,
            Some(InternalPayState::RefundError { .. } | InternalPayState::UnexpectedError(_)) => {
                OutgoingState::Unknown
            }
            Some(InternalPayState::Funding) | None if active => OutgoingState::Pending,
            Some(InternalPayState::Funding) | None => OutgoingState::Unknown,
        })
    } else {
        let outcome = entry
            .try_outcome::<LnPayState>()
            .context("decode external Lightning v1 operation outcome")?;
        Ok(match outcome {
            Some(LnPayState::Success { .. }) => OutgoingState::Succeeded,
            Some(LnPayState::Canceled | LnPayState::Refunded { .. }) => {
                OutgoingState::FailedOrRefunded
            }
            Some(LnPayState::UnexpectedError { .. }) => OutgoingState::Unknown,
            Some(
                LnPayState::Created
                | LnPayState::Funded { .. }
                | LnPayState::WaitingForRefund { .. }
                | LnPayState::AwaitingChange,
            )
            | None
                if active =>
            {
                OutgoingState::Pending
            }
            Some(
                LnPayState::Created
                | LnPayState::Funded { .. }
                | LnPayState::WaitingForRefund { .. }
                | LnPayState::AwaitingChange,
            )
            | None => OutgoingState::Unknown,
        })
    }
}

fn v2_state(
    entry: &fedimint_client_module::oplog::OperationLogEntry,
    active: bool,
) -> anyhow::Result<OutgoingState> {
    let outcome = entry
        .try_outcome::<SendOperationState>()
        .context("decode Lightning v2 operation outcome")?;
    Ok(match outcome {
        Some(SendOperationState::Success(_)) => OutgoingState::Succeeded,
        Some(SendOperationState::Refunded) => OutgoingState::FailedOrRefunded,
        Some(SendOperationState::Failure) => OutgoingState::Unknown,
        Some(
            SendOperationState::Funding
            | SendOperationState::Funded
            | SendOperationState::Refunding,
        )
        | None
            if active =>
        {
            OutgoingState::Pending
        }
        Some(
            SendOperationState::Funding
            | SendOperationState::Funded
            | SendOperationState::Refunding,
        )
        | None => OutgoingState::Unknown,
    })
}

fn operation(
    operation_id: OperationId,
    rail: OutgoingRail,
    state: OutgoingState,
    recipient_amount_msat: u64,
    contract_amount_msat: u64,
    active: bool,
) -> OutgoingOperation {
    OutgoingOperation::new(
        PayoutOperationId::parse(&operation_id.fmt_full().to_string())
            .expect("Fedimint formats a canonical operation id"),
        rail,
        state,
        recipient_amount_msat,
        contract_amount_msat,
        active,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fedimint_client_module::oplog::{JsonStringed, OperationLogEntry, OperationOutcome};
    use fedimint_core::{OutPoint, TransactionId};
    use fedimint_ln_client::{LightningOperationMetaPay, LightningOperationMetaVariant};
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
    use std::time::SystemTime;

    struct Operations(
        std::sync::Mutex<Option<Vec<(ChronologicalOperationLogKey, OperationLogEntry)>>>,
    );

    #[async_trait::async_trait]
    impl OperationSource for Operations {
        async fn all(
            &self,
        ) -> anyhow::Result<Vec<(ChronologicalOperationLogKey, OperationLogEntry)>> {
            Ok(self.0.lock().unwrap().take().unwrap_or_default())
        }
    }

    fn id() -> OperationId {
        OperationId([7; 32])
    }

    fn entry_with_outcome(outcome: impl serde::Serialize) -> OperationLogEntry {
        let mut entry = OperationLogEntry::new(
            "test".to_owned(),
            JsonStringed(serde_json::Value::Null),
            None,
        );
        entry.set_outcome(OperationOutcome {
            time: SystemTime::UNIX_EPOCH,
            outcome: JsonStringed(serde_json::to_value(outcome).unwrap()),
        });
        entry
    }

    fn unrelated_entry(index: u8) -> (ChronologicalOperationLogKey, OperationLogEntry) {
        (
            ChronologicalOperationLogKey {
                creation_time: SystemTime::UNIX_EPOCH,
                operation_id: OperationId([index; 32]),
            },
            OperationLogEntry::new(
                "mint".to_owned(),
                JsonStringed(serde_json::Value::Null),
                None,
            ),
        )
    }

    fn invoice() -> lightning_invoice::Bolt11Invoice {
        use bitcoin::secp256k1::{SECP256K1, SecretKey};
        use bitcoin_hashes::{Hash as _, sha256};

        InvoiceBuilder::new(Currency::Regtest)
            .description(String::new())
            .payment_hash(sha256::Hash::hash(&[1; 32]))
            .current_timestamp()
            .min_final_cltv_expiry_delta(0)
            .payment_secret(PaymentSecret([2; 32]))
            .amount_milli_satoshis(900)
            .build_signed(|message| {
                SECP256K1.sign_ecdsa_recoverable(message, &SecretKey::from_slice(&[3; 32]).unwrap())
            })
            .unwrap()
    }

    #[test]
    fn funding_has_unknown_encumbrance_while_failed_refund_wait_is_known() {
        for state in [OutgoingState::Pending, OutgoingState::FailedOrRefunded] {
            let operation = operation(id(), OutgoingRail::Lnv1, state, 900, 1_000, true);
            assert_eq!(
                operation.encumbered_msat(),
                (state == OutgoingState::FailedOrRefunded).then_some(1_000)
            );
            assert!(operation.has_active_state_machines());
        }
    }

    #[test]
    fn v2_success_does_not_call_paid_value_encumbered_while_change_is_active() {
        let operation = operation(
            id(),
            OutgoingRail::Lnv2,
            OutgoingState::Succeeded,
            900,
            1_000,
            true,
        );
        assert_eq!(operation.encumbered_msat(), Some(0));
        assert!(operation.has_active_state_machines());
    }

    #[test]
    fn inactive_unknown_operation_remains_encumbered_after_restart() {
        let operation = operation(
            id(),
            OutgoingRail::Lnv1,
            OutgoingState::Unknown,
            900,
            1_000,
            false,
        );
        assert_eq!(operation.encumbered_msat(), None);
    }

    #[test]
    fn cached_external_v1_stream_states_decode_without_final_outcome_wrapper() {
        let success = entry_with_outcome(LnPayState::Success {
            preimage: "07".repeat(32),
        });
        let canceled = entry_with_outcome(LnPayState::Canceled);

        assert_eq!(
            v1_state(&success, false, false).unwrap(),
            OutgoingState::Succeeded
        );
        assert_eq!(
            v1_state(&canceled, false, true).unwrap(),
            OutgoingState::FailedOrRefunded
        );
        let error = entry_with_outcome(LnPayState::UnexpectedError {
            error_message: "refund output failed".to_owned(),
        });
        assert_eq!(
            v1_state(&error, false, false).unwrap(),
            OutgoingState::Unknown
        );
    }

    #[test]
    fn pinned_v1_payout_metadata_and_raw_outcome_decode_together() {
        use bitcoin_hashes::{Hash as _, sha256};

        let metadata = LightningV1OperationMeta {
            variant: LightningOperationMetaVariant::Pay(LightningOperationMetaPay {
                out_point: OutPoint {
                    txid: TransactionId::from_byte_array([4; 32]),
                    out_idx: 0,
                },
                invoice: invoice(),
                fee: Amount::from_msats(100),
                change: Vec::new(),
                is_internal_payment: false,
                contract_id: fedimint_ln_common::contracts::ContractId::from_raw_hash(
                    sha256::Hash::hash(&[5; 32]),
                ),
                gateway_id: None,
            }),
            extra_meta: serde_json::json!({
                "purpose": "fman-payout",
                "request_id": "caller-request",
                "destination": "operator@example.com",
            }),
        };
        let mut entry = OperationLogEntry::new(
            "ln".to_owned(),
            JsonStringed(serde_json::to_value(metadata).unwrap()),
            None,
        );
        entry.set_outcome(OperationOutcome {
            time: SystemTime::UNIX_EPOCH,
            outcome: JsonStringed(
                serde_json::to_value(LnPayState::WaitingForRefund {
                    error_reason: "test".to_owned(),
                })
                .unwrap(),
            ),
        });
        let decoded = entry.try_meta::<LightningV1OperationMeta>().unwrap();

        assert!(is_fman_payout(&decoded.extra_meta));
        assert_eq!(
            payout_request_amount(
                &entry,
                &PayoutRequestId::parse("caller-request").unwrap(),
                "operator@example.com",
            )
            .unwrap(),
            Some(900)
        );
        assert_eq!(
            payout_request_amount(
                &entry,
                &PayoutRequestId::parse("other").unwrap(),
                "operator@example.com",
            )
            .unwrap(),
            None
        );
        assert!(
            payout_request_amount(
                &entry,
                &PayoutRequestId::parse("caller-request").unwrap(),
                "retargeted@example.com",
            )
            .unwrap_err()
            .to_string()
            .contains("another destination")
        );
        assert!(matches!(
            decoded.variant,
            LightningOperationMetaVariant::Pay(_)
        ));
        assert_eq!(
            v1_state(&entry, false, true).unwrap(),
            OutgoingState::Pending
        );
    }

    #[test]
    fn pinned_v2_payout_metadata_binds_request_and_destination() {
        use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey};
        use bitcoin_hashes::{Hash as _, sha256};
        use fedimint_core::OutPointRange;
        use fedimint_lnv2_client::SendOperationMeta;
        use fedimint_lnv2_common::LightningInvoice;
        use fedimint_lnv2_common::contracts::{OutgoingContract, PaymentImage};

        let public_key =
            PublicKey::from_secret_key(SECP256K1, &SecretKey::from_slice(&[9; 32]).unwrap());
        let metadata = LightningV2OperationMeta::Send(SendOperationMeta {
            change_outpoint_range: OutPointRange::new_single(
                TransactionId::from_byte_array([8; 32]),
                0,
            )
            .unwrap(),
            gateway: "https://gateway.example.com".parse().unwrap(),
            contract: OutgoingContract {
                payment_image: PaymentImage::Hash(sha256::Hash::hash(&[7; 32])),
                amount: Amount::from_msats(1_000),
                expiration: 100,
                claim_pk: public_key,
                refund_pk: public_key,
                ephemeral_pk: public_key,
            },
            invoice: LightningInvoice::Bolt11(invoice()),
            custom_meta: serde_json::json!({
                "purpose": "fman-payout",
                "request_id": "caller-request",
                "destination": "operator@example.com",
            }),
        });
        let entry = OperationLogEntry::new(
            "lnv2".to_owned(),
            JsonStringed(serde_json::to_value(metadata).unwrap()),
            None,
        );

        assert_eq!(
            payout_request_amount(
                &entry,
                &PayoutRequestId::parse("caller-request").unwrap(),
                "operator@example.com",
            )
            .unwrap(),
            Some(900)
        );
        assert!(
            payout_request_amount(
                &entry,
                &PayoutRequestId::parse("caller-request").unwrap(),
                "retargeted@example.com",
            )
            .unwrap_err()
            .to_string()
            .contains("another destination")
        );
    }

    #[test]
    fn cached_internal_v1_stream_states_decode_without_external_shape() {
        let success = entry_with_outcome(InternalPayState::Preimage(
            fedimint_ln_common::contracts::Preimage([7; 32]),
        ));
        let funding = entry_with_outcome(InternalPayState::Funding);

        assert_eq!(
            v1_state(&success, true, false).unwrap(),
            OutgoingState::Succeeded
        );
        assert_eq!(
            v1_state(&funding, true, true).unwrap(),
            OutgoingState::Pending
        );
        let error = entry_with_outcome(InternalPayState::UnexpectedError(
            "refund output failed".to_owned(),
        ));
        assert_eq!(
            v1_state(&error, true, false).unwrap(),
            OutgoingState::Unknown
        );
    }

    #[test]
    fn cached_v2_stream_states_preserve_terminal_and_refunding_meanings() {
        let success = entry_with_outcome(SendOperationState::Success([7; 32]));
        let refunding = entry_with_outcome(SendOperationState::Refunding);
        let refunded = entry_with_outcome(SendOperationState::Refunded);
        let failure = entry_with_outcome(SendOperationState::Failure);

        assert_eq!(v2_state(&success, true).unwrap(), OutgoingState::Succeeded);
        assert_eq!(v2_state(&refunding, true).unwrap(), OutgoingState::Pending);
        assert_eq!(
            v2_state(&refunded, false).unwrap(),
            OutgoingState::FailedOrRefunded
        );
        assert_eq!(v2_state(&failure, false).unwrap(), OutgoingState::Unknown);
    }

    #[test]
    fn observation_change_fails_closed_instead_of_combining_snapshots() {
        let before = WalletObservation {
            active_operations: std::collections::HashSet::new(),
            available: Ok(Msats(0)),
            outgoing: Ok(Vec::new()),
        };
        let mut after = before.clone();
        after.available = Ok(Msats(1_000));

        let status = validated_status(before, Ok(Msats(0)), after);

        assert_eq!(
            status.query_errors,
            vec![WalletDrainQuery::InconsistentSnapshot]
        );
        assert_eq!(status.drain_state, crate::wallet_drain::DrainState::Unknown);
    }

    #[test]
    fn malformed_cached_state_is_a_query_error() {
        let malformed = entry_with_outcome(serde_json::json!({"success": "not-a-preimage"}));
        assert!(v2_state(&malformed, false).is_err());
    }

    #[test]
    fn only_the_exact_payout_marker_is_selected() {
        assert!(is_fman_payout(
            &serde_json::json!({"purpose": "fman-payout"})
        ));
        assert!(!is_fman_payout(
            &serde_json::json!({"purpose": "another-operation"})
        ));
        assert!(!is_fman_payout(&serde_json::Value::Null));
    }

    #[test]
    fn dust_below_the_gateway_minimum_is_known_uneconomical() {
        assert!(cannot_cover_minimum_contract(
            Amount::from_msats(350),
            Amount::from_msats(1_001),
        ));
        assert!(!cannot_cover_minimum_contract(
            Amount::from_msats(1_001),
            Amount::from_msats(1_001),
        ));
    }

    #[tokio::test]
    async fn operation_scan_has_no_clock_bound_and_filters_other_rails() {
        let mut entries = (0..100)
            .map(|index| unrelated_entry(index as u8))
            .collect::<Vec<_>>();
        let mut future = unrelated_entry(200);
        future.0.creation_time = SystemTime::now() + std::time::Duration::from_secs(3_600);
        entries.push(future);
        let operations = Operations(std::sync::Mutex::new(Some(entries)));

        let outgoing = outgoing_operations_from(&operations, &Default::default())
            .await
            .unwrap();

        assert!(outgoing.is_empty());
        assert!(operations.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_lightning_metadata_fails_the_whole_scan_closed() {
        let malformed = (
            ChronologicalOperationLogKey {
                creation_time: SystemTime::UNIX_EPOCH,
                operation_id: id(),
            },
            OperationLogEntry::new("ln".to_owned(), JsonStringed(serde_json::Value::Null), None),
        );
        let operations = Operations(std::sync::Mutex::new(Some(vec![malformed])));

        assert!(
            outgoing_operations_from(&operations, &Default::default())
                .await
                .is_err()
        );
    }
}
