//! Periodic checks that a new seat would be served, gating admission.
//!
//! An FMan that can advertise and sell a seat but cannot reach its relay,
//! be discovered, or use its Bitcoin backend becomes the cause of a failed
//! DKG. Each run probes those prerequisites, hands the verdict to the fleet
//! (which stops advertising and quoting while it fails), and records the
//! outcome as one shareable event for telemetry.

use std::sync::Arc;
use std::time::Duration;

use fedimint_core::bitcoin::Network;
use fedimint_core::util::SafeUrl;
use fedimint_server_bitcoin_rpc::bitcoind::BitcoindClient;
use fedimint_server_bitcoin_rpc::esplora::EsploraClient;
use fedimint_server_core::bitcoin_rpc::{DynServerBitcoinRpc, IServerBitcoinRpc as _};
use fman_core::fleet::Fleet;
use fman_core::seat_process::{BitcoinBackend, SeatProcessConfig};
use futures::StreamExt as _;
use iroh::address_lookup::{
    AddressLookup, AddressLookupBuilder as _, DnsAddressLookup, PkarrResolver,
};
use iroh::{Endpoint, RelayUrl, Watcher as _};

const READY_INTERVAL: Duration = Duration::from_secs(10 * 60);
const NOT_READY_INTERVAL: Duration = Duration::from_secs(60);
/// A run retries failed prerequisites for about a minute, which also covers
/// relay connection and discovery publication after startup.
const ATTEMPTS: u32 = 6;
const RETRY_DELAY: Duration = Duration::from_secs(10);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// One prerequisite's fixed outcome code; never a remote value or error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Pass,
    NotApplicable,
    RelayDisconnected,
    DiscoveryRecordMissing,
    BitcoinUnavailable,
    BitcoinWrongNetwork,
    BitcoinSyncing,
    BitcoinNoFeeRate,
}

impl Outcome {
    fn passed(self) -> bool {
        matches!(self, Self::Pass | Self::NotApplicable)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::NotApplicable => "not_applicable",
            Self::RelayDisconnected => "relay_disconnected",
            Self::DiscoveryRecordMissing => "record_missing",
            Self::BitcoinUnavailable => "unavailable",
            Self::BitcoinWrongNetwork => "wrong_network",
            Self::BitcoinSyncing => "syncing",
            Self::BitcoinNoFeeRate => "no_fee_rate",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Report {
    relay: Outcome,
    discovery: Outcome,
    bitcoin: Outcome,
}

impl Report {
    fn ready(&self) -> bool {
        self.relay.passed() && self.discovery.passed() && self.bitcoin.passed()
    }
}

pub(super) struct SeatReadiness {
    endpoint: Endpoint,
    /// `None` in local E2E, whose endpoints use explicit loopback routes and
    /// neither use a relay nor publish discovery records.
    discovery: Option<[Box<dyn AddressLookup>; 2]>,
    bitcoin: Result<DynServerBitcoinRpc, ()>,
    network: Network,
}

impl SeatReadiness {
    pub(super) fn new(
        endpoint: Endpoint,
        process: &SeatProcessConfig,
        local_e2e: bool,
    ) -> anyhow::Result<Self> {
        let discovery = if local_e2e {
            None
        } else {
            // The same n0 services the endpoint publishes to. Either one
            // returning a usable record keeps this FMan discoverable.
            Some([
                Box::new(PkarrResolver::n0_dns().into_address_lookup(&endpoint)?)
                    as Box<dyn AddressLookup>,
                Box::new(DnsAddressLookup::n0_dns().into_address_lookup(&endpoint)?),
            ])
        };
        Ok(Self {
            endpoint,
            discovery,
            bitcoin: bitcoin_rpc(&process.bitcoin_backend),
            network: process.bitcoin_network,
        })
    }

    /// Run until aborted: probe, report, and wait longer while ready.
    pub(super) fn spawn(self, fleet: Arc<Fleet>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let report = self.probe().await;
                let ready = report.ready();
                tracing::info!(
                    safe_to_share = true,
                    ready,
                    relay = report.relay.as_str(),
                    discovery = report.discovery.as_str(),
                    bitcoin = report.bitcoin.as_str(),
                    "seat readiness check completed"
                );
                if let Err(error) = fleet.set_ready_for_new_seats(ready).await {
                    tracing::warn!(%error, "failed to apply the seat readiness verdict");
                }
                tokio::time::sleep(if ready {
                    READY_INTERVAL
                } else {
                    NOT_READY_INTERVAL
                })
                .await;
            }
        })
    }

    async fn probe(&self) -> Report {
        let mut report = self.probe_once().await;
        for _ in 1..ATTEMPTS {
            if report.ready() {
                break;
            }
            tokio::time::sleep(RETRY_DELAY).await;
            report = self.probe_once().await;
        }
        report
    }

    async fn probe_once(&self) -> Report {
        let relays = self.connected_relays();
        let relay = match (&self.discovery, relays.is_empty()) {
            (None, _) => Outcome::NotApplicable,
            (Some(_), true) => Outcome::RelayDisconnected,
            (Some(_), false) => Outcome::Pass,
        };
        let (discovery, bitcoin) = tokio::join!(self.discovery(&relays), self.bitcoin());
        Report {
            relay,
            discovery,
            bitcoin,
        }
    }

    fn connected_relays(&self) -> Vec<RelayUrl> {
        self.endpoint
            .home_relay_status()
            .get()
            .into_iter()
            .filter(|status| status.is_connected())
            .map(|status| status.url().clone())
            .collect()
    }

    /// Resolve this FMan's own record through the public services and require
    /// it to name a relay the endpoint is connected to.
    async fn discovery(&self, relays: &[RelayUrl]) -> Outcome {
        let Some(lookups) = &self.discovery else {
            return Outcome::NotApplicable;
        };
        for lookup in lookups {
            let Some(mut items) = lookup.resolve(self.endpoint.id()) else {
                continue;
            };
            let found = tokio::time::timeout(CHECK_TIMEOUT, async {
                while let Some(item) = items.next().await {
                    if let Ok(item) = item
                        && item.relay_urls().any(|url| relays.contains(url))
                    {
                        return true;
                    }
                }
                false
            })
            .await;
            if found == Ok(true) {
                return Outcome::Pass;
            }
        }
        Outcome::DiscoveryRecordMissing
    }

    async fn bitcoin(&self) -> Outcome {
        match &self.bitcoin {
            Ok(rpc) => bitcoin_outcome(rpc, self.network).await,
            Err(()) => Outcome::BitcoinUnavailable,
        }
    }
}

