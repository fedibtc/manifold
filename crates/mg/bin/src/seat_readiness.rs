//! Periodic checks that a new seat would be served, gating admission.
//!
//! An MG that can advertise and sell a seat but cannot reach its relay,
//! be discovered, or use its Bitcoin backend becomes the cause of a failed
//! DKG. This module probes those prerequisites against the endpoint and the
//! Bitcoin backend, and reads its running seats' consensus ordering latency
//! from their loopback metrics ports; [`mg_core::seat_readiness::run`] turns
//! each probe into a verdict for the fleet and a shareable telemetry event.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fedi_decentralized_domain::AdmittedFeaturePolicy;
use fedi_decentralized_service_manifold_guardian::{MAX_GUARDIAN_METRICS_BODY_BYTES, SeatId};
use fedimint_core::bitcoin::Network;
use fedimint_core::util::SafeUrl;
use fedimint_server_bitcoin_rpc::bitcoind::BitcoindClient;
use fedimint_server_bitcoin_rpc::esplora::EsploraClient;
use fedimint_server_core::bitcoin_rpc::{DynServerBitcoinRpc, IServerBitcoinRpc as _};
use futures::StreamExt as _;
use iroh::address_lookup::{
    AddressLookup, AddressLookupBuilder as _, DnsAddressLookup, PkarrResolver,
};
use iroh::{Endpoint, RelayUrl, Watcher as _};
use mg_core::fleet::Fleet;
use mg_core::ordering_latency::OrderingTotals;
use mg_core::seat_process::{BitcoinBackend, SeatProcessConfig};
use mg_core::seat_readiness::{
    self, OrderingLatencySource, ReadinessOutcome as Outcome, ReadinessProbe,
    ReadinessReport as Report,
};
use tokio::sync::watch;
use tokio::time::Instant;

