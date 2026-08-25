//! Durable request-bound payout start and reconciliation.

use std::str::FromStr as _;

use crate::payout_job::Payout;
use crate::payout_job::PayoutRequestId;
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::{OutgoingOperation, OutgoingRail};
use anyhow::Context as _;
use bitcoin::secp256k1::PublicKey;
use fedimint_client::ClientHandleArc;
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_ln_client::LightningClientModule;
use fedimint_ln_common::config::FeeToAmount as _;
use fedimint_lnv2_client::{LightningClientModule as LightningV2ClientModule, SelectGatewayError};
use lightning_invoice::RoutingFees;
use tracing::{info, warn};

use super::{drain_status, lnurl_pay, payout_observer, select_v1_gateway};

/// Reject a v1 invoice whose payment hash already names a completed operation.
pub(crate) async fn start_fresh_v1_payment<T, F, Fut>(
    has_completed_payment: bool,
    start: F,
) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    anyhow::ensure!(
        !has_completed_payment,
        "LNURL endpoint reused an already-paid Lightning invoice"
    );
    start().await
}

/// Require the exact operation returned by the v1 start to carry this payout's
/// invoice amount after its rail and request metadata have been validated.
fn validate_committed_v1_payout_amount(
    committed_amount: anyhow::Result<u64>,
    amount_msat: u64,
) -> anyhow::Result<()> {
    let committed_amount =
        committed_amount.context("validate committed Lightning v1 payout metadata")?;
    anyhow::ensure!(
        committed_amount == amount_msat,
        "committed Lightning v1 payout amount differs from requested amount"
    );
    Ok(())
}

/// Start a payout and return only after Fedimint has committed its native
/// operation and state machines to the local wallet database.
pub(crate) async fn start_payout(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
) -> anyhow::Result<Payout> {
    let balance = client.get_balance_for_btc().await?;
    anyhow::ensure!(balance != Amount::ZERO, "no balance to sweep");

    let v1_lightning = client.get_first_module::<LightningClientModule>();
    let has_v1_lightning = v1_lightning.is_ok();
    if let Ok(lightning) = client.get_first_module::<LightningV2ClientModule>() {
        match lightning.select_gateway(None).await {
            Ok((gateway, routing_info)) => {
                let spendable = lightning
                    .spendable_amount(balance, Some(gateway.clone()))
                    .await?;
                let (invoice, amount) =
                    lnurl_pay(destination, |maximum| spendable.msats.min(maximum)).await?;
                let gateway_fee_quote_msat =
                    routing_info.send_parameters(&invoice).0.fee(amount).msats;
                let operation_id = lightning
                    .send(
                        invoice,
                        Some(gateway.clone()),
                        payout_metadata(request_id, destination),
                    )
                    .await?;
                log_payout_start(
                    client,
                    OutgoingRail::Lnv2,
                    &routing_info.module_public_key,
                    operation_id,
                    amount,
                    gateway_fee_quote_msat,
                );
                return Ok(Payout {
                    operation_id: PayoutOperationId::parse(&operation_id.fmt_full().to_string())
                        .expect("Fedimint formats a canonical operation id"),
                    amount_msat: amount,
                });
            }
            Err(error) if has_v1_lightning => {
                log_v2_fallback(client, v2_fallback_reason(&error));
            }
            Err(error) => {
                warn!(
                    federation_id = %client.federation_id(),
                    rail = "lnv2",
                    gateway_selection = v2_fallback_reason(&error).gateway_selection(),
                    "Lightning v2 gateway selection failed with no v1 fallback"
                );
                return Err(error).context("select Lightning v2 gateway");
            }
        }
    } else if has_v1_lightning {
        log_v2_fallback(client, V2Fallback::ModuleAbsentOrUnsupported);
    }

    {
        let lightning = v1_lightning.context("federation has no Lightning v1 module")?;
        let gateway = select_v1_gateway(client, &lightning).await?;
        validate_v1_gateway_fees(balance, &gateway.fees)?;
        let spendable = lightning
            .spendable_amount(balance, Some(gateway.clone()))
            .await?;
        let (invoice, amount) =
            lnurl_pay(destination, |maximum| spendable.msats.min(maximum)).await?;
        let has_completed_payment = {
            let mut dbtx = lightning.db.begin_transaction_nc().await;
            lightning
                .get_prev_payment_result(invoice.payment_hash(), &mut dbtx)
                .await
                .completed_payment
                .is_some()
        };
        let payment = start_fresh_v1_payment(has_completed_payment, || {
            lightning.pay_bolt11_invoice(
                Some(gateway.clone()),
                invoice,
                payout_metadata(request_id, destination),
            )
        })
        .await?;
        let operation_id = payment.payment_type.operation_id();
        validate_committed_v1_payout_amount(
            drain_status::v1_payout_request_amount(client, operation_id, request_id, destination)
                .await,
            amount,
        )?;
        let gateway_fee_quote_msat = gateway.fees.to_amount(&Amount::from_msats(amount)).msats;
        log_payout_start(
            client,
            OutgoingRail::Lnv1,
            &gateway.gateway_id,
            operation_id,
            amount,
            gateway_fee_quote_msat,
        );
        Ok(Payout {
            operation_id: PayoutOperationId::parse(&operation_id.fmt_full().to_string())
                .expect("Fedimint formats a canonical operation id"),
            amount_msat: amount,
        })
    }
}

