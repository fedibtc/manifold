use std::{net::SocketAddr, path::PathBuf, str::FromStr as _};

use clap::Parser;
use fedi_decentralized_guardian_metrics_policy::{SOURCE_VERSION, SOURCE_VERSION_HASH};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;

pub(crate) const MAX_LOG_QUOTA_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(crate) const MAX_LOG_RETENTION_DAYS: u16 = 30;

/// Configuration whose numeric bounds and units have been validated once.
pub(crate) struct RuntimeConfig {
    /// Parsed trust environment.
    pub(crate) environment: ManifoldEnvironment,
    /// Validated sparse-metrics settings.
    pub(crate) metrics: MetricsRuntimeConfig,
    /// Safe-journal interval.
    pub(crate) log_cadence: std::time::Duration,
    /// Maximum concurrent journal target work.
    pub(crate) log_concurrency: std::num::NonZeroUsize,
    /// Global compressed archive byte quota.
    pub(crate) log_quota_bytes: std::num::NonZeroU64,
    /// UTC reception days retained.
    pub(crate) log_retention_days: std::num::NonZeroU16,
    /// Registrations admitted per source network prefix each minute.
    pub(crate) source_budget: u8,
    /// Defe-only direct route for deterministic local protocol validation.
    #[cfg(feature = "defe-test-support")]
    pub(crate) e2e_iroh_endpoint_addr: Option<fedi_iroh_rpc::iroh::EndpointAddr>,
    /// Defe-only accelerated worker cadence.
    #[cfg(feature = "defe-test-support")]
    pub(crate) e2e_poll_cadence: Option<std::time::Duration>,
    /// Defe-only explicit badge verifier.
    #[cfg(feature = "defe-test-support")]
    pub(crate) e2e_badge_verifier:
        Option<fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier>,
}

/// Metrics settings after all externally supplied bounds have been checked.
#[derive(Clone)]
pub(crate) struct MetricsRuntimeConfig {
    /// Minimum duration between attempts for one target.
    pub(crate) cadence: std::time::Duration,
    /// Maximum concurrent target polls.
    pub(crate) concurrency: std::num::NonZeroUsize,
    /// Two-cadence remote freshness threshold.
    pub(crate) stale_after: std::time::Duration,
    /// Exact expected `fm_app_start_ts` release.
    pub(crate) source_version: String,
    /// Exact expected `fm_app_start_ts` build hash.
    pub(crate) source_version_hash: String,
    /// Operator assertion that the deployed source contains both canonicalizers.
    pub(crate) canonical_method_labels: bool,
}
/// Cloud collector process configuration.
#[derive(Clone, Debug, Parser)]
#[command(name = "cloud-fman-telemetry")]
pub struct Args {
    /// Public registration listener.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_PUBLIC_BIND",
        default_value = "127.0.0.1:8175"
    )]
    pub public_bind: SocketAddr,
    /// Private health and readiness listener.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_PRIVATE_BIND",
        default_value = "127.0.0.1:8176"
    )]
    pub private_bind: SocketAddr,
    /// Confirm that deployment policy isolates a non-loopback private listener.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_PRIVATE_BIND_ISOLATED",
        default_value_t = false
    )]
    pub private_bind_isolated: bool,
    /// Exact externally visible HTTPS origin, without a trailing slash.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_PUBLIC_BASE_URL")]
    pub public_base_url: String,
    /// Persistent directory containing SQLite and the process lock.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_DATA_DIR")]
    pub data_dir: PathBuf,
    /// Read-only file containing exactly 32 raw encryption-key bytes.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_KEY_FILE")]
    pub key_file: PathBuf,
    /// Stable deployment key identifier stored beside ciphertext.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_KEY_ID")]
    pub key_id: String,
    /// Manifold trust environment.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_ENVIRONMENT")]
    pub environment: String,
    /// Registration lease duration in seconds.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LEASE_SECONDS",
        default_value_t = 3600
    )]
    pub lease_seconds: i64,
    /// Sparse guardian metrics polling cadence; only 15 or 30 minutes is supported.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_METRICS_POLL_SECONDS",
        default_value_t = 1800
    )]
    pub metrics_poll_seconds: u64,
    /// Maximum concurrent registered FMan metrics polls.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_METRICS_CONCURRENCY",
        default_value_t = 4
    )]
    pub metrics_concurrency: usize,
    /// Exact `fedimintd` release version admitted by the metrics inventory.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_METRICS_SOURCE_VERSION")]
    pub metrics_source_version: String,
    /// Exact `fedimintd` release hash admitted by the metrics inventory.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_METRICS_SOURCE_VERSION_HASH")]
    pub metrics_source_version_hash: String,
    /// Assert that the deployed source includes Fedimint PRs 9032 and 9033.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_CANONICAL_METHOD_LABELS",
        default_value_t = false
    )]
    pub canonical_method_labels: bool,
    /// Safe-journal polling cadence in seconds, independent of metrics cadence.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LOG_POLL_SECONDS",
        default_value_t = 300
    )]
    pub log_poll_seconds: u64,
    /// Maximum concurrent registered FMan journal polls.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LOG_CONCURRENCY",
        default_value_t = 4
    )]
    pub log_concurrency: usize,
    /// Maximum compressed archive bytes across all safe-journal streams.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LOG_QUOTA_BYTES",
        default_value_t = 10 * 1024 * 1024 * 1024
    )]
    pub log_quota_bytes: u64,
    /// Maximum archive reception-day retention.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LOG_RETENTION_DAYS",
        default_value_t = 30
    )]
    pub log_retention_days: u16,
    /// Registrations admitted per source network prefix each minute.
    ///
    /// The default suits one Fleet Manager per operator network. Raise it only
    /// where the deployment knowingly places several Fleet Managers behind one
    /// egress address, because the receiver cannot tell them apart.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_SOURCE_BUDGET", default_value_t = 4)]
    pub source_budget: u8,
    /// Proxy networks trusted to supply Forwarded or X-Forwarded-For client addresses.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_TRUSTED_PROXY",
        value_delimiter = ','
    )]
    pub trusted_proxies: Vec<ipnet::IpNet>,
    /// Defe-only direct Iroh address encoded as JSON.
    #[cfg(feature = "defe-test-support")]
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_E2E_IROH_ENDPOINT_ADDR", hide = true)]
    pub e2e_iroh_endpoint_addr: Option<String>,
    /// Defe-only worker cadence used by the real-daemon integration test.
    #[cfg(feature = "defe-test-support")]
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_E2E_POLL_MILLIS", hide = true)]
    pub e2e_poll_millis: Option<u64>,
    /// Defe-only trusted issuer identity.
    #[cfg(feature = "defe-test-support")]
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_E2E_ISSUER", hide = true)]
    pub e2e_issuer: Option<String>,
    /// Defe-only authority and revocation relay.
    #[cfg(feature = "defe-test-support")]
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_E2E_NOSTR_RELAY", hide = true)]
    pub e2e_nostr_relay: Option<String>,
}

