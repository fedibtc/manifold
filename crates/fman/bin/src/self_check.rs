//! Diagnostic adapter: only daemon-owned configured targets, fixed read-only requests.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fedimint_core::bitcoin::{self, Network};
use fman_core::seat_process::{BitcoinBackend, SeatProcessConfig};
use fman_core::self_check::{
    Check, CheckId as Id, CheckStatus as Status, ConnectivityChecks, ReasonCode as Reason,
};
use iroh::{Endpoint, Watcher as _};
use iroh_dns::dns::DnsResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

const DNS_LIMIT: Duration = Duration::from_secs(3);
const HTTP_LIMIT: Duration = Duration::from_secs(5);

struct EndpointResolver(DnsResolver);

impl Resolve for EndpointResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let dns = self.0.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = resolve_addresses(&dns, &host).await;
            if addresses.is_empty() {
                return Err::<Addrs, Box<dyn std::error::Error + Send + Sync>>(
                    "name resolution failed".into(),
                );
            }
            Ok(Box::new(addresses.into_iter().map(|ip| SocketAddr::new(ip, 0))) as Addrs)
        })
    }
}

async fn resolve_addresses(dns: &DnsResolver, host: &str) -> Vec<std::net::IpAddr> {
    let a = async {
        dns.lookup_ipv4(host, DNS_LIMIT)
            .await
            .map(|iter| iter.collect())
            .unwrap_or_default()
    };
    let aaaa = async {
        dns.lookup_ipv6(host, DNS_LIMIT)
            .await
            .map(|iter| iter.collect())
            .unwrap_or_default()
    };
    collect_families(a, aaaa, DNS_LIMIT).await
}

async fn collect_families<A, B>(a: A, aaaa: B, limit: Duration) -> Vec<IpAddr>
where
    A: Future<Output = Vec<IpAddr>>,
    B: Future<Output = Vec<IpAddr>>,
{
    tokio::pin!(a, aaaa);
    let deadline = tokio::time::Instant::now() + limit;
    // Retain one completed family even if the other stalls. HTTP needs both
    // families when available: choosing only the first can strand a dual-stack
    // name on a single-stack host.
    match tokio::time::timeout_at(deadline, async {
        tokio::select! {
            result = &mut a => (true, result),
            result = &mut aaaa => (false, result),
        }
    })
    .await
    {
        Ok((true, mut addresses)) => {
            if let Ok(other) = tokio::time::timeout_at(deadline, aaaa).await {
                addresses.extend(other);
            }
            addresses
        }
        Ok((false, mut addresses)) => {
            if let Ok(other) = tokio::time::timeout_at(deadline, a).await {
                addresses.extend(other);
            }
            addresses
        }
        Err(_) => Vec::new(),
    }
}

/// One daemon-side adapter, built from the effective child configuration and live endpoint.
pub struct DaemonConnectivityChecks {
    endpoint: Endpoint,
    resolver: Option<DnsResolver>,
    client: Option<reqwest::Client>,
    discovery: Url,
    backend: BitcoinBackend,
    network: Network,
    relay_disabled: bool,
    proxy_configured: bool,
}

impl DaemonConnectivityChecks {
    /// Snapshot configured authority and proxy policy once, without logging it.
    pub fn new(config: &SeatProcessConfig, endpoint: Endpoint, relay_disabled: bool) -> Self {
        let resolver = endpoint.dns_resolver().ok().cloned();
        let proxy_configured = [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
        let client = resolver.as_ref().and_then(|dns| {
            reqwest::Client::builder()
                .dns_resolver(Arc::new(EndpointResolver(dns.clone())))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .http1_only()
                .timeout(HTTP_LIMIT)
                .build()
                .ok()
        });
        Self {
            endpoint,
            resolver,
            client,
            discovery: config.iroh_dns.clone().to_unsafe(),
            backend: config.bitcoin_backend.clone(),
            network: config.bitcoin_network,
            relay_disabled,
            proxy_configured,
        }
    }

    fn relay(&self) -> Check {
        if self.relay_disabled {
            return check(Id::FmanRelay, Status::NotApplicable, Reason::Disabled);
        }
        let relay = self.endpoint.home_relay_status().get();
        if relay.iter().any(|status| status.is_connected()) {
            check(Id::FmanRelay, Status::Pass, Reason::Connected)
        } else if relay.is_empty() {
            check(Id::FmanRelay, Status::Unknown, Reason::NotSelected)
        } else {
            check(Id::FmanRelay, Status::Warning, Reason::Disconnected)
        }
    }

    async fn dns(&self, id: Id, url: Option<&Url>) -> Check {
        let Some(host) = url.and_then(Url::host_str) else {
            return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
        };
        if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
            return check(id, Status::NotApplicable, Reason::NumericHost);
        }
        let Some(dns) = &self.resolver else {
            return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
        };
        if resolve_addresses(dns, host).await.is_empty() {
            check(id, Status::Warning, Reason::NoRecords)
        } else {
            check(id, Status::Pass, Reason::Reached)
        }
    }

