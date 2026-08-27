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

use async_trait::async_trait;
use bitcoin::Address;
use bitcoin::address::NetworkUnchecked;
use fedi_decentralized_service_liquidity_manager::{
    BitcoinNetwork, GatewayApiUrl, GatewayId, Sats, SetupConfigView,
};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::config::FederationId;
use fedimint_core::util::SafeUrl;
use fedimint_gateway_client::payment_log;
use fedimint_gateway_common::{
    ConnectFedPayload, DepositAddressPayload, DepositAddressRecheckPayload, GatewayInfo,
    LightningInfo, PaymentLogPayload, RegisteredProtocol, V1_API_ENDPOINT,
};
use fedimint_ln_common::client::GatewayApi;

use crate::wallet::{bitcoin_network_to_domain, domain_network_to_bitcoin};

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
/// This is the target-side record the completion guard needs: gatewayd logs a
/// `deposit-confirmed` event for every peg-in its federation client claims,
/// with the Bitcoin transaction id, output index, and amount of the deposit.
/// A txid plus output index identifies exactly the output the item's funding
/// operation paid, so matching it is what makes completion attribution and
/// not a balance coincidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayDepositClaim {
    pub txid: String,
    pub out_idx: u32,
    pub amount: Sats,
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

    /// Deposits the gateway's Fedimint client has observed and claimed for
    /// this federation. The completion guard matches one of these against the
    /// item's own funding txid; an aggregate balance read cannot.
    async fn deposit_claims(
        &self,
        federation_id: &str,
    ) -> anyhow::Result<Vec<GatewayDepositClaim>>;
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

    async fn deposit_claims(
        &self,
        federation_id: &str,
    ) -> anyhow::Result<Vec<GatewayDepositClaim>> {
        let federation_id = federation_id.parse::<FederationId>()?;
        let response = payment_log(
            &self.api,
            &self.base_url,
            PaymentLogPayload {
                // `None` starts at the newest log position, so the read covers
                // every retained event.
                end_position: None,
                pagination_size: PAYMENT_LOG_PAGE_SIZE,
                federation_id,
                event_kinds: vec![DEPOSIT_CONFIRMED_KIND.into()],
            },
        )
        .await?;
        Ok(response
            .0
            .iter()
            .filter_map(|entry| {
                // `DepositConfirmed` is the wallet-client event gatewayd logs
                // when its federation client claims a deposit; its payload
                // carries the txid, output index, and deposit amount.
                let payload: fedimint_wallet_client::events::DepositConfirmed =
                    serde_json::from_slice(&entry.as_raw().payload).ok()?;
                Some(GatewayDepositClaim {
                    txid: payload.txid.to_string(),
                    out_idx: payload.out_idx,
                    amount: Sats(payload.amount.msats / 1000),
                })
            })
            .collect())
    }
}

/// Upper bound on deposit-confirmed entries read per completion check. The
/// log is read newest-first and each item matches its own txid, so a page
/// larger than the concurrent-item ceiling adds matches, not correctness.
const PAYMENT_LOG_PAGE_SIZE: usize = 1000;

/// Event kind gatewayd's federation clients log when they observe a confirmed
/// deposit, from `fedimint_wallet_client::events::DepositConfirmed::KIND`.
const DEPOSIT_CONFIRMED_KIND: &str = "deposit-confirmed";

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