/// The status fedimintd's own Bitcoin monitor requires, read through the same
/// client it builds, plus a completed initial block download.
async fn bitcoin_outcome(rpc: &DynServerBitcoinRpc, network: Network) -> Outcome {
    tokio::time::timeout(CHECK_TIMEOUT, async {
        let Ok(chain_id) = rpc.get_chain_id().await else {
            return Outcome::BitcoinUnavailable;
        };
        if fedimint_server_core::bitcoin_rpc::network_from_chain_id(chain_id) != network {
            return Outcome::BitcoinWrongNetwork;
        }
        // Regtest chains idle long enough to report IBD, and fedimintd uses a
        // fixed regtest fee rate.
        if network == Network::Regtest {
            return match rpc.get_block_count().await {
                Ok(_) => Outcome::Pass,
                Err(_) => Outcome::BitcoinUnavailable,
            };
        }
        match rpc.get_block_count_and_initial_block_download().await {
            Ok((_, false)) => {}
            Ok((_, true)) => return Outcome::BitcoinSyncing,
            Err(_) => return Outcome::BitcoinUnavailable,
        }
        match rpc.get_feerate().await {
            Ok(Some(_)) => Outcome::Pass,
            Ok(None) => Outcome::BitcoinNoFeeRate,
            Err(_) => Outcome::BitcoinUnavailable,
        }
    })
    .await
    .unwrap_or(Outcome::BitcoinUnavailable)
}

