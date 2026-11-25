//! Native Lightning payout: prepare an attempt, pay it, and read it back.

use crate::payout_job::{LightningAttempt, Payout, PayoutRequestId};
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::{OutgoingOperation, OutgoingRail};
use anyhow::Context as _;
use bitcoin::secp256k1::PublicKey;
use fedimint_client::ClientHandleArc;
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::util::SafeUrl;
use fedimint_ln_client::LightningClientModule;
use fedimint_ln_common::config::FeeToAmount as _;
use fedimint_lnv2_client::{LightningClientModule as LightningV2ClientModule, SelectGatewayError};
use fedimint_lnv2_common::gateway_api::PaymentFee;
use lightning_invoice::RoutingFees;
use tracing::{info, warn};

use super::{drain_status, lnurl_pay, payout_observer, select_v1_gateway};

/// A Lightning payout sized and invoiced through a selected gateway, not yet
/// paid.
pub(crate) struct PreparedPayout {
    pub(crate) attempt: LightningAttempt,
    gateway: PreparedGateway,
    gateway_fee_quote_msat: u64,
}

enum PreparedGateway {
    V1(fedimint_ln_common::LightningGateway),
    V2(SafeUrl, PublicKey),
}

/// Select the rail and gateway, size the payout to the spendable balance, and
/// fetch its invoice. Nothing is paid. The invoice must be new to this wallet,
/// so the operation its attempt names can only be this payout's.
pub(crate) async fn prepare_payout(
    client: &ClientHandleArc,
    destination: &str,
) -> anyhow::Result<PreparedPayout> {
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
                let (invoice, amount, capped) = lnurl_pay(destination, spendable.msats).await?;
                let gateway_fee_quote_msat =
                    routing_info.send_parameters(&invoice).0.fee(amount).msats;
                let attempt = LightningAttempt {
                    rail: OutgoingRail::Lnv2,
                    invoice,
                    capped,
                };
                anyhow::ensure!(
                    !client
                        .operation_exists(attempt.operation_id().to_native())
                        .await,
                    "LNURL endpoint reused an already-paid Lightning invoice"
                );
                return Ok(PreparedPayout {
                    attempt,
                    gateway: PreparedGateway::V2(gateway, routing_info.module_public_key),
                    gateway_fee_quote_msat,
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

    let lightning = v1_lightning.context("federation has no Lightning v1 module")?;
    let gateway = select_v1_gateway(client, &lightning, balance).await?;
    validate_v1_gateway_fees(balance, &gateway.fees)?;
    let spendable = lightning
        .spendable_amount(balance, Some(gateway.clone()))
        .await?;
    let (invoice, amount, capped) = lnurl_pay(destination, spendable.msats).await?;
    // The v1 client numbers payment attempts per payment hash; the attempt's
    // operation id is the first one's.
    let attempted = {
        let mut dbtx = lightning.db.begin_transaction_nc().await;
        lightning
            .get_prev_payment_result(invoice.payment_hash(), &mut dbtx)
            .await
            .index
            != 0
    };
    anyhow::ensure!(
        !attempted,
        "LNURL endpoint reused an already-paid Lightning invoice"
    );
    let gateway_fee_quote_msat = gateway.fees.to_amount(&Amount::from_msats(amount)).msats;
    Ok(PreparedPayout {
        attempt: LightningAttempt {
            rail: OutgoingRail::Lnv1,
            invoice,
            capped,
        },
        gateway: PreparedGateway::V1(gateway),
        gateway_fee_quote_msat,
    })
}

/// Pay a prepared payout, whose attempt the ledger already holds, and return
/// once Fedimint has committed its operation under the attempt's id.
pub(crate) async fn pay_payout(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    destination: &str,
    prepared: PreparedPayout,
) -> anyhow::Result<Payout> {
    let PreparedPayout {
        attempt,
        gateway,
        gateway_fee_quote_msat,
    } = prepared;
    let metadata = payout_metadata(request_id, destination);
    let (operation_id, gateway_public_key) = match gateway {
        PreparedGateway::V2(gateway, public_key) => (
            client
                .get_first_module::<LightningV2ClientModule>()?
                .send(attempt.invoice.clone(), Some(gateway), metadata)
                .await?,
            public_key,
        ),
        PreparedGateway::V1(gateway) => {
            let public_key = gateway.gateway_id;
            let payment = client
                .get_first_module::<LightningClientModule>()?
                .pay_bolt11_invoice(Some(gateway), attempt.invoice.clone(), metadata)
                .await?;
            (payment.payment_type.operation_id(), public_key)
        }
    };
    anyhow::ensure!(
        PayoutOperationId::from_native(operation_id) == attempt.operation_id(),
        "Lightning payout started under an operation other than its attempt's"
    );
    log_payout_start(
        client,
        attempt.rail,
        &gateway_public_key,
        operation_id,
        attempt.amount_msat(),
        gateway_fee_quote_msat,
    );
    Ok(Payout {
        operation_id: attempt.operation_id(),
        amount_msat: attempt.amount_msat(),
    })
}

/// The payout of the job's recorded Lightning attempt, if the wallet has its
/// operation.
pub(crate) async fn attempted_payout(
    client: &ClientHandleArc,
    job: &crate::payout_job::PayoutJob,
) -> Option<Payout> {
    let attempt = job.attempt.as_ref()?;
    let operation_id = attempt.operation_id();
    client
        .operation_exists(operation_id.to_native())
        .await
        .then(|| Payout {
            operation_id,
            amount_msat: attempt.amount_msat(),
        })
}

/// Enforce the shared send fee-schedule limits and checked native arithmetic
/// for an invoice no larger than `balance`.
pub(crate) fn validate_v1_gateway_fees(balance: Amount, fees: &RoutingFees) -> anyhow::Result<()> {
    anyhow::ensure!(
        PaymentFee::from(*fees).is_within(&PaymentFee::SEND_FEE_LIMIT),
        "v1 gateway fee schedule exceeds send limits"
    );
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
        "committed MG payout operation"
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

/// Read the cached rail state and independent active-state-machine fact for one
/// exact native payout.
pub(crate) async fn payout_status(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<crate::wallet_drain::OutgoingOperation> {
    let status = drain_status::payout_status(client, operation_id.to_native()).await?;
    log_payout_rail(client, &status);
    Ok(status)
}

/// Await the rail outcome of one exact native payout, then report its current
/// rail and mint state without creating another payment.
pub(crate) async fn await_payout(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<crate::wallet_drain::OutgoingOperation> {
    drain_status::payout_status(client, operation_id.to_native()).await?;
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
        "observed MG payout rail"
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