/// A run retries failed prerequisites for about a minute, which also covers
/// relay connection and discovery publication after startup.
const RETRY_BUDGET: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(10);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const METRICS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct SeatReadiness {
    endpoint: Endpoint,
    /// `None` in local E2E, whose endpoints use explicit loopback routes and
    /// neither use a relay nor publish discovery records.
    discovery: Option<[Box<dyn AddressLookup>; 2]>,
    bitcoin_backend: BitcoinBackend,
    network: Network,
    /// A Bitcoin check that outlived its deadline. The next attempt waits on
    /// it rather than starting another, so a stalled backend holds at most
    /// one thread.
    stalled_bitcoin: Option<tokio::task::JoinHandle<Outcome>>,
    /// The client that reads the seats' loopback metrics; `None` in local
    /// E2E, which skips the ordering latency check.
    seat_metrics: Option<reqwest::Client>,
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
            // returning a usable record keeps this MG discoverable.
            Some([
                Box::new(PkarrResolver::n0_dns().into_address_lookup(&endpoint)?)
                    as Box<dyn AddressLookup>,
                Box::new(DnsAddressLookup::n0_dns().into_address_lookup(&endpoint)?),
            ])
        };
        // Every MG in a local end-to-end test runs on one busy host, so its
        // seats' ordering latency says nothing about any one MG.
        let seat_metrics = if local_e2e {
            None
        } else {
            // Plain-HTTP loopback only, as the telemetry proxy reads the same
            // ports: no redirects, so a seat cannot point the daemon
            // elsewhere, and no TLS roots it can never need.
            Some(
                reqwest::Client::builder()
                    .connect_timeout(METRICS_CONNECT_TIMEOUT)
                    .timeout(CHECK_TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none())
                    .tls_built_in_root_certs(false)
                    .build()?,
            )
        };
        Ok(Self {
            endpoint,
            discovery,
            bitcoin_backend: process.bitcoin_backend.clone(),
            network: process.bitcoin_network,
            stalled_bitcoin: None,
            seat_metrics,
        })
    }

    /// Run the readiness worker on this MG's probes until aborted
    /// ([`mg_core::seat_readiness::run`]).
    pub(super) fn spawn(
        mut self,
        fleet: Arc<Fleet>,
        feature_policy: watch::Receiver<Option<AdmittedFeaturePolicy>>,
    ) -> tokio::task::JoinHandle<()> {
        // A local end-to-end test runs every MG on one busy host's cgroup,
        // as it skips relay and discovery, so it skips pressure too.
        let read_pressure = self
            .discovery
            .is_some()
            .then_some(mg_core::host_pressure::read_cgroup_pressure);
        let ordering_latency = self.seat_metrics.take().map(|http| SeatOrderingLatency {
            fleet: fleet.clone(),
            http,
        });
        tokio::spawn(seat_readiness::run(
            self,
            fleet,
            feature_policy,
            SystemTime::now,
            read_pressure,
            ordering_latency,
        ))
    }

    async fn probe_once(&mut self) -> Report {
        let bitcoin_deadline = Instant::now() + CHECK_TIMEOUT;
        let mut bitcoin = self
            .stalled_bitcoin
            .take()
            .unwrap_or_else(|| spawn_bitcoin_check(self.bitcoin_backend.clone(), self.network));
        let relays = self.connected_relays();
        let relay = match (&self.discovery, relays.is_empty()) {
            (None, _) => Outcome::NotApplicable,
            (Some(_), true) => Outcome::RelayDisconnected,
            (Some(_), false) => Outcome::Pass,
        };
        let discovery = self.discovery(&relays).await;
        let bitcoin = match tokio::time::timeout_at(bitcoin_deadline, &mut bitcoin).await {
            Ok(outcome) => outcome.unwrap_or(Outcome::BitcoinUnavailable),
            Err(_) => {
                self.stalled_bitcoin = Some(bitcoin);
                Outcome::BitcoinUnavailable
            }
        };
        Report {
            checked_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is after the Unix epoch")
                .as_millis() as u64,
            relay,
            discovery,
            bitcoin,
            version: Outcome::NotApplicable,
            pressure: Outcome::NotApplicable,
            host_pressure: None,
            max_host_pressure: None,
            ordering_latency: Outcome::NotApplicable,
            ordering_latency_6h_ms: None,
            max_ordering_latency_6h_ms: None,
            uptime: Outcome::NotApplicable,
            uptime_comparison: None,
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

    /// Resolve this MG's own record through the public services and require
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
}

#[async_trait::async_trait]
impl ReadinessProbe for SeatReadiness {
    /// Probe the prerequisites, retrying failures for about a minute.
    async fn probe(&mut self) -> Report {
        let give_up = Instant::now() + RETRY_BUDGET;
        let mut report = self.probe_once().await;
        while !report.prerequisites_passed() && Instant::now() + RETRY_DELAY < give_up {
            tokio::time::sleep(RETRY_DELAY).await;
            report = self.probe_once().await;
        }
        report
    }
}

/// Reads each running seat's ordering latency counters from the fedimintd
/// metrics endpoint the seat binds on loopback.
struct SeatOrderingLatency {
    fleet: Arc<Fleet>,
    http: reqwest::Client,
}

#[async_trait::async_trait]
impl OrderingLatencySource for SeatOrderingLatency {
    async fn read(&mut self) -> Vec<(SeatId, Option<OrderingTotals>)> {
        let http = &self.http;
        futures::future::join_all(
            self.fleet
                .running_seat_metrics_ports()
                .await
                .into_iter()
                .map(|(seat_id, port)| async move {
                    (seat_id, read_ordering_totals(http, port).await)
                }),
        )
        .await
    }
}

/// The ordering latency totals in the metrics a seat serves on loopback
/// `port`, or `None` when they cannot be read.
async fn read_ordering_totals(http: &reqwest::Client, port: u16) -> Option<OrderingTotals> {
    let mut response = http
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if MAX_GUARDIAN_METRICS_BODY_BYTES.saturating_sub(body.len()) < chunk.len() {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    OrderingTotals::parse(std::str::from_utf8(&body).ok()?)
}

/// Check Bitcoin on a blocking thread with a client built for this attempt.
///
/// fedimint's Core client blocks inside its async methods, so a Tokio timeout
/// cannot interrupt it; only a thread boundary bounds it. Building the client
/// each time retries its constructor, which resolves the RPC host.
fn spawn_bitcoin_check(
    backend: BitcoinBackend,
    network: Network,
) -> tokio::task::JoinHandle<Outcome> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || match bitcoin_rpc(&backend) {
        Ok(rpc) => runtime.block_on(bitcoin_outcome(&rpc, network)),
        Err(()) => Outcome::BitcoinUnavailable,
    })
}