impl Args {
    /// Validate configuration and parse its trust environment.
    pub(crate) fn validate(&self) -> Result<RuntimeConfig, String> {
        #[cfg(feature = "defe-test-support")]
        let e2e = self.e2e_config()?;
        let base_url = url::Url::parse(&self.public_base_url)
            .map_err(|_| "public base URL must be an absolute URL")?;
        if base_url.scheme() != "https"
            || base_url.cannot_be_a_base()
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.path() != "/"
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || self.public_base_url.ends_with('/')
        {
            return Err("public base URL must be an HTTPS origin without trailing slash".into());
        }
        if self.public_bind == self.private_bind {
            return Err("public and private listeners must differ".into());
        }
        if !self.private_bind.ip().is_loopback() && !self.private_bind_isolated {
            return Err("a non-loopback private listener requires --private-bind-isolated".into());
        }
        if self.key_id.is_empty() || self.key_id.len() > 128 || self.lease_seconds <= 60 {
            return Err("invalid key id or registration lease".into());
        }
        if !matches!(self.metrics_poll_seconds, 900 | 1800) {
            return Err("metrics cadence must be exactly 900 or 1800 seconds".into());
        }
        let metrics_concurrency = std::num::NonZeroUsize::new(self.metrics_concurrency)
            .filter(|value| value.get() <= 32)
            .ok_or("metrics concurrency must be in 1..=32")?;
        if self.metrics_source_version.is_empty()
            || self.metrics_source_version.len() > 128
            || (self.environment == "production" && self.metrics_source_version == "REPLACE_ME")
        {
            return Err("metrics source version must contain 1..=128 bytes".into());
        }
        if self.metrics_source_version_hash.is_empty()
            || self.metrics_source_version_hash.len() > 128
            || (self.environment == "production"
                && self.metrics_source_version_hash == "REPLACE_ME")
        {
            return Err("metrics source hash must contain 1..=128 bytes".into());
        }
        if !cfg!(any(test, feature = "defe-test-support"))
            && (self.metrics_source_version != SOURCE_VERSION
                || self.metrics_source_version_hash != SOURCE_VERSION_HASH
                || self.canonical_method_labels)
        {
            return Err("metrics source profile does not match the compiled policy".into());
        }
        if !(1..=64).contains(&self.source_budget) {
            return Err("source registration budget must be in 1..=64".into());
        }
        if !(10..=86_400).contains(&self.log_poll_seconds)
            || !(1..=32).contains(&self.log_concurrency)
            || !(1024 * 1024..=MAX_LOG_QUOTA_BYTES).contains(&self.log_quota_bytes)
            || !(1..=MAX_LOG_RETENTION_DAYS).contains(&self.log_retention_days)
        {
            return Err("invalid key id, lease, or safe-journal bounds".into());
        }
        Ok(RuntimeConfig {
            environment: ManifoldEnvironment::from_str(&self.environment)
                .map_err(|_| "invalid environment")?,
            metrics: MetricsRuntimeConfig {
                cadence: std::time::Duration::from_secs(self.metrics_poll_seconds),
                concurrency: metrics_concurrency,
                stale_after: std::time::Duration::from_secs(
                    self.metrics_poll_seconds.saturating_mul(2),
                ),
                source_version: self.metrics_source_version.clone(),
                source_version_hash: self.metrics_source_version_hash.clone(),
                canonical_method_labels: self.canonical_method_labels,
            },
            log_cadence: std::time::Duration::from_secs(self.log_poll_seconds),
            log_concurrency: std::num::NonZeroUsize::new(self.log_concurrency)
                .ok_or("invalid safe-journal concurrency")?,
            log_quota_bytes: std::num::NonZeroU64::new(self.log_quota_bytes)
                .ok_or("invalid safe-journal quota")?,
            log_retention_days: std::num::NonZeroU16::new(self.log_retention_days)
                .ok_or("invalid safe-journal retention")?,
            source_budget: self.source_budget,
            #[cfg(feature = "defe-test-support")]
            e2e_iroh_endpoint_addr: e2e.as_ref().map(|(address, _)| address.clone()),
            #[cfg(feature = "defe-test-support")]
            e2e_poll_cadence: e2e.map(|(_, cadence)| cadence),
            #[cfg(feature = "defe-test-support")]
            e2e_badge_verifier: self.e2e_badge_verifier()?,
        })
    }

