//! The configured gatewayd admin-API adapter.
//!
//! One deployment is locked to one gatewayd. This module is the boundary to
//! it: the client trait the funding worker programs against, the configured
//! implementation, and the identity probe setup validation runs before an
//! operator commits a gatewayd to the deployment.
//!
//! The worker that funds gateway allocations through this adapter lives in
//! [`crate::gateway_allocation`], mirroring the split the stability-pool side
//! already has between [`crate::stability_pool`] and
//! [`crate::stability_allocation`]. Keeping the adapter free of the worker's
//! dependencies is what lets `setup_store` probe a gatewayd without importing
//! the worker that reads `setup_store` back.

use std::collections::BTreeSet;

use async_trait::async_trait;
use bitcoin::Address;
use bitcoin::address::NetworkUnchecked;
use fedi_decentralized_service_liquidity_manager::{
    BitcoinNetwork, GatewayApiUrl, GatewayId, Sats, SetupConfigView,
};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_core::util::SafeUrl;
use fedimint_eventlog::{Event, EventKind, EventLogId, PersistedLogEntry};
use fedimint_gateway_client::payment_log;
use fedimint_gateway_common::{
    ConnectFedPayload, DepositAddressPayload, DepositAddressRecheckPayload, GatewayInfo,
    LightningInfo, PaymentLogPayload, RegisteredProtocol, V1_API_ENDPOINT,
};
use fedimint_ln_common::client::GatewayApi;
use fedimint_wallet_client::events::DepositConfirmed;
use fedimint_walletv2_client::events::{
    ReceivePaymentEvent, ReceivePaymentStatus, ReceivePaymentUpdateEvent,
};

use crate::wallet::{bitcoin_network_to_domain, domain_network_to_bitcoin};

/// Claim events requested per payment-log read.
///
/// The search walks the log a page at a time, so this sets the cost of one
/// round trip rather than how far back a claim can be found.
const PAYMENT_LOG_PAGE_SIZE: usize = 1000;

/// Event kinds a gateway's federation client logs when it claims a deposit.
///
/// The two wallet modules record a claim differently. A wallet v1 federation
/// logs one [`DepositConfirmed`] per claimed peg-in. A walletv2 federation
/// logs [`ReceivePaymentEvent`] when its client submits the claiming
/// transaction and [`ReceivePaymentUpdateEvent`] when the federation accepts
/// or rejects it, so both are needed to tell a claimed deposit from an
/// attempted one.
fn claim_event_kinds() -> Vec<EventKind> {
    vec![
        DepositConfirmed::KIND,
        ReceivePaymentEvent::KIND,
        ReceivePaymentUpdateEvent::KIND,
    ]
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewaySnapshot {
    pub state: String,
    pub network: BitcoinNetwork,
    pub synced_to_chain: bool,
    pub gateway_api: GatewayApiUrl,
    pub federations: Vec<GatewayFederationSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayFederationSnapshot {
    pub federation_id: String,
    pub balance: Sats,
}

/// A deposit the gateway's own Fedimint client observed and claimed.
///
/// This is the target-side record the completion guard needs. Both wallet
/// modules name the Bitcoin outpoint they claimed, so both reduce to this
/// shape: a txid plus output index identifies exactly the output the item's
/// funding operation paid, which is what makes completion attribution rather
/// than a balance coincidence. `amount` is what the federation credited, so a
/// module that charges its peg-in fee out of the deposit reports the value
/// after that fee.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayDepositClaim {
    pub txid: String,
    pub out_idx: u32,
    pub amount: Sats,
}

/// The deposit output one allocation item's funding operation paid.
///
/// Completion needs an item-output-to-claim identity, so the worker names the
/// output it is looking for rather than reading the gateway's claims in bulk
/// and choosing afterwards.
pub(crate) struct DepositClaimQuery<'a> {
    /// Transaction the item's funding operation recorded.
    pub txid: &'a str,
    /// Output index chain observation verified, when it verified one. A
    /// manual review an operator resolved leaves it unset, and the txid is
    /// then the whole of the attribution that exists.
    pub out_idx: Option<u32>,
    /// Least amount the federation must have credited for the deposit to
    /// cover the item.
    pub min_amount: Sats,
}