    async fn discovery_https(&self) -> Check {
        let id = Id::DiscoveryHttps;
        if self.discovery.scheme() != "https" {
            return check(id, Status::NotApplicable, Reason::NotHttps);
        }
        if !safe_url(&self.discovery) {
            return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
        }
        if self.proxy_configured {
            return check(id, Status::Unknown, Reason::UnsupportedProxyConfiguration);
        }
        let Some(client) = &self.client else {
            return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
        };
        match client.head(self.discovery.clone()).send().await {
            Ok(response)
                if response.status().is_server_error() || response.status().as_u16() == 429 =>
            {
                check(id, Status::Warning, Reason::HttpService)
            }
            Ok(response)
                if response.status().as_u16() == 401 || response.status().as_u16() == 403 =>
            {
                check(id, Status::Warning, Reason::HttpAccess)
            }
            Ok(_) => check(id, Status::Pass, Reason::Reached),
            Err(error) if error.is_timeout() => check(id, Status::Warning, Reason::Timeout),
            Err(_) => check(id, Status::Warning, Reason::Unreachable),
        }
    }

    async fn bitcoin(&self, id: Id, backend: &BitcoinBackend) -> Check {
        let Some(client) = &self.client else {
            return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
        };
        if self.proxy_configured {
            return check(id, Status::Unknown, Reason::UnsupportedProxyConfiguration);
        }
        let outcome = match backend {
            BitcoinBackend::Bitcoind { primary, .. } => {
                let Ok(url) = Url::parse(&primary.url) else {
                    return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
                };
                if !safe_url(&url) || !matches!(url.scheme(), "http" | "https") {
                    return check(id, Status::Unknown, Reason::UnsupportedConfiguration);
                }
                let response = client.post(url)
                    .basic_auth(&primary.username, Some(&primary.password))
                    .json(&serde_json::json!({"jsonrpc":"1.0","id":"fman-self-check","method":"getblockchaininfo","params":[]}))
                    .send().await;
                match response {
                    Ok(response) => {
                        let status = response.status();
                        if status.as_u16() == 401 || status.as_u16() == 403 {
                            Reason::HttpAccess
                        } else {
                            match bounded_body(response, 16 * 1024).await {
                                Ok(bytes) => core_response(status, &bytes, self.network),
                                Err(reason) => reason,
                            }
                        }
                    }
                    Err(error) if error.is_timeout() => Reason::Timeout,
                    Err(_) => Reason::Unreachable,
                }
            }
            BitcoinBackend::Esplora(base) => self.esplora(client, base).await,
        };
        let status = match outcome {
            Reason::Reached => Status::Pass,
            Reason::Synchronizing | Reason::Starting => Status::Warning,
            Reason::UnsupportedConfiguration | Reason::UnsupportedProxyConfiguration => {
                Status::Unknown
            }
            Reason::WrongNetwork
            | Reason::HttpAccess
            | Reason::InvalidResponse
            | Reason::RequestRejected => {
                if matches!(id, Id::BitcoinFallback) {
                    Status::Warning
                } else {
                    Status::Failure
                }
            }
            _ => Status::Warning,
        };
        check(id, status, outcome)
    }