/// The status fedimintd's own Bitcoin monitor requires, read through the same
/// client it builds, plus a completed initial block download.
async fn bitcoin_outcome(rpc: &DynServerBitcoinRpc, network: Network) -> Outcome {
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
}

/// The client fedimintd builds for this backend, except that Bitcoin Core is
/// required on its own: its Esplora fallback does not make an MG ready.
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
    use mg_core::seat_process::BitcoindConfig;
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

    fn core_backend(url: String, password: &str) -> BitcoinBackend {
        BitcoinBackend::Bitcoind {
            primary: BitcoindConfig {
                url,
                username: "user".to_owned(),
                password: password.to_owned(),
            },
            esplora_fallback: None,
        }
    }

    fn core_rpc(url: String, password: &str) -> DynServerBitcoinRpc {
        bitcoin_rpc(&core_backend(url, password)).unwrap()
    }

    fn metrics_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(CHECK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// A seat metrics endpoint answering `status` with `body`; its port.
    async fn serve_metrics(status: StatusCode, body: String) -> u16 {
        let app = axum::Router::new().route(
            "/metrics",
            axum::routing::get(move || async move { (status, body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    /// The totals come from the seat's own exposition; an error status, an
    /// oversized body or a port nothing listens on reads as no reading.
    #[tokio::test]
    async fn a_seat_s_ordering_latency_is_read_from_its_metrics_port() {
        let http = metrics_client();
        let exposition = "fm_consensus_transaction_ordering_latency_seconds_sum 12.5\n\
            fm_consensus_transaction_ordering_latency_seconds_count 5\n";
        let port = serve_metrics(StatusCode::OK, exposition.to_owned()).await;
        assert_eq!(
            read_ordering_totals(&http, port).await,
            Some(OrderingTotals {
                sum_secs: 12.5,
                count: 5.0,
            })
        );
        let failing = serve_metrics(StatusCode::INTERNAL_SERVER_ERROR, exposition.to_owned()).await;
        assert_eq!(read_ordering_totals(&http, failing).await, None);
        let oversized = serve_metrics(
            StatusCode::OK,
            format!(
                "{exposition}{}",
                "# padding\n".repeat(MAX_GUARDIAN_METRICS_BODY_BYTES / 10 + 1)
            ),
        )
        .await;
        assert_eq!(read_ordering_totals(&http, oversized).await, None);
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(read_ordering_totals(&http, closed_port).await, None);
    }

    /// The deadline holds against a Core that accepts and never answers: the
    /// client blocks inside its async methods, so only the blocking-thread
    /// boundary lets the caller stop waiting.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_core_misses_the_deadline_instead_of_blocking_the_run() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let _held = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                held.push(listener.accept().await.unwrap());
            }
        });
        let started = Instant::now();
        let check = spawn_bitcoin_check(core_backend(url, "pass"), Network::Signet);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), check)
                .await
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_spawned_check_reads_a_ready_core() {
        let url = serve_core(Core {
            block_1: MUTINYNET_BLOCK_1,
            initial_block_download: false,
            fee_rate: true,
        })
        .await;
        assert_eq!(
            spawn_bitcoin_check(core_backend(url, "pass"), Network::Signet)
                .await
                .unwrap(),
            Outcome::Pass
        );
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