impl DepositClaimQuery<'_> {
    /// Whether one claim names the output this query identifies.
    ///
    /// One transaction can pay two items' deposit addresses in separate
    /// outputs, so a txid alone does not name the output an item funded.
    pub(crate) fn matches(&self, claim: &GatewayDepositClaim) -> bool {
        claim.txid == self.txid
            && self.out_idx.is_none_or(|out_idx| claim.out_idx == out_idx)
            && claim.amount.0 >= self.min_amount.0
    }
}

#[async_trait]
pub(crate) trait GatewayClient: Send + Sync {
    async fn gateway_info(&self) -> anyhow::Result<GatewaySnapshot>;

    async fn connect_federation(
        &self,
        invite_code: &str,
    ) -> anyhow::Result<GatewayFederationSnapshot>;

    async fn deposit_address(
        &self,
        federation_id: &str,
        expected_network: BitcoinNetwork,
    ) -> anyhow::Result<String>;

    async fn recheck_deposit_address(
        &self,
        federation_id: &str,
        address: &str,
        expected_network: BitcoinNetwork,
    ) -> anyhow::Result<()>;

    async fn observe_federation_balance(
        &self,
        federation_id: &str,
    ) -> anyhow::Result<Option<Sats>> {
        Ok(self
            .gateway_info()
            .await?
            .federations
            .into_iter()
            .find(|federation| federation.federation_id == federation_id)
            .map(|federation| federation.balance))
    }

    /// The gateway Fedimint client's claim of the deposit this query names,
    /// if its payment log holds one.
    ///
    /// An aggregate balance read cannot establish attribution, so completion
    /// asks for this instead. The whole log is searched, so `None` means the
    /// gateway has not claimed that output rather than that the search gave
    /// up short of it.
    async fn find_deposit_claim(
        &self,
        federation_id: &str,
        query: &DepositClaimQuery<'_>,
    ) -> anyhow::Result<Option<GatewayDepositClaim>>;
}

/// What a gateway reports about itself, read before any config names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayIdentityProbe {
    pub gateway_id: GatewayId,
    pub network: BitcoinNetwork,
    pub lightning_alias: String,
}

/// Asks a gateway who it is.
///
/// This exists because `gateway_id` is operator input today and cannot be: it
/// is frozen at first setup, it decides which gateway an accepted allocation
/// pays, and a typo in it is permanent. The wizard never collected it, so
/// first-time setup through the dashboard was impossible — the daemon refused
/// every save for a field no screen offered.
///
/// The identity taken is the Lightning node's public key. `GatewayInfo` carries
/// no gateway id of its own, and of the two candidates this is the one always
/// present when setup can succeed at all: the network check already refuses a
/// gateway whose node is not connected. FLIP never sends this value back to the
/// gateway — it is a local label on completion evidence and allocation records —
/// so it only has to be stable and distinct, and it is read once, when nothing
/// is stored yet.
pub(crate) async fn probe_gateway_identity(
    admin_url: &str,
    admin_credential: String,
) -> anyhow::Result<GatewayIdentityProbe> {
    let connectors = ConnectorRegistry::build_from_client_defaults()
        .bind()
        .await?;
    let base_url = SafeUrl::parse(admin_url)?.join(V1_API_ENDPOINT)?;
    let api = GatewayApi::new(Some(admin_credential), connectors);
    let info: GatewayInfo = fedimint_gateway_client::get_info(&api, &base_url).await?;

    match info.lightning_info {
        LightningInfo::Connected {
            public_key,
            alias,
            network,
            ..
        } => Ok(GatewayIdentityProbe {
            gateway_id: GatewayId(public_key.to_string()),
            network: bitcoin_network_to_domain(network),
            lightning_alias: alias,
        }),
        LightningInfo::NotConnected => anyhow::bail!(
            "the gateway's Lightning node is not connected, so it cannot report an identity yet"
        ),
    }
}

