use std::{net::SocketAddr, path::PathBuf, str::FromStr as _};

use clap::Parser;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;

pub(crate) const MAX_LOG_QUOTA_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(crate) const MAX_LOG_RETENTION_DAYS: u16 = 30;
/// Most targets one collector stores; archive recovery bounds assume it.
pub(crate) const MAX_TARGETS: u16 = 4096;

/// Which registrations a collector admits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum AdmissionMode {
    /// Only MGs whose Holder authorization verifies.
    Badged,
    /// Any NIP-98 signer, for MGs without a badge, under creation and
    /// archive limits. Run as a separate deployment.
    Unbadged,
}

/// Validated admission policy.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Admission {
    Badged,
    Unbadged(UnbadgedLimits),
}

/// Limits that bound what free identities can take from an unbadged collector.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnbadgedLimits {
    /// Targets the store may hold.
    pub(crate) max_targets: std::num::NonZeroU16,
    /// Targets created in any trailing hour.
    pub(crate) new_targets_per_hour: std::num::NonZeroU16,
    /// Targets created from one source network prefix in any trailing hour.
    pub(crate) source_new_targets_per_hour: std::num::NonZeroU8,
    /// Compressed archive bytes one target may hold within retention.
    pub(crate) log_target_quota_bytes: std::num::NonZeroU64,
}

/// Configuration whose numeric bounds and units have been validated once.
pub(crate) struct RuntimeConfig {
    /// Parsed trust environment.
    pub(crate) environment: ManifoldEnvironment,
    /// Admission mode and its limits.
    pub(crate) admission: Admission,
    /// Validated sparse-metrics settings.
    pub(crate) metrics: MetricsRuntimeConfig,
    /// Includes the MG journal and retained/decommissioned seat journals.
    pub(crate) max_journals_per_target: u16,
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
    /// Maximum concurrent registered MG metrics polls.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_METRICS_CONCURRENCY",
        default_value_t = 4
    )]
    pub metrics_concurrency: usize,
    /// Safe-journal polling cadence in seconds, independent of metrics cadence.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_LOG_POLL_SECONDS",
        default_value_t = 300
    )]
    pub log_poll_seconds: u64,
    /// Maximum concurrent registered MG journal polls.
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
    /// The default suits one Manifold Guardian per operator network. Raise it only
    /// where the deployment knowingly places several Manifold Guardians behind one
    /// egress address, because the receiver cannot tell them apart.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_SOURCE_BUDGET", default_value_t = 4)]
    pub source_budget: u8,
    /// Admission mode. An unbadged collector admits any NIP-98 signer and
    /// must be a separate deployment with its own data directory and key.
    #[arg(
        long,
        env = "CLOUD_FMAN_TELEMETRY_ADMISSION",
        value_enum,
        default_value_t = AdmissionMode::Badged
    )]
    pub admission: AdmissionMode,
    /// Unbadged only: most targets the collector stores, in 1..=4096.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_MAX_TARGETS")]
    pub max_targets: Option<u16>,
    /// Unbadged only: most targets created in any trailing hour.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_NEW_TARGETS_PER_HOUR")]
    pub new_targets_per_hour: Option<u16>,
    /// Unbadged only: most targets created from one source network prefix in
    /// any trailing hour.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_SOURCE_NEW_TARGETS_PER_HOUR")]
    pub source_new_targets_per_hour: Option<u8>,
    /// Unbadged only: compressed archive bytes one target may hold.
    #[arg(long, env = "CLOUD_FMAN_TELEMETRY_LOG_TARGET_QUOTA_BYTES")]
    pub log_target_quota_bytes: Option<u64>,
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
        let environment =
            ManifoldEnvironment::from_str(&self.environment).map_err(|_| "invalid environment")?;
        let admission = self.admission()?;
        Ok(RuntimeConfig {
            environment,
            admission,
            // Repeated staging ceremonies retain many decommissioned seats.
            max_journals_per_target: if environment == ManifoldEnvironment::Staging {
                1_000
            } else {
                32
            },
            metrics: MetricsRuntimeConfig {
                cadence: std::time::Duration::from_secs(self.metrics_poll_seconds),
                concurrency: metrics_concurrency,
                stale_after: std::time::Duration::from_secs(
                    self.metrics_poll_seconds.saturating_mul(2),
                ),
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

    fn admission(&self) -> Result<Admission, String> {
        let limits = (
            self.max_targets,
            self.new_targets_per_hour,
            self.source_new_targets_per_hour,
            self.log_target_quota_bytes,
        );
        match (self.admission, limits) {
            (AdmissionMode::Badged, (None, None, None, None)) => Ok(Admission::Badged),
            (AdmissionMode::Badged, _) => {
                Err("target creation and per-target archive limits apply only to unbadged admission".into())
            }
            (
                AdmissionMode::Unbadged,
                (
                    Some(max_targets),
                    Some(new_targets_per_hour),
                    Some(source_new_targets_per_hour),
                    Some(log_target_quota_bytes),
                ),
            ) => Ok(Admission::Unbadged(UnbadgedLimits {
                max_targets: std::num::NonZeroU16::new(max_targets)
                    .filter(|value| value.get() <= MAX_TARGETS)
                    .ok_or("max targets must be in 1..=4096")?,
                new_targets_per_hour: std::num::NonZeroU16::new(new_targets_per_hour)
                    .ok_or("new targets per hour must be positive")?,
                source_new_targets_per_hour: std::num::NonZeroU8::new(
                    source_new_targets_per_hour,
                )
                .ok_or("source new targets per hour must be positive")?,
                log_target_quota_bytes: std::num::NonZeroU64::new(log_target_quota_bytes)
                    .filter(|value| value.get() <= self.log_quota_bytes)
                    .ok_or("per-target archive quota must be in 1..=log quota")?,
            })),
            (AdmissionMode::Unbadged, _) => Err(
                "unbadged admission requires max targets, new targets per hour, source new targets per hour and a per-target archive quota".into(),
            ),
        }
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