/// The client fedimintd builds for this backend, except that Bitcoin Core is
/// required on its own: its Esplora fallback does not make an FMan ready.
fn bitcoin_rpc(backend: &BitcoinBackend) -> Result<DynServerBitcoinRpc, ()> {
    let rpc = match backend {
        BitcoinBackend::Bitcoind { primary, .. } => {
            let url: SafeUrl = primary.url.parse().map_err(drop)?;
            BitcoindClient::new(primary.username.clone(), primary.password.clone(), &url)
                .map_err(drop)?
                .into_dyn()
        }
        BitcoinBackend::Esplora(url) => EsploraClient::new(&SafeUrl::from(url.clone()))
            .map_err(drop)?
            .into_dyn(),
    };
    Ok(rpc)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::Json;
    use axum::http::{HeaderMap, StatusCode};
    use fman_core::seat_process::BitcoindConfig;
    use serde_json::{Value, json};

    use super::*;

    /// Block 1 of Mutinynet, which fedimintd classifies as Signet.
    const MUTINYNET_BLOCK_1: &str =
        "000002855893a0a9b24eaffc5efc770558a326fee4fc10c9da22fc19cd2954f9";
    /// Basic auth for `user:pass`.
    const AUTHORIZATION: &str = "Basic dXNlcjpwYXNz";

    /// Any other block 1 is how fedimintd identifies a regtest chain.
    const OTHER_BLOCK_1: &str = "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206";

    #[derive(Clone, Copy)]
    struct Core {
        block_1: &'static str,
        initial_block_download: bool,
        fee_rate: bool,
    }

    /// A Bitcoin Core JSON-RPC endpoint answering the reads the check makes.
    async fn serve_core(core: Core) -> String {
        let core = Arc::new(Mutex::new(core));
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |headers: HeaderMap, Json(request): Json<Value>| {
                let core = *core.lock().unwrap();
                async move {
                    if headers.get("authorization").and_then(|v| v.to_str().ok())
                        != Some(AUTHORIZATION)
                    {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let result = match request["method"].as_str().unwrap() {
                        "getblockhash" => json!(core.block_1),
                        "getblockcount" => json!(100),
                        "getnetworkinfo" => json!({ "version": 270_000 }),
                        "getblockchaininfo" => json!({
                            "chain": "signet",
                            "blocks": 100,
                            "headers": 100,
                            "bestblockhash": MUTINYNET_BLOCK_1,
                            "difficulty": 1.0,
                            "mediantime": 0,
                            "verificationprogress": 1.0,
                            "initialblockdownload": core.initial_block_download,
                            "chainwork": "00",
                            "size_on_disk": 0,
                            "pruned": false,
                            "warnings": "",
                        }),
                        "estimatesmartfee" if core.fee_rate => {
                            json!({ "feerate": 0.0001, "blocks": 1 })
                        }
                        "estimatesmartfee" => json!({ "errors": ["no data"], "blocks": 0 }),
                        method => panic!("unexpected Core method {method}"),
                    };
                    Ok(Json(
                        json!({ "result": result, "error": null, "id": request["id"] }),
                    ))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn core_rpc(url: String, password: &str) -> DynServerBitcoinRpc {
        bitcoin_rpc(&BitcoinBackend::Bitcoind {
            primary: BitcoindConfig {
                url,
                username: "user".to_owned(),
                password: password.to_owned(),
            },
            esplora_fallback: None,
        })
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bitcoin_core_must_serve_the_configured_synced_chain_with_fees() {
        let ready = Core {
            block_1: MUTINYNET_BLOCK_1,
            initial_block_download: false,
            fee_rate: true,
        };
        let url = serve_core(ready).await;
        assert_eq!(
            bitcoin_outcome(&core_rpc(url.clone(), "pass"), Network::Signet).await,
            Outcome::Pass
        );
        assert_eq!(
            bitcoin_outcome(&core_rpc(url.clone(), "pass"), Network::Bitcoin).await,
            Outcome::BitcoinWrongNetwork
        );
        assert_eq!(
            bitcoin_outcome(&core_rpc(url, "wrong"), Network::Signet).await,
            Outcome::BitcoinUnavailable
        );

        let syncing = serve_core(Core {
            initial_block_download: true,
            ..ready
        })
        .await;
        assert_eq!(
            bitcoin_outcome(&core_rpc(syncing, "pass"), Network::Signet).await,
            Outcome::BitcoinSyncing
        );

        let no_fee_rate = serve_core(Core {
            fee_rate: false,
            ..ready
        })
        .await;
        assert_eq!(
            bitcoin_outcome(&core_rpc(no_fee_rate, "pass"), Network::Signet).await,
            Outcome::BitcoinNoFeeRate
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn regtest_ignores_initial_block_download_and_fee_estimates() {
        let url = serve_core(Core {
            block_1: OTHER_BLOCK_1,
            initial_block_download: true,
            fee_rate: false,
        })
        .await;
        assert_eq!(
            bitcoin_outcome(&core_rpc(url.clone(), "pass"), Network::Regtest).await,
            Outcome::Pass
        );
        assert_eq!(
            bitcoin_outcome(&core_rpc(url, "pass"), Network::Signet).await,
            Outcome::BitcoinWrongNetwork
        );
    }
}