#[derive(Clone)]
pub(crate) struct ConfiguredGatewayClient {
    api: GatewayApi,
    base_url: SafeUrl,
}

impl ConfiguredGatewayClient {
    pub(crate) async fn new(
        config: SetupConfigView,
        admin_credential: String,
    ) -> anyhow::Result<Self> {
        let connectors = ConnectorRegistry::build_from_client_defaults()
            .bind()
            .await?;
        let base_url = SafeUrl::parse(&config.gateway.admin_url)?.join(V1_API_ENDPOINT)?;
        Ok(Self {
            api: GatewayApi::new(Some(admin_credential), connectors),
            base_url,
        })
    }
}

#[async_trait]
impl GatewayClient for ConfiguredGatewayClient {
    async fn gateway_info(&self) -> anyhow::Result<GatewaySnapshot> {
        let info: GatewayInfo =
            fedimint_gateway_client::get_info(&self.api, &self.base_url).await?;
        gateway_snapshot_from_info(info)
    }

    async fn connect_federation(
        &self,
        invite_code: &str,
    ) -> anyhow::Result<GatewayFederationSnapshot> {
        let federation = fedimint_gateway_client::connect_federation(
            &self.api,
            &self.base_url,
            ConnectFedPayload {
                invite_code: invite_code.to_owned(),
                use_tor: Some(false),
                recover: Some(true),
            },
        )
        .await?;
        Ok(GatewayFederationSnapshot {
            federation_id: federation.federation_id.to_string(),
            balance: Sats(federation.balance_msat.msats / 1000),
        })
    }

    async fn deposit_address(
        &self,
        federation_id: &str,
        expected_network: BitcoinNetwork,
    ) -> anyhow::Result<String> {
        let federation_id = federation_id.parse::<FederationId>()?;
        let address: Address<NetworkUnchecked> = fedimint_gateway_client::get_deposit_address(
            &self.api,
            &self.base_url,
            DepositAddressPayload { federation_id },
        )
        .await?;
        Ok(address
            .require_network(domain_network_to_bitcoin(expected_network))?
            .to_string())
    }

    async fn recheck_deposit_address(
        &self,
        federation_id: &str,
        address: &str,
        expected_network: BitcoinNetwork,
    ) -> anyhow::Result<()> {
        let federation_id = federation_id.parse::<FederationId>()?;
        let address: Address<NetworkUnchecked> = address.parse()?;
        address
            .clone()
            .require_network(domain_network_to_bitcoin(expected_network))?;
        fedimint_gateway_client::recheck_address(
            &self.api,
            &self.base_url,
            DepositAddressRecheckPayload {
                address,
                federation_id,
            },
        )
        .await?;
        Ok(())
    }

    async fn find_deposit_claim(
        &self,
        federation_id: &str,
        query: &DepositClaimQuery<'_>,
    ) -> anyhow::Result<Option<GatewayDepositClaim>> {
        let federation_id = federation_id.parse::<FederationId>()?;
        let mut reader = DepositClaimReader::default();
        // `None` starts at the newest log position; each later read ends just
        // before the oldest entry the previous one returned, so the walk
        // strictly recedes and finishes at the start of the log.
        let mut end_position = None;
        loop {
            let response = payment_log(
                &self.api,
                &self.base_url,
                PaymentLogPayload {
                    end_position,
                    pagination_size: PAYMENT_LOG_PAGE_SIZE,
                    federation_id,
                    event_kinds: claim_event_kinds(),
                },
            )
            .await?;
            let page = response.0;
            if let Some(claim) = reader
                .read_page(&page)
                .into_iter()
                .find(|claim| query.matches(claim))
            {
                return Ok(Some(claim));
            }
            match page_before(&page) {
                Some(position) => end_position = Some(position),
                None => return Ok(None),
            }
        }
    }
}