    #[cfg(feature = "defe-test-support")]
    fn e2e_badge_verifier(
        &self,
    ) -> Result<Option<fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier>, String> {
        match (&self.e2e_issuer, &self.e2e_nostr_relay) {
            (None, None) => Ok(None),
            (Some(issuer), Some(relay)) => {
                self.require_defe_socket()?;
                Ok(Some(
                    fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier::new_for_test(
                        [issuer.parse().map_err(|_| "invalid Defe issuer")?],
                        [relay.parse().map_err(|_| "invalid Defe relay")?],
                        1,
                    )
                    .map_err(|_| "invalid Defe badge verifier")?,
                ))
            }
            _ => Err("Defe issuer and relay must be configured together".into()),
        }
    }

    #[cfg(feature = "defe-test-support")]
    fn require_defe_socket(&self) -> Result<(), String> {
        let socket = std::env::var_os("DEV_DEFE_SOCKET_PATH")
            .ok_or("E2E configuration is available only under Defe")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt as _;
            if !std::fs::metadata(socket)
                .map(|metadata| metadata.file_type().is_socket())
                .unwrap_or(false)
            {
                return Err("Defe socket is not an active Unix socket".into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = socket;
            Err("E2E configuration requires a Unix Defe socket".into())
        }
    }

    #[cfg(feature = "defe-test-support")]
    fn e2e_config(
        &self,
    ) -> Result<Option<(fedi_iroh_rpc::iroh::EndpointAddr, std::time::Duration)>, String> {
        let Some(encoded) = &self.e2e_iroh_endpoint_addr else {
            if self.e2e_poll_millis.is_some() {
                return Err("Defe cadence requires a direct Iroh address".into());
            }
            return Ok(None);
        };
        self.require_defe_socket()?;
        let address = serde_json::from_str(encoded)
            .map_err(|_| "invalid Defe direct Iroh endpoint address")?;
        let millis = self
            .e2e_poll_millis
            .filter(|millis| (50..=1000).contains(millis))
            .ok_or("Defe poll cadence must be in 50..=1000 milliseconds")?;
        Ok(Some((address, std::time::Duration::from_millis(millis))))
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