/// Reject a v1 gateway fee schedule that can panic or overflow the pinned
/// client's fee arithmetic for an invoice no larger than `balance`.
pub(crate) fn validate_v1_gateway_fees(balance: Amount, fees: &RoutingFees) -> anyhow::Result<()> {
    // Mirror the pinned client's `FeeToAmount` implementation without invoking
    // its unchecked division or additions. The fee is monotone over
    // `[0, balance]`, so proving the endpoint also proves every binary-search
    // candidate passed to `spendable_amount`.
    let margin_fee = if fees.proportional_millionths == 0 {
        0
    } else {
        let divisor = 1_000_000u64 / u64::from(fees.proportional_millionths);
        anyhow::ensure!(divisor != 0, "unsupported v1 gateway fee rate");
        balance.msats / divisor
    };
    let fee = u64::from(fees.base_msat)
        .checked_add(margin_fee)
        .context("v1 gateway fee overflows millisatoshi amount")?;
    balance
        .msats
        .checked_add(fee)
        .context("v1 gateway contract amount overflows millisatoshi amount")?;
    Ok(())
}

fn payout_metadata(request_id: &PayoutRequestId, destination: &str) -> serde_json::Value {
    serde_json::json!({
        "purpose": "fman-payout",
        "request_id": request_id,
        "destination": destination,
    })
}

/// Log a selected gateway and its fee quote after a payout operation is durable.
fn log_payout_start(
    client: &ClientHandleArc,
    rail: OutgoingRail,
    gateway_public_key: &PublicKey,
    operation_id: OperationId,
    recipient_amount_msat: u64,
    gateway_fee_quote_msat: u64,
) {
    emit_payout_start(
        &client.federation_id().to_string(),
        rail,
        gateway_public_key,
        operation_id,
        recipient_amount_msat,
        gateway_fee_quote_msat,
    );
}

/// Emit the closed, post-commit payout-start diagnostic.
fn emit_payout_start(
    federation_id: &str,
    rail: OutgoingRail,
    gateway_public_key: &PublicKey,
    operation_id: OperationId,
    recipient_amount_msat: u64,
    gateway_fee_quote_msat: u64,
) {
    info!(
        federation_id,
        rail = rail_name(rail),
        gateway_public_key = %gateway_public_key,
        operation_id = %operation_id.fmt_full(),
        recipient_amount_msat,
        gateway_fee_quote_msat,
        "committed FMan payout operation"
    );
}

/// Closed, upstream-derived reasons for continuing a payout on the legacy rail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum V2Fallback {
    /// The federation has no LNv2 client module.
    ModuleAbsentOrUnsupported,
    /// The LNv2 module reported no registered gateway APIs.
    NoAnnouncements,
    /// No gateway API supplied routing information for this federation.
    NoResponsiveSupportedGateway,
    /// The LNv2 module could not read the federation's gateway announcements.
    AnnouncementQueryFailed,
}

impl V2Fallback {
    /// Return the closed diagnostic category for this fallback.
    fn gateway_selection(self) -> &'static str {
        match self {
            Self::ModuleAbsentOrUnsupported => "module_absent_or_unsupported",
            Self::NoAnnouncements => "no_announcements",
            Self::NoResponsiveSupportedGateway => "no_responsive_supported_gateway",
            Self::AnnouncementQueryFailed => "announcement_query_failed",
        }
    }

    /// Return whether this fallback warns because it is not normal capability absence.
    fn warns(self) -> bool {
        match self {
            Self::ModuleAbsentOrUnsupported | Self::NoAnnouncements => false,
            Self::NoResponsiveSupportedGateway | Self::AnnouncementQueryFailed => true,
        }
    }

    /// Return the fixed operator message for this fallback.
    fn message(self) -> &'static str {
        match self {
            Self::ModuleAbsentOrUnsupported => "Lightning v2 is unavailable; using Lightning v1",
            Self::NoAnnouncements => {
                "Lightning v2 has no gateway announcements; using Lightning v1"
            }
            Self::NoResponsiveSupportedGateway => {
                "Lightning v2 gateways did not provide routing information; using Lightning v1"
            }
            Self::AnnouncementQueryFailed => {
                "could not read Lightning v2 gateway announcements; using Lightning v1"
            }
        }
    }
}