/// The inclusive end position of the page preceding this one, or `None` when
/// this page is empty or already reaches the start of the log.
fn page_before(page: &[PersistedLogEntry]) -> Option<EventLogId> {
    page.last()?.id().checked_sub(1)
}

/// Reduces a gateway's payment log to the deposits its federation client
/// claimed, whichever wallet module served the federation, while the log is
/// read newest-first one page at a time.
///
/// A walletv2 receive is admitted only when the federation accepted the
/// claiming transaction: [`ReceivePaymentEvent`] alone records an attempt, and
/// its `Aborted` counterpart records one that failed. The credited amount is
/// the output value less the module's receive fee, which is the ecash the
/// client issued itself for that deposit.
///
/// The accepting update is logged after the receive it accepts, so a
/// backwards walk meets the update first. Accepted operation ids therefore
/// carry from page to page, and a receive a page boundary separates from its
/// update still resolves.
#[derive(Default)]
struct DepositClaimReader {
    accepted: BTreeSet<OperationId>,
}

impl DepositClaimReader {
    fn read_page(&mut self, entries: &[PersistedLogEntry]) -> Vec<GatewayDepositClaim> {
        self.accepted.extend(
            entries
                .iter()
                .filter(|entry| entry.as_raw().kind == ReceivePaymentUpdateEvent::KIND)
                .filter_map(|entry| entry.as_raw().to_event::<ReceivePaymentUpdateEvent>())
                .filter(|update| matches!(update.status, ReceivePaymentStatus::Success))
                .map(|update| update.operation_id),
        );
        let accepted = &self.accepted;

        entries
            .iter()
            .filter_map(|entry| {
                let raw = entry.as_raw();
                if raw.kind == DepositConfirmed::KIND {
                    let event = raw.to_event::<DepositConfirmed>()?;
                    return Some(GatewayDepositClaim {
                        txid: event.txid.to_string(),
                        out_idx: event.out_idx,
                        amount: Sats(event.amount.msats / 1000),
                    });
                }
                if raw.kind == ReceivePaymentEvent::KIND {
                    let event = raw.to_event::<ReceivePaymentEvent>()?;
                    if !accepted.contains(&event.operation_id) {
                        return None;
                    }
                    // A receive the federation has not yet assigned an outpoint
                    // names no output, so it attributes nothing.
                    let outpoint = event.outpoint?;
                    let credited = event.value.checked_sub(event.fee)?;
                    return Some(GatewayDepositClaim {
                        txid: outpoint.txid.to_string(),
                        out_idx: outpoint.vout,
                        amount: Sats(credited.to_sat()),
                    });
                }
                None
            })
            .collect()
    }
}

fn gateway_snapshot_from_info(info: GatewayInfo) -> anyhow::Result<GatewaySnapshot> {
    let (network, synced_to_chain) = match info.lightning_info {
        LightningInfo::Connected {
            network,
            synced_to_chain,
            ..
        } => (bitcoin_network_to_domain(network), synced_to_chain),
        LightningInfo::NotConnected => {
            anyhow::bail!("gatewayd lightning node is not connected");
        }
    };

    let registered_url = info
        .registrations
        .get(&RegisteredProtocol::Iroh)
        .or_else(|| info.registrations.get(&RegisteredProtocol::Http))
        .map(|(url, _)| url.as_str())
        .ok_or_else(|| anyhow::anyhow!("gatewayd has no client-facing registration"))?;
    let gateway_api = GatewayApiUrl::try_from(registered_url)
        .map_err(|_| anyhow::anyhow!("gatewayd has no safe client-facing registration"))?;

    Ok(GatewaySnapshot {
        state: info.gateway_state,
        network,
        synced_to_chain,
        gateway_api,
        federations: info
            .federations
            .into_iter()
            .map(|federation| GatewayFederationSnapshot {
                federation_id: federation.federation_id.to_string(),
                balance: Sats(federation.balance_msat.msats / 1000),
            })
            .collect(),
    })
}

#[cfg(test)]
#[path = "../tests/gateway.rs"]
mod tests;