    async fn esplora(&self, client: &reqwest::Client, base: &Url) -> Reason {
        if !safe_url(base) || !matches!(base.scheme(), "http" | "https") {
            return Reason::UnsupportedConfiguration;
        }
        let mut prefix = base.clone();
        let path = format!("{}/", prefix.path().trim_end_matches('/'));
        prefix.set_path(&path);
        let Ok(tip) = prefix.join("blocks/tip/height") else {
            return Reason::UnsupportedConfiguration;
        };
        let Ok(genesis) = prefix.join("block-height/0") else {
            return Reason::UnsupportedConfiguration;
        };
        let Ok(tip) = client.get(tip).send().await else {
            return Reason::Unreachable;
        };
        if !tip.status().is_success() {
            return Reason::RequestRejected;
        }
        let Ok(bytes) = bounded_body(tip, 128).await else {
            return Reason::InvalidResponse;
        };
        let Ok(height) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .ok_or(())
        else {
            return Reason::InvalidResponse;
        };
        let _ = height;
        let Ok(genesis) = client.get(genesis).send().await else {
            return Reason::Unreachable;
        };
        if !genesis.status().is_success() {
            return Reason::RequestRejected;
        }
        let Ok(bytes) = bounded_body(genesis, 128).await else {
            return Reason::InvalidResponse;
        };
        let expected = bitcoin::constants::genesis_block(self.network)
            .block_hash()
            .to_string();
        match std::str::from_utf8(&bytes) {
            Ok(hash) if hash.trim() == expected => Reason::Reached,
            Ok(hash) if hash.trim().parse::<bitcoin::BlockHash>().is_ok() => Reason::WrongNetwork,
            _ => Reason::InvalidResponse,
        }
    }
}

#[async_trait::async_trait]
impl ConnectivityChecks for DaemonConnectivityChecks {
    async fn run(&self, slots: Arc<Mutex<[Check; 6]>>) {
        let primary = match &self.backend {
            BitcoinBackend::Esplora(url) => Some(url.clone()),
            BitcoinBackend::Bitcoind { primary, .. } => Url::parse(&primary.url).ok(),
        };
        let store = |index, result| slots.lock().expect("slots mutex")[index] = result;
        let discovery_dns =
            async { store(0, self.dns(Id::DiscoveryDns, Some(&self.discovery)).await) };
        let discovery_https = async { store(1, self.discovery_https().await) };
        let relay = async { store(2, self.relay()) };
        tokio::join!(discovery_dns, discovery_https, relay);

        let bitcoin_dns = async { store(3, self.dns(Id::BitcoinDns, primary.as_ref()).await) };
        let bitcoin_primary = async {
            store(
                4,
                tokio::time::timeout(HTTP_LIMIT, self.bitcoin(Id::BitcoinPrimary, &self.backend))
                    .await
                    .unwrap_or_else(|_| {
                        check(Id::BitcoinPrimary, Status::Warning, Reason::Timeout)
                    }),
            );
        };
        let bitcoin_fallback = async {
            let result = match &self.backend {
                BitcoinBackend::Bitcoind {
                    esplora_fallback: Some(url),
                    ..
                } => tokio::time::timeout(
                    HTTP_LIMIT,
                    self.bitcoin(Id::BitcoinFallback, &BitcoinBackend::Esplora(url.clone())),
                )
                .await
                .unwrap_or_else(|_| check(Id::BitcoinFallback, Status::Warning, Reason::Timeout)),
                _ => check(
                    Id::BitcoinFallback,
                    Status::NotApplicable,
                    Reason::NotConfigured,
                ),
            };
            store(5, result);
        };
        tokio::join!(bitcoin_dns, bitcoin_primary, bitcoin_fallback);
    }
}

fn check(check_id: Id, status: Status, reason_code: Reason) -> Check {
    Check::new(check_id, status, reason_code)
}

fn safe_url(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

async fn bounded_body(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, Reason> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Reason::InvalidResponse)?
    {
        if chunk.len() > max - bytes.len() {
            return Err(Reason::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn core_response(status: reqwest::StatusCode, bytes: &[u8], network: Network) -> Reason {
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Reason::InvalidResponse;
    };
    if body.get("id").and_then(|v| v.as_str()) != Some("fman-self-check") {
        return Reason::InvalidResponse;
    }
    if let Some(error) = body.get("error").filter(|v| !v.is_null()) {
        return if error.get("code").and_then(|v| v.as_i64()) == Some(-28) {
            Reason::Starting
        } else {
            Reason::RequestRejected
        };
    }
    if !status.is_success() {
        return Reason::RequestRejected;
    }
    let Some(result) = body.get("result") else {
        return Reason::InvalidResponse;
    };
    if result.get("chain").and_then(|v| v.as_str()) != Some(network.to_core_arg()) {
        return Reason::WrongNetwork;
    }
    match result.get("initialblockdownload").and_then(|v| v.as_bool()) {
        Some(true) => Reason::Synchronizing,
        Some(false) => Reason::Reached,
        None => Reason::InvalidResponse,
    }
}

#[cfg(test)]
#[path = "self_check/tests.rs"]
mod tests;