/// Map the closed upstream result without formatting its arbitrary error detail.
fn v2_fallback_reason(error: &SelectGatewayError) -> V2Fallback {
    match error {
        SelectGatewayError::NoGatewaysAvailable => V2Fallback::NoAnnouncements,
        SelectGatewayError::GatewaysUnresponsive => V2Fallback::NoResponsiveSupportedGateway,
        SelectGatewayError::FailedToRequestGateways(_) => V2Fallback::AnnouncementQueryFailed,
    }
}

/// Log a v1 fallback without treating expected v2 absence as an operator alarm.
fn log_v2_fallback(client: &ClientHandleArc, fallback: V2Fallback) {
    emit_v2_fallback(&client.federation_id().to_string(), fallback);
}

/// Emit one closed v2-to-v1 fallback diagnostic.
fn emit_v2_fallback(federation_id: &str, fallback: V2Fallback) {
    if fallback.warns() {
        warn!(
            federation_id,
            rail = "lnv2",
            fallback_rail = "lnv1",
            gateway_selection = fallback.gateway_selection(),
            "{}",
            fallback.message()
        );
    } else {
        info!(
            federation_id,
            rail = "lnv2",
            fallback_rail = "lnv1",
            gateway_selection = fallback.gateway_selection(),
            "{}",
            fallback.message()
        );
    }
}

/// Find one request-marked native payout without creating an invoice or payment.
pub(crate) async fn payout_for_request(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
) -> anyhow::Result<Option<Payout>> {
    drain_status::payout_for_request(client, request_id, destination).await
}

/// Read the cached rail state and independent active-state-machine fact for one
/// exact native payout.
pub(crate) async fn payout_status(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<crate::wallet_drain::OutgoingOperation> {
    let operation_id = OperationId::from_str(operation_id.as_str())
        .expect("PayoutOperationId validates Fedimint's native encoding");
    let status =
        drain_status::payout_status(client, operation_id, Some(request_id), Some(destination))
            .await?;
    log_payout_rail(client, &status);
    Ok(status)
}

/// Await the rail outcome of one exact native payout, then report its current
/// rail and mint state without creating another payment.
pub(crate) async fn await_payout(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<crate::wallet_drain::OutgoingOperation> {
    let native_id = OperationId::from_str(operation_id.as_str())
        .expect("PayoutOperationId validates Fedimint's native encoding");
    drain_status::payout_status(client, native_id, Some(request_id), Some(destination)).await?;
    let status = payout_observer::await_terminal(client, operation_id).await?;
    log_payout_rail(client, &status);
    Ok(status)
}

/// Log the exact rail fact and only the fee effect encoded in its contract.
fn log_payout_rail(client: &ClientHandleArc, payout: &OutgoingOperation) {
    emit_payout_rail(&client.federation_id().to_string(), payout);
}

/// Emit the exact-operation rail diagnostic without inferring unobserved fees.
fn emit_payout_rail(federation_id: &str, payout: &OutgoingOperation) {
    let contract_amount_msat = payout.contract_amount_msat();
    info!(
        federation_id,
        rail = rail_name(payout.rail),
        operation_id = %payout.operation_id,
        rail_state = ?payout.state(),
        has_active_state_machines = payout.has_active_state_machines(),
        recipient_amount_msat = payout.recipient_amount_msat,
        contract_amount_msat,
        known_contract_fee_effect_msat = contract_amount_msat.saturating_sub(payout.recipient_amount_msat),
        encumbered_msat = ?payout.encumbered_msat(),
        "observed FMan payout rail"
    );
}

/// Return the stable diagnostic spelling for each Lightning rail.
fn rail_name(rail: OutgoingRail) -> &'static str {
    match rail {
        OutgoingRail::Lnv1 => "lnv1",
        OutgoingRail::Lnv2 => "lnv2",
    }
}

#[cfg(test)]
mod tests;
