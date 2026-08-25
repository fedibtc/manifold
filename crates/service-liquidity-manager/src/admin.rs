//! Private Operator Admin API messages.

use crate::{
    AllocationStatus, AttestationPayload, AttestationPayloadId, BackupArchive, BitcoinNetwork,
    CapacityMode, DurationSecs, FederationId, GatewayId, GatewayName, ItemAllocationStatus, ItemId,
    ListResponse, PageRequest, ProtocolVersion, ProviderDisplay, ProviderPolicy, Pubkey,
    RpcDiscoveryHint, RpcEndpointAddress, RpcEndpointId, RpcProtocolName, RpcTransport, Sats,
    SecretString, Signed, SourceType, TimeRange, Timestamp, Url, VerificationSummary,
    WalletOperationId,
};
use serde::{Deserialize, Serialize};

/// Request current service health.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetHealthRequest;

/// Health response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetHealthResponse {
    /// Overall health status.
    pub overall_status: HealthStatus,

    /// Boot mode the reporting daemon is running in.
    pub mode: HealthMode,

    /// Component health entries.
    pub components: Vec<ComponentHealth>,

    /// Response generation time.
    pub observed_at: Timestamp,
}

impl GetHealthResponse {
    /// Projects this response onto the fields the unauthenticated `GET /health`
    /// route may disclose.
    ///
    /// The rule is deliberately blunt: statuses and the boot mode cross the
    /// boundary, and no free text does. Every `detail` is dropped rather than
    /// filtered, because the details are formatted from the daemon's own
    /// configuration and observations — database path, bind addresses, node id,
    /// auth and verification modes, wallet balance — and an allowlist over that
    /// set has to be re-audited every time a caller appends one more field to a
    /// format string.
    ///
    /// Both structs below are rebuilt field by field with no `..` rest pattern,
    /// so a new field on either one fails to compile here until somebody decides
    /// which side of the boundary it belongs on.
    pub fn redacted_for_public(self) -> Self {
        Self {
            overall_status: self.overall_status,
            mode: self.mode,
            components: self
                .components
                .into_iter()
                .map(|component| ComponentHealth {
                    component: component.component,
                    status: component.status,
                    detail: None,
                    observed_at: component.observed_at,
                })
                .collect(),
            observed_at: self.observed_at,
        }
    }
}

/// What a client can reach on the reporting process right now.
///
/// This is the one operational fact the unauthenticated route has to carry.
/// Restore-only mode routes a much smaller verb set, and during a live restore
/// swap every runtime-backed route answers `unavailable` — including both
/// authenticated health verbs — so a client that cannot read this from
/// `GET /health` cannot read it anywhere. Reporting it as a typed variant, and
/// not as prose inside `detail`, is what lets the unauthenticated projection
/// drop free text without losing the signal.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HealthMode {
    /// Normal operation: the full Admin API is routed.
    #[strum(serialize = "normal")]
    Normal,

    /// Restore-only boot: only health, `inspect_backup`, and `restore_backup`.
    #[strum(serialize = "restore")]
    Restore,

    /// A live restore is rebuilding the runtime against restored state.
    #[strum(serialize = "reloading")]
    Reloading,

    /// No runtime generation is installed.
    #[strum(serialize = "no_runtime")]
    NoRuntime,
}

/// Component health entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component being checked.
    pub component: HealthComponent,

    /// Health status.
    pub status: HealthStatus,

    /// Optional operator-readable detail.
    pub detail: Option<String>,

    /// Last observed timestamp.
    pub observed_at: Timestamp,
}

/// Health component.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HealthComponent {
    /// FLIP daemon process.
    #[strum(serialize = "daemon")]
    Daemon,

    /// Package wrapper or platform integration.
    #[strum(serialize = "package")]
    Package,

    /// Bundled operator web client.
    #[strum(serialize = "web_client")]
    WebClient,

    /// Provider funds wallet.
    #[strum(serialize = "wallet")]
    Wallet,

    /// SQLite database.
    #[strum(serialize = "database")]
    Database,

    /// Nostr relays.
    #[strum(serialize = "relays")]
    Relays,

    /// Operator Admin API.
    #[strum(serialize = "admin_api")]
    AdminApi,

    /// Public Liquidity API.
    #[strum(serialize = "public_liquidity_api")]
    PublicLiquidityApi,

    /// External gateway dependency.
    #[strum(serialize = "gateway")]
    Gateway,

    /// External chain observer dependency, backed by Esplora or Bitcoin Core RPC.
    #[strum(serialize = "chain_observer")]
    ChainObserver,

    /// Periodic allocation, wallet-sync, and advertisement workers. Their
    /// errors are retried rather than fatal, so this is how a worker that is
    /// failing every pass becomes visible without reading logs.
    #[strum(serialize = "background_workers")]
    BackgroundWorkers,
}

/// Health status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Component is healthy.
    #[strum(serialize = "healthy")]
    Healthy,

    /// Component has a warning.
    #[strum(serialize = "warning")]
    Warning,

    /// Component is unhealthy.
    #[strum(serialize = "unhealthy")]
    Unhealthy,

    /// Component status is unknown.
    #[strum(serialize = "unknown")]
    Unknown,
}

/// Request setup state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetSetupStateRequest;

/// Setup state response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetSetupStateResponse {
    /// Setup status.
    pub status: SetupStatus,

    /// Current non-secret setup config view.
    pub config: Option<SetupConfigView>,

    /// Required fields not yet configured.
    pub missing_fields: Vec<String>,

    /// Latest validation summary.
    pub validation: Option<SetupValidationSummary>,
}

/// Apply first-run setup config.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplySetupConfigRequest {
    /// Complete setup config.
    pub config: SetupConfig,
}

/// Setup config apply response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApplySetupConfigResponse {
    /// Setup status after applying config.
    pub status: SetupStatus,

    /// Validation summary.
    pub validation: SetupValidationSummary,
}

/// Validate current setup config or a candidate replacement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidateSetupRequest {
    /// Candidate config to validate; absent means validate current config.
    pub candidate_config: Option<SetupConfig>,
}

/// Setup validation response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidateSetupResponse {
    /// Validation summary.
    pub validation: SetupValidationSummary,
}

/// Setup status.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SetupStatus {
    /// First-run setup is not complete.
    #[strum(serialize = "not_configured")]
    NotConfigured,

    /// Required config exists but validation has not passed.
    #[strum(serialize = "pending_validation")]
    PendingValidation,

    /// Setup completed and dependencies validated.
    #[strum(serialize = "ready")]
    Ready,
}

/// Full setup config accepted by the admin command API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetupConfig {
    /// Configured Bitcoin network.
    pub network: BitcoinNetwork,

    /// Gatewayd used for gateway/LN allocation and provider wallet operations.
    pub gateway: GatewayConfig,

    /// Chain observer used to reconcile gatewayd on-chain sends.
    pub chain_observer: ChainObserverConfig,

    /// Nostr relays.
    pub relays: Vec<Url>,

    /// Capacity configuration.
    pub capacity: CapacityConfig,

    /// Funding reserve and confirmation-depth policy.
    pub funding_policy: FundingPolicyConfig,

    /// Replenishment thresholds.
    pub replenishment: ReplenishmentConfig,

    /// Public endpoint config to advertise after validation.
    pub advertised_endpoint: RpcEndpointConfig,

    /// Advertisement publication config.
    pub advertisement: AdvertisementConfig,

    /// Optional operator-readable provider metadata.
    pub provider_display: Option<ProviderDisplay>,

    /// Provider policy.
    pub policy: ProviderPolicy,
}

/// Non-secret setup config view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetupConfigView {
    /// Configured Bitcoin network.
    pub network: BitcoinNetwork,

    /// Gatewayd config view.
    pub gateway: GatewayConfigView,

    /// Chain observer config view.
    pub chain_observer: ChainObserverConfigView,

    /// Nostr relays.
    pub relays: Vec<Url>,

    /// Capacity configuration.
    pub capacity: CapacityConfig,

    /// Funding reserve and confirmation-depth policy.
    pub funding_policy: FundingPolicyConfig,

    /// Replenishment thresholds.
    pub replenishment: ReplenishmentConfig,

    /// Public endpoint config.
    pub advertised_endpoint: RpcEndpointConfig,

    /// Advertisement publication config.
    pub advertisement: AdvertisementConfig,

    /// Optional operator-readable provider metadata.
    pub provider_display: Option<ProviderDisplay>,

    /// Provider policy.
    pub policy: ProviderPolicy,

    /// Installed attestation payload summary.
    pub attestation_summary: AttestationSummary,
}

/// Chain observer config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChainObserverConfig {
    /// Chain observer backend.
    pub backend: ChainObserverBackend,
}

/// Chain observer backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChainObserverBackend {
    /// Esplora HTTP API.
    Esplora {
        /// Esplora base URL.
        url: Url,
    },

    /// Bitcoin Core RPC.
    Bitcoind {
        /// Bitcoin Core RPC URL.
        url: Url,

        /// Optional RPC username.
        username: Option<String>,
    },
}

/// Non-secret chain observer config view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChainObserverConfigView {
    /// Chain observer backend type.
    pub backend: ChainObserverBackendView,
}

/// Non-secret chain observer backend view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChainObserverBackendView {
    /// Esplora HTTP API.
    Esplora {
        /// Esplora base URL.
        url: Url,
    },

    /// Bitcoin Core RPC.
    Bitcoind {
        /// Bitcoin Core RPC URL.
        url: Url,

        /// Optional RPC username.
        username: Option<String>,

        /// Whether an RPC password is configured.
        has_password: bool,
    },
}

/// Gateway config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Gateway id, when restoring or replacing the configured gateway.
    pub gateway_id: Option<GatewayId>,

    /// Gateway display name.
    pub gateway_name: GatewayName,

    /// Gateway admin URL.
    pub admin_url: String,

    /// Gateway identity metadata needed for allocations.
    pub identity_metadata: Vec<(String, String)>,
}

/// Non-secret gateway config view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfigView {
    /// Gateway id.
    pub gateway_id: GatewayId,

    /// Gateway display name.
    pub gateway_name: GatewayName,

    /// Gateway admin URL.
    pub admin_url: String,

    /// Whether admin credentials are configured.
    pub has_admin_credential: bool,

    /// Gateway identity metadata needed for allocations.
    pub identity_metadata: Vec<(String, String)>,
}

/// Capacity configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapacityConfig {
    /// Capacity configuration mode.
    pub mode: CapacityMode,

    /// Explicit cap when mode is `ExplicitCap`.
    pub explicit_cap: Option<Sats>,

    /// Supported source types.
    pub supported_sources: Vec<SourceType>,
}

/// Funding reserve and confirmation-depth policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FundingPolicyConfig {
    /// Fee reserve subtracted from available balance and reserved on top of
    /// each accepted allocation item's committed amount.
    pub fee_reserve: Sats,

    /// Confirmation depth required before treating provider-wallet operations
    /// as settled.
    pub confirmations: u32,

    /// Minimum provider fee rate for stability-pool `deposit_to_provide`, in parts per billion.
    #[serde(default)]
    pub stability_pool_min_fee_rate_ppb: u64,

    /// How long a wallet send may stay `in_doubt` without resolving evidence
    /// before it is escalated to `manual_review_required`.
    ///
    /// Measured from submission. An operation whose chain and target-side
    /// evidence is still missing or ambiguous after this long is not going to
    /// resolve itself, and leaving it `in_doubt` blocks operator retry and
    /// cancellation both.
    #[serde(default = "default_in_doubt_review_after_secs")]
    pub in_doubt_review_after_secs: u64,
}

/// Conservative default review threshold: long enough that an honestly
/// broadcast send has had every chance to appear in chain evidence, so
/// escalation means a human is genuinely needed.
const DEFAULT_IN_DOUBT_REVIEW_AFTER_SECS: u64 = 21_600;

fn default_in_doubt_review_after_secs() -> u64 {
    DEFAULT_IN_DOUBT_REVIEW_AFTER_SECS
}

impl FundingPolicyConfig {
    /// Conservative defaults for the daemon's configured Bitcoin network.
    #[must_use]
    pub fn defaults_for_network(network: BitcoinNetwork) -> Self {
        let (fee_reserve, confirmations) = match network {
            BitcoinNetwork::Bitcoin => (Sats(25_000), 3),
            BitcoinNetwork::Testnet | BitcoinNetwork::Signet => (Sats(5_000), 1),
            BitcoinNetwork::Regtest => (Sats(0), 1),
        };

        Self {
            fee_reserve,
            confirmations,
            stability_pool_min_fee_rate_ppb: 0,
            in_doubt_review_after_secs: DEFAULT_IN_DOUBT_REVIEW_AFTER_SECS,
        }
    }
}

/// Wallet balance thresholds used for operator replenishment alerts.
///
/// These thresholds only drive status reporting in `GetFundsResponse`; they do
/// not trigger automatic wallet top-ups or fund movement by themselves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplenishmentConfig {
    /// Warning threshold.
    pub warning_threshold: Sats,

    /// Critical threshold.
    pub critical_threshold: Sats,
}

/// Public API endpoint config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RpcEndpointConfig {
    /// Endpoint id, when known.
    pub endpoint_id: Option<RpcEndpointId>,

    /// Endpoint transport.
    pub transport: RpcTransport,

    /// Transport-specific address or URL.
    pub address: RpcEndpointAddress,

    /// Optional discovery hints.
    pub discovery_hints: Vec<RpcDiscoveryHint>,

    /// RPC protocol name.
    pub rpc_protocol_name: RpcProtocolName,
}

/// Advertisement publication config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementConfig {
    /// How often the provider refreshes or republishes its advertisement.
    pub republish_interval: DurationSecs,

    /// Whether the provider is allowed to publish the MVP public ready advertisement.
    pub ready_advertisement_enabled: bool,
}

/// Setup validation summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetupValidationSummary {
    /// Overall validation result.
    pub status: ValidationStatus,

    /// Individual checks.
    pub checks: Vec<SetupValidationCheck>,
}

/// Setup validation check.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetupValidationCheck {
    /// Check name.
    pub name: String,

    /// Check status.
    pub status: ValidationStatus,

    /// Operator-readable detail.
    pub detail: Option<String>,
}

/// Validation status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// Validation passed.
    #[strum(serialize = "passed")]
    Passed,

    /// Validation failed.
    #[strum(serialize = "failed")]
    Failed,

    /// Validation has not run.
    #[strum(serialize = "not_run")]
    NotRun,
}

/// Which kind of trust object an installed attestation payload is.
///
/// Operator-configured trust policy only. A Holder authorization and its
/// backing badge are not installed: they arrive together in the Holder's
/// published event and are enrolled from a relay
/// ([SPEC-flip-holder-authorization](../../liquidity-manager-daemon/specs/SPEC-flip-holder-authorization.md)).
/// The enum keeps its shape so a later policy document can be added without a
/// wire break.
#[derive(
    Clone, Debug, Eq, PartialEq, strum::Display, strum::EnumString, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AttestationKind {
    /// Issuer authority document used as a trusted verification input.
    #[strum(serialize = "issuer_authority")]
    IssuerAuthority,
}

/// The subject an attestation payload commits to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationSubject {
    /// An issuer authority describes this issuer identity.
    Issuer(Pubkey),
}

/// Ingest an issuer authority payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationInstallRequest {
    /// Raw payload file contents.
    pub payload: AttestationPayload,
}

/// Result of installing an attestation payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationInstallResponse {
    /// Identifier assigned to the stored payload.
    pub id: AttestationPayloadId,

    /// Which kind of payload was recognized.
    pub kind: AttestationKind,
}

/// Request to list installed attestation payloads.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationListRequest;

/// Metadata for one installed attestation payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationPayloadInfo {
    /// Identifier of the stored payload.
    pub id: AttestationPayloadId,

    /// Which kind of payload it is.
    pub kind: AttestationKind,

    /// Issuer key for an issuer credential or issuer authority, when known.
    pub issuer: Option<Pubkey>,

    /// The identity this payload commits to.
    pub subject: AttestationSubject,

    /// When the operator ingested it.
    pub ingested_at: Timestamp,

    /// Whether it currently validates.
    pub valid: bool,
}

/// Response listing installed attestation payloads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationListResponse {
    /// Installed payload metadata.
    pub payloads: Vec<AttestationPayloadInfo>,
}

/// How to select attestation payloads to remove.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationSelector {
    /// Remove one payload by id.
    Id(AttestationPayloadId),

    /// Remove every payload from the given issuer.
    Issuer(Pubkey),
}

/// Remove installed attestation payloads by id or issuer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationRemoveRequest {
    /// Payload selection.
    pub target: AttestationSelector,
}

/// Response confirming attestation removal.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationRemoveResponse;

/// Operator-visible installed attestation summary.
///
/// Counts installed policy documents only. Enrolled Holder authorizations are
/// reported by `get_holder_authorization_state`, not here, so an operator is
/// never shown a zero that contradicts what the advertisement carries.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationSummary {
    /// Number of issuer authority payloads.
    pub issuer_authorities: u32,

    /// Number of currently valid payloads.
    pub valid: u32,

    /// Number of currently invalid payloads.
    pub invalid: u32,
}

/// Request the Holder-authorization enrollment state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetHolderAuthorizationStateRequest;

/// What the operator console needs to run the enrollment flow.
///
/// The console builds the authorization request it shows a Holder — the
/// credential SDK's `HolderAuthorizationRequest`, carrying `subject_pubkey`
/// alone — from `provider_pubkey`, then watches `status` for the Holder's
/// publication arriving. Both are read from local state; neither costs a relay
/// round trip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetHolderAuthorizationStateResponse {
    /// The pubkey a Holder authorizes, or `None` before an operator installs
    /// the provider identity. There is nothing to put in a QR until then.
    pub provider_pubkey: Option<Pubkey>,

    /// Whether a Holder has authorized this provider yet.
    pub status: HolderAuthorizationStatus,
}

/// What this provider knows about being authorized by a Holder.
///
/// Four states rather than two, because "no authorization" was three different
/// situations an operator must tell apart: nothing has been read yet, a read
/// found nothing, and a read failed. A console that cannot distinguish them has
/// to hedge every sentence it shows.
///
/// `AuthorizationObserved` outranks the rest. Enrolled authorizations are
/// durable and re-verified before every use, so an empty or failed read never
/// demotes a provider that has one.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum HolderAuthorizationStatus {
    /// No reconciliation has completed since this runtime started, so nothing
    /// is known yet. This clears on its own and is not an operator action.
    #[default]
    Checking,

    /// A reconciliation completed and no Holder has authorized this provider.
    NotObserved {
        /// When that read finished.
        read_completed_at: Timestamp,
    },

    /// At least one Holder authorization is enrolled and carried in the
    /// advertisement.
    AuthorizationObserved {
        /// Enrolled authorizations, one per authorized badge.
        authorizations: u32,

        /// Distinct Holder identities behind them, sorted and deduplicated, so
        /// one Holder with several badges appears once.
        holders: Vec<Pubkey>,

        /// When the most recent of them was taken in. Durable, so it survives a
        /// restart and answers "when did this provider last enrol something"
        /// rather than "when did this process last look".
        newest_ingested_at: Timestamp,
    },

    /// Every relay the last reconciliation tried refused or was unreachable, so
    /// nothing can be concluded about whether a Holder has authorized this
    /// provider.
    RelayError {
        /// Operator-readable reason from the last relay tried.
        reason: String,

        /// When that attempt finished.
        failed_at: Timestamp,
    },
}

/// Reconcile enrolled Holder authorizations against the configured relays.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefreshHolderAuthorizationsRequest;

/// What one reconciliation did.
///
/// The counts describe relay answers, not trust. A candidate counted in
/// `candidates_seen` is whatever a relay chose to return; only
/// `candidates_verified` passed the local admission checks, and even those are
/// not judged badges — the app verifies the envelope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefreshHolderAuthorizationsResponse {
    /// Relays that answered, whether or not they carried anything.
    pub relays_answered: u32,

    /// Relays that could not be reached or refused, with the reason. A refresh
    /// succeeds on a partial answer: one unreachable relay must not block
    /// enrollment when another served the authorization.
    pub relays_failed: Vec<RelayFetchFailure>,

    /// Candidates offered across every answering relay.
    pub candidates_seen: u32,

    /// Candidates that passed the local admission checks.
    pub candidates_verified: u32,

    /// Enrollment state after the reconciliation.
    pub status: HolderAuthorizationStatus,
}

/// One relay that could not answer a reconciliation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayFetchFailure {
    /// Relay URL.
    pub relay_url: Url,

    /// Operator-readable reason.
    pub reason: String,
}

/// Request current provider config.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetProviderConfigRequest;

/// Provider config response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetProviderConfigResponse {
    /// Current config view.
    pub config: SetupConfigView,
}

/// Install the provider signing identity on a live daemon.
///
/// Install-only: a request naming a different key than the one already
/// installed is rejected, because provider-key rotation is out of scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallProviderIdentityRequest {
    /// Hex-encoded provider Nostr secret key.
    pub nostr_secret_key: SecretString,
}

/// Result of installing the provider signing identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallProviderIdentityResponse {
    /// Provider pubkey derived from the installed secret key.
    pub provider_pubkey: Pubkey,

    /// Whether this call installed the key, as opposed to matching the key
    /// already installed.
    pub installed: bool,

    /// Public readiness after the install.
    pub public_ready: bool,

    /// Why the deployment is not publicly ready, when it is not.
    pub not_ready_reason: Option<String>,
}

/// Replace the Operator Admin API bearer token.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotateAdminTokenRequest {
    /// Replacement bearer token.
    pub new_token: SecretString,
}

/// Result of replacing the Operator Admin API bearer token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RotateAdminTokenResponse {
    /// Whether the boot `--bootstrap-admin-token` is still accepted. Always
    /// false after a successful rotation.
    pub bootstrap_token_accepted: bool,
}

/// Close a target federation's Fedimint client so the next use reopens it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReopenFederationClientRequest {
    /// Federation whose client should be closed.
    pub federation_id: FederationId,
}

/// Result of closing a target federation's Fedimint client.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReopenFederationClientResponse {
    /// Whether anything was closed or cancelled. False means there was nothing
    /// to act on; the next use opens a client either way.
    ///
    /// True does not always mean a file lock is free yet. An open already in
    /// flight is cancelled rather than closed: it is told not to install its
    /// client, and it releases the lock when the build it cannot be interrupted
    /// part-way through finishes.
    pub closed: bool,
}

/// Update grouped provider config.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateProviderConfigRequest {
    /// Config patch.
    pub patch: ProviderConfigPatch,
}

/// Provider config update response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateProviderConfigResponse {
    /// Updated config view.
    pub config: SetupConfigView,

    /// Validation summary after the update.
    pub validation: SetupValidationSummary,
}

/// Gateway identity probe request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProbeGatewayRequest {
    /// Admin URL of the gateway to ask.
    pub admin_url: String,
}

/// What the gateway reports about itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProbeGatewayResponse {
    /// Identity FLIP will record for this gateway.
    pub gateway_id: GatewayId,

    /// Network the gateway's Lightning node runs on.
    pub network: BitcoinNetwork,

    /// Operator-readable alias, for confirming the right gateway was reached.
    pub lightning_alias: String,
}

/// A secret the daemon holds on the operator's behalf.
///
/// Secrets are named rather than carried inside the configuration they belong
/// to. A configuration write states the whole configuration, so a secret riding
/// inside one has to answer "what does absent mean?" — and the two secrets here
/// used to answer it differently: a missing gateway credential failed the write,
/// while a missing chain-observer password silently deleted the stored one. An
/// operator changing an unrelated field lost their bitcoind password and their
/// chain connection with it.
///
/// A named secret with an explicit operation has no absent case to interpret.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSecret {
    /// Credential the daemon authenticates to its gateway with.
    #[strum(serialize = "gateway_admin_credential")]
    GatewayAdminCredential,

    /// Password for a Bitcoin Core chain observer.
    #[strum(serialize = "chain_observer_password")]
    ChainObserverPassword,
}

/// What to do with a named secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum SecretUpdate {
    /// Replace the stored secret.
    Set(SecretString),

    /// Remove the stored secret. Refused for secrets the daemon cannot run
    /// without.
    Clear,
}

/// Secret write request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetConfigSecretRequest {
    /// Which secret to write.
    pub secret: ConfigSecret,

    /// The operation to perform.
    pub update: SecretUpdate,
}

/// Secret write response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetConfigSecretResponse {
    /// The secret that was written.
    pub secret: ConfigSecret,

    /// Whether a secret is stored under that name after the write.
    pub present: bool,
}

/// Grouped provider config patch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfigPatch {
    /// Policy replacement.
    pub policy: Option<ProviderPolicy>,

    /// Relay replacement.
    pub relays: Option<Vec<Url>>,

    /// Capacity config replacement.
    pub capacity: Option<CapacityConfig>,

    /// Funding policy replacement.
    pub funding_policy: Option<FundingPolicyConfig>,

    /// Replenishment config replacement.
    pub replenishment: Option<ReplenishmentConfig>,

    /// Advertised endpoint replacement.
    pub advertised_endpoint: Option<RpcEndpointConfig>,

    /// Advertisement config replacement.
    pub advertisement: Option<AdvertisementConfig>,

    /// Provider display metadata patch. `None` leaves it unchanged.
    pub provider_display: Option<ProviderDisplayPatch>,
}

/// Provider display metadata patch operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum ProviderDisplayPatch {
    /// Replace provider display metadata.
    Set(ProviderDisplay),

    /// Clear provider display metadata.
    Clear,
}

/// Request advertisement state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAdvertisementStateRequest;

/// Advertisement state response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetAdvertisementStateResponse {
    /// Current signed public ready advertisement, when publishable.
    pub advertisement: Option<Signed<crate::LiquidityProviderAdvertisement>>,

    /// Publication status.
    pub publication_status: AdvertisementPublicationStatus,

    /// Last publication time.
    pub last_published_at: Option<Timestamp>,

    /// Current expiry.
    pub expires_at: Option<Timestamp>,

    /// When the operator last withdrew the advertisement, if it is still
    /// withdrawn.
    ///
    /// A withdrawal is durable: while this is set the publisher leaves the
    /// provider off the relays, and only an explicit republish puts it back.
    /// The client needs it to say who took the provider off the market and
    /// when, which the publication status alone cannot express.
    pub withdrawn_at: Option<Timestamp>,

    /// Relay publication states.
    pub relay_states: Vec<RelayPublicationState>,

    /// Whether current setup and dependencies make the public advertisement ready.
    pub ready: bool,

    /// Latest readiness validation, when known.
    pub readiness: Option<SetupValidationSummary>,

    /// How many holder authorizations in `advertisement` no longer verify.
    ///
    /// The advertisement above is returned exactly as it was signed and
    /// published, because dropping an envelope from a signed payload would
    /// invalidate the payload's own proof. So the Admin surface reports the
    /// discrepancy instead of hiding it: `0` means every published envelope
    /// still verifies against the enrolled set, and anything higher means the
    /// stored payload carries envelopes FLIP would no longer stand behind.
    ///
    /// A non-zero count is not by itself an attack. An envelope revoked or
    /// expired since publication reads the same way as one written into the
    /// row by another route, and both are things an operator should see.
    pub unverified_holder_authorization_count: u32,
}

/// Republish the provider's public ready advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepublishAdvertisementRequest {
    /// Force publication even if the current advertisement appears fresh.
    pub force: bool,
}

/// Advertisement publish response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepublishAdvertisementResponse {
    /// Publication status after the command.
    pub publication_status: AdvertisementPublicationStatus,

    /// Relay publication states.
    pub relay_states: Vec<RelayPublicationState>,
}

/// Withdraw the current public ready advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WithdrawAdvertisementRequest {
    /// Optional operator-readable reason.
    pub reason: Option<String>,
}

/// Advertisement withdrawal response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WithdrawAdvertisementResponse {
    /// Publication status after the command.
    pub publication_status: AdvertisementPublicationStatus,

    /// Relay publication states.
    pub relay_states: Vec<RelayPublicationState>,
}

/// Refresh relay connections and cursors.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefreshRelaysRequest;

/// Relay refresh response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefreshRelaysResponse {
    /// Relay publication states after refresh.
    pub relay_states: Vec<RelayPublicationState>,
}

/// Advertisement publication status.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AdvertisementPublicationStatus {
    /// The public ready advertisement cannot be published yet.
    #[strum(serialize = "not_ready")]
    NotReady,

    /// The public ready advertisement is published and fresh.
    #[strum(serialize = "published")]
    Published,

    /// The public ready advertisement publication is stale.
    #[strum(serialize = "stale")]
    Stale,

    /// The public ready advertisement has been withdrawn.
    #[strum(serialize = "withdrawn")]
    Withdrawn,

    /// Publication of the public ready advertisement failed.
    #[strum(serialize = "failed")]
    Failed,
}

/// Produce an unencrypted FLIP data-directory tarball backup archive.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateBackupRequest;

/// Backup creation response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateBackupResponse {
    /// Handle to the produced archive.
    pub archive: BackupArchive,

    /// Manifest for the produced archive.
    pub manifest: BackupManifest,
}

/// Inspect an unencrypted backup archive manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectBackupRequest {
    /// Archive to inspect.
    pub archive: BackupArchive,
}

/// Backup inspection response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectBackupResponse {
    /// Manifest extracted from the archive.
    pub manifest: BackupManifest,
}

/// Restore from an unencrypted backup archive on a fresh host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestoreBackupRequest {
    /// Archive to restore from.
    pub archive: BackupArchive,
}

/// Restore response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestoreBackupResponse {
    /// Setup status after restore.
    pub status: SetupStatus,

    /// Validation summary after restore.
    pub validation: SetupValidationSummary,

    /// State groups restored from the archive.
    pub restored_state_groups: Vec<BackupStateGroup>,
}

/// Backup manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Backup format version.
    pub version: ProtocolVersion,

    /// Backup creation timestamp.
    pub created_at: Timestamp,

    /// State groups included in the archive.
    pub state_groups: Vec<BackupStateGroup>,

    /// The one instant every archived store was captured at.
    pub recovery_point: BackupRecoveryPoint,
}

/// The common recovery point an archive's payload was captured at.
///
/// FLIP's durable state spans SQLite and the target-Fedimint client
/// directories. Reading two mutable stores without a shared snapshot or a
/// quiescence barrier may observe different instants, so an archive that simply
/// walked the live data directory could hold SQLite from one moment and a
/// client database from another, with nothing recording that it had.
///
/// `create_backup` holds every periodic worker pass still, copies both stores,
/// and only then releases and compresses. This records the instant that barrier
/// was taken, so the correspondence is a property of the archive rather than of
/// the daemon that happened to write it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupRecoveryPoint {
    /// When the quiescence barrier was taken, before either store was copied.
    pub quiesced_at: Timestamp,

    /// Copy order under the barrier, oldest first, for an operator reconciling
    /// an archive against external records.
    pub stores: Vec<BackupStore>,
}

/// A durable store captured under one recovery point.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStore {
    /// The SQLite database, captured with `VACUUM INTO` so the copy needs no
    /// write-ahead log beside it.
    #[strum(serialize = "sqlite")]
    Sqlite,

    /// The target-Fedimint client directories and every other payload file.
    #[strum(serialize = "data_directory")]
    DataDirectory,
}

/// Restorable FLIP state group.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStateGroup {
    /// Provider identity material.
    #[strum(serialize = "provider_identity")]
    ProviderIdentity,

    /// Installed issuer authorities and enrolled Holder authorizations.
    #[strum(serialize = "attestations")]
    Attestations,

    /// Gatewayd operation checkpoints and target-federation wallet/client storage.
    #[strum(serialize = "wallet_client_state")]
    WalletClientState,

    /// Local database and migration state.
    #[strum(serialize = "database")]
    Database,

    /// Request, allocation, and wallet operation history.
    #[strum(serialize = "operation_history")]
    OperationHistory,

    /// Active operator configuration.
    #[strum(serialize = "operator_config")]
    OperatorConfig,

    /// External dependency configuration and credentials persisted in the data dir.
    #[strum(serialize = "external_dependencies")]
    ExternalDependencies,
}

/// Relay publication state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayPublicationState {
    /// Relay URL.
    pub relay_url: Url,

    /// Relay status.
    pub status: RelayStatus,

    /// Last error, if any.
    pub last_error: Option<String>,

    /// Last observed timestamp.
    pub last_seen_at: Option<Timestamp>,
}

/// Relay status.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RelayStatus {
    /// Relay is connected.
    #[strum(serialize = "connected")]
    Connected,

    /// Relay is disconnected.
    #[strum(serialize = "disconnected")]
    Disconnected,

    /// Relay publish succeeded.
    #[strum(serialize = "published")]
    Published,

    /// Relay publish failed.
    #[strum(serialize = "failed")]
    Failed,
}

/// Request current funds and inventory state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetFundsRequest;

/// Funds and inventory response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetFundsResponse {
    /// Wallet balance summary.
    pub balance: WalletBalanceSummary,

    /// Replenishment status.
    pub replenishment: ReplenishmentStatus,

    /// Gateway inventory state.
    pub gateway: GatewayInventoryState,

    /// Stability-pool inventory state.
    pub stability_pool: StabilityPoolInventoryState,

    /// Effective liquidity by source.
    pub effective_liquidity: Vec<EffectiveLiquidityItem>,
}

/// Wallet balance summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WalletBalanceSummary {
    /// Spendable gatewayd on-chain wallet balance.
    pub spendable: Sats,

    /// Pending incoming funds not yet spendable.
    pub pending_incoming: Sats,

    /// Pending outgoing wallet operations.
    pub pending_outgoing: Sats,

    /// Amounts in flight for accepted allocations.
    pub in_flight_allocations: Sats,

    /// Configured fee reserve withheld from `available_balance`.
    pub fee_reserve: Sats,

    /// Available balance for new allocations.
    pub available_balance: Sats,
}

/// Replenishment status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentStatus {
    /// Balance is above warning threshold.
    #[strum(serialize = "ok")]
    Ok,

    /// Balance is below warning threshold.
    #[strum(serialize = "warning")]
    Warning,

    /// Balance is below critical threshold.
    #[strum(serialize = "critical")]
    Critical,
}

/// Gateway inventory state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayInventoryState {
    /// Gateway id.
    pub gateway_id: GatewayId,

    /// Gateway display name.
    pub gateway_name: GatewayName,

    /// Gateway status.
    pub status: InventoryStatus,

    /// Effective available amount.
    pub available_amount: Sats,

    /// Last observed timestamp.
    pub observed_at: Option<Timestamp>,
}

/// Stability-pool inventory state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StabilityPoolInventoryState {
    /// Stability-pool status.
    pub status: InventoryStatus,

    /// Effective available amount.
    pub available_amount: Sats,

    /// Last observed timestamp.
    pub observed_at: Option<Timestamp>,
}

/// Inventory status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum InventoryStatus {
    /// Inventory is available.
    #[strum(serialize = "available")]
    Available,

    /// Inventory is configured but unavailable.
    #[strum(serialize = "unavailable")]
    Unavailable,

    /// Inventory is disabled.
    #[strum(serialize = "disabled")]
    Disabled,

    /// Inventory state is unknown.
    #[strum(serialize = "unknown")]
    Unknown,
}

/// Effective liquidity item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectiveLiquidityItem {
    /// Source type.
    pub source_type: SourceType,

    /// Gateway id for gateway/LN liquidity.
    pub gateway_id: Option<GatewayId>,

    /// Effective amount.
    pub amount: Sats,
}

/// Create a fresh gatewayd wallet top-up address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateDepositAddressRequest {
    /// Optional operator label.
    pub label: Option<String>,
}

/// Gatewayd wallet top-up address response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateDepositAddressResponse {
    /// Address for topping up the gatewayd on-chain wallet.
    pub address: String,

    /// Bitcoin network.
    pub network: BitcoinNetwork,

    /// Wallet operation id, when a tracking record was created.
    pub operation_id: Option<WalletOperationId>,
}

/// Request an operator withdrawal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestWithdrawalRequest {
    /// Opaque client-generated identity for this withdrawal intent.
    ///
    /// Reuse this value when retrying the same request. The value is unique
    /// within one FLIP installation's Admin API.
    pub withdrawal_intent_id: String,

    /// Withdrawal address.
    pub address: String,

    /// Withdrawal amount.
    pub amount: Sats,

    /// Optional fee rate in sat/vbyte.
    pub fee_rate_sat_per_vbyte: Option<u64>,
}

/// Withdrawal response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestWithdrawalResponse {
    /// Wallet operation record.
    pub operation: WalletOperation,
}

/// List wallet operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListWalletOperationsRequest {
    /// Page request.
    pub page: PageRequest,

    /// Optional operation status filter.
    pub status_filter: Option<WalletOperationStatus>,

    /// Optional time range.
    pub time_range: Option<TimeRange>,
}

/// Wallet operation list response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListWalletOperationsResponse {
    /// Paginated wallet operation summaries.
    pub operations: ListResponse<WalletOperationSummary>,
}

/// Wallet operation summary.
///
/// The list shape. It deliberately omits the destination, the chain evidence
/// and the failure detail: a list of sends does not need them, and a client
/// that has to act on one operation reads it with `get_wallet_operation`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WalletOperationSummary {
    /// Operation id.
    pub operation_id: WalletOperationId,

    /// Operation type.
    pub operation_type: WalletOperationType,

    /// Operation amount.
    pub amount: Sats,

    /// Operation status.
    pub status: WalletOperationStatus,

    /// Federation of the related allocation.
    pub federation_id: Option<FederationId>,

    /// Creation timestamp.
    pub created_at: Timestamp,

    /// Last update timestamp.
    pub updated_at: Timestamp,
}

/// Wallet operation detail request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetWalletOperationRequest {
    /// Operation to read.
    pub operation_id: WalletOperationId,
}

/// Wallet operation detail response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetWalletOperationResponse {
    /// The operation.
    pub operation: WalletOperation,
}

/// Wallet operation detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WalletOperation {
    /// Operation id.
    pub operation_id: WalletOperationId,

    /// Operation type.
    pub operation_type: WalletOperationType,

    /// Operation amount.
    pub amount: Sats,

    /// Address involved in the operation, when one exists.
    pub address: Option<String>,

    /// On-chain transaction id returned by gatewayd or found by chain observation.
    pub txid: Option<String>,

    /// Output index claimed as settlement evidence, once chain observation
    /// has verified the operation's destination and amount.
    pub tx_vout: Option<u32>,

    /// Operation status.
    pub status: WalletOperationStatus,

    /// Confirmation count, when applicable to the operation.
    pub confirmation_count: Option<u32>,

    /// Federation of the related allocation.
    pub federation_id: Option<FederationId>,

    /// Related allocation item.
    pub item_id: Option<ItemId>,

    /// Creation timestamp.
    pub created_at: Timestamp,

    /// Last update timestamp.
    pub updated_at: Timestamp,

    /// Failure details, when failed.
    pub failure: Option<AdminFailure>,
}

/// Wallet operation type.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WalletOperationType {
    /// Incoming wallet deposit.
    #[strum(serialize = "deposit")]
    Deposit,

    /// Outgoing wallet withdrawal.
    #[strum(serialize = "withdrawal")]
    Withdrawal,

    /// Gateway funding operation.
    #[strum(serialize = "gateway_funding")]
    GatewayFunding,

    /// Stability-pool funding operation.
    #[strum(serialize = "stability_pool_funding")]
    StabilityPoolFunding,
}

/// Wallet operation status.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WalletOperationStatus {
    /// Operation is pending.
    #[strum(serialize = "pending")]
    Pending,

    /// Operation has been broadcast.
    #[strum(serialize = "broadcast")]
    Broadcast,

    /// Operation is confirmed.
    #[strum(serialize = "confirmed")]
    Confirmed,

    /// Operation completed.
    #[strum(serialize = "completed")]
    Completed,

    /// Submission outcome is unknown and must be reconciled before retrying.
    #[strum(serialize = "in_doubt")]
    InDoubt,

    /// Operator review is required before this operation can continue.
    #[strum(serialize = "manual_review_required")]
    ManualReviewRequired,

    /// Operation failed.
    #[strum(serialize = "failed")]
    Failed,

    /// Operation was cancelled before irreversible submission.
    #[strum(serialize = "cancelled")]
    Cancelled,
}

/// List allocations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListAllocationsRequest {
    /// Page request.
    pub page: PageRequest,

    /// Optional time range.
    pub time_range: Option<TimeRange>,
}

/// Allocation list response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListAllocationsResponse {
    /// Paginated allocation summaries.
    pub allocations: ListResponse<AdminAllocationSummary>,
}

/// Get allocation detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAdminAllocationRequest {
    /// Federation whose allocation to fetch.
    pub federation_id: FederationId,
}

/// Allocation detail response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAdminAllocationResponse {
    /// Allocation detail.
    pub allocation: AdminAllocationDetail,
}

/// Allocation summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdminAllocationSummary {
    /// Federation this allocation funds.
    pub federation_id: FederationId,

    /// Gateway item status, when requested.
    pub gateway_status: Option<ItemAllocationStatus>,

    /// Stability-pool item status, when requested.
    pub stability_pool_status: Option<ItemAllocationStatus>,

    /// Total committed amount.
    pub committed_amount: Sats,

    /// Creation timestamp.
    pub created_at: Timestamp,

    /// Last update timestamp.
    pub updated_at: Timestamp,
}

/// Allocation detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdminAllocationDetail {
    /// Federation this allocation funds.
    pub federation_id: FederationId,

    /// Public allocation status.
    pub status: AllocationStatus,

    /// Wallet operations for this allocation.
    pub wallet_operations: Vec<WalletOperation>,

    /// Failures recorded by attached wallet operations. Item failures remain
    /// on their independent item statuses.
    pub failures: Vec<AdminFailure>,
}

/// Request verification summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetVerificationSummaryRequest {
    /// Federation whose allocation's verification summary to fetch.
    pub federation_id: FederationId,
}

/// Verification summary response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetVerificationSummaryResponse {
    /// Verification summary.
    pub summary: VerificationSummary,
}

/// Retry an idempotent funding step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryFundingStepRequest {
    /// Federation whose allocation to retry.
    pub federation_id: FederationId,

    /// Optional item id.
    pub item_id: Option<ItemId>,

    /// Optional wallet operation id.
    pub operation_id: Option<WalletOperationId>,
}

/// Funding retry response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryFundingStepResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Inspect the target federation client behind one stability-pool allocation.
///
/// Read-only. When an interruption leaves FLIP unable to say whether its
/// deposit was submitted, the target client's own records are the only place
/// the answer exists, and this is the operator's view of them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectTargetClientRequest {
    /// Federation whose stability-pool allocation to inspect.
    pub federation_id: FederationId,
}

/// Target-client inspection response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectTargetClientResponse {
    /// Spendable BTC-denominated e-cash the target client currently holds.
    ///
    /// A client-wide total, not this item's: any other operation moves it too,
    /// which is why it informs an operator rather than settling anything.
    pub spendable_balance: Sats,

    /// Provider liquidity the stability pool reports for this provider account.
    pub observed_provided_amount: Sats,

    /// Raw provider liquidity statistics, as the pool reported them.
    pub liquidity_stats_json: String,

    /// The deposit operation this allocation item has recorded, if any.
    ///
    /// Its absence beside a non-empty `deposit_operations` is the signature of
    /// the interrupted-submit window.
    pub recorded_deposit_operation_id: Option<String>,

    /// Stability-pool deposits the target client records having made.
    pub deposit_operations: Vec<TargetDepositOperationView>,

    /// False when the client's operation history was longer than FLIP reads in
    /// one pass, so the list above may omit older deposits.
    pub scan_complete: bool,
}

/// One stability-pool deposit as the target client records it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetDepositOperationView {
    /// Target-client operation id.
    pub operation_id: String,

    /// Deposit amount.
    pub amount: Sats,

    /// Outcome the client has cached: `initiated`, `tx_accepted`, `success`, or
    /// `failed`. Absent when the client has never observed the operation to
    /// completion, which is what an interrupted submit leaves behind.
    pub outcome: Option<String>,

    /// Failure detail, when the outcome is `failed`.
    pub failure_detail: Option<String>,

    /// When the client created the operation.
    pub created_at: Timestamp,
}

/// Bind a target-client deposit to the allocation item that paid for it.
///
/// The operator has established, from the inspection above, which deposit this
/// item's e-cash actually became. Binding hands that back to FLIP so its normal
/// observation resumes; it moves no money.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindTargetDepositRequest {
    /// Federation whose stability-pool item to bind.
    pub federation_id: FederationId,

    /// Target-client deposit operation id to bind.
    pub operation_id: String,

    /// Operator's reason, recorded in the audit log.
    pub reason: Option<String>,
}

/// Target-deposit binding response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindTargetDepositResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Give up on a stability item whose value cannot be recovered by FLIP.
///
/// After a peg-in is claimed, the provider's e-cash is inside the target client
/// and the funding send has settled, so the item can be neither retried nor
/// cancelled. If the pool will never accept the deposit, it can also never
/// complete — leaving it holding provider capacity forever. This releases that
/// capacity and records that the value itself needs recovering outside FLIP.
///
/// It moves no money and recovers none. Returning target-client value to the
/// provider wallet is a peg-out and is not this.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AbandonTargetClientValueRequest {
    /// Federation whose stability-pool item to give up on.
    pub federation_id: FederationId,

    /// Operator's reason. Required: this writes off FLIP's ability to manage
    /// funds it already sent, and the audit log should say why.
    pub reason: String,
}

/// Release a federation's allocation binding when it is idle but wedged.
///
/// A federation has one allocation, and the requester that first presented a
/// valid endorsement for it owns that allocation. `insert_allocation` is the
/// table's only production writer and there is no production `UPDATE`, so that
/// binding used to be permanent. `SPEC-flip-rpc`
/// removes the permanence with two mechanisms: a verified requester takes over
/// an allocation that holds nothing, automatically; and this verb, for the case
/// where an operator needs to unstick one by hand.
///
/// It refuses unless the allocation holds nothing — nothing reserving, nothing
/// awaiting settlement, no delivered value — which is the same predicate the
/// automatic takeover uses. It is an override of *who* holds a federation, not
/// of that predicate.
///
/// It moves no money. The federation is simply free to be requested again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseFederationAllocationRequest {
    /// Federation whose allocation binding to release.
    pub federation_id: FederationId,

    /// Operator's reason. Required: this hands a federation to whoever asks for
    /// it next, and the audit log should say why.
    pub reason: String,
}

/// Release response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseFederationAllocationResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Requester the allocation was bound to, when one was released.
    pub previous_requester: Option<Pubkey>,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Abandonment response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AbandonTargetClientValueResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Value left at the target client, when the item recorded one.
    pub abandoned_amount: Option<Sats>,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Complete a reviewed wallet send that FLIP cannot verify against the chain.
///
/// `resolve_manual_review` requires exact-output chain evidence for a `completed`
/// resolution: the chain observer must return the named transaction and one of
/// its outputs must pay this operation's persisted address for its persisted
/// amount. That refuses three cases an operator legitimately meets — the observer
/// is unreachable, the operation has no persisted address, or the observer does
/// not know the transaction — and a chain-observer outage is close to the
/// situation that produces reviewed operations in the first place.
///
/// This is the deliberate way through. It completes the operation on the
/// operator's assertion alone and records that no evidence existed, rather than
/// letting an unverified assertion pass through the normal verb where nothing
/// marks it as unverified. It is the same shape as
/// [`AbandonTargetClientValueRequest`]: FLIP does not deny a state it cannot
/// prevent; it names the state, requires a deliberate second call to reach it,
/// and writes the choice down.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompleteReviewWithoutEvidenceRequest {
    /// Wallet operation under review.
    pub operation_id: WalletOperationId,

    /// Transaction the operator asserts settled the send. Recorded as an
    /// operator assertion, not as evidence: FLIP does not confirm it.
    pub txid: String,

    /// Operator's reason. Required: this records a settlement FLIP could not
    /// verify, and the audit log should say why the operator was sure.
    pub reason: String,
}

/// Response to completing a reviewed send without evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompleteReviewWithoutEvidenceResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Resolve a wallet send that escalation put under manual review.
///
/// FLIP escalates a send whose outcome it cannot establish rather than guessing,
/// and will not resubmit or cancel it on its own. This is how an operator who
/// has established the outcome out of band tells FLIP what it was.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveManualReviewRequest {
    /// Wallet operation under review.
    pub operation_id: WalletOperationId,

    /// What the operator established.
    pub resolution: ManualReviewResolution,

    /// Transaction that settled the send. Required for `completed`, and
    /// rejected for the other resolutions, which assert no send happened.
    pub txid: Option<String>,

    /// Operator's reason, recorded in the audit log.
    pub reason: Option<String>,
}

/// Operator's conclusion about a wallet send held for manual review.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ManualReviewResolution {
    /// The send settled on chain as the supplied `txid`.
    #[strum(serialize = "completed")]
    Completed,

    /// The send did not happen and is not to be attempted again.
    #[strum(serialize = "failed")]
    Failed,

    /// The send did not happen and may be attempted again.
    #[strum(serialize = "safe_to_retry")]
    SafeToRetry,
}

/// Manual review resolution response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveManualReviewResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Operation after the resolution, when one was applied.
    pub operation: Option<WalletOperation>,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Cancel allocation work where protocol state allows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CancelAllocationRequest {
    /// Federation whose allocation to cancel.
    pub federation_id: FederationId,

    /// Optional operator reason.
    pub reason: Option<String>,
}

/// Allocation cancellation response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CancelAllocationResponse {
    /// Manual operation status.
    pub status: ManualOperationStatus,

    /// Allocation status after cancellation.
    pub allocation_status: Option<AllocationStatus>,

    /// Optional detail.
    pub detail: Option<String>,
}

/// Manual operation status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ManualOperationStatus {
    /// Operation was accepted.
    #[strum(serialize = "accepted")]
    Accepted,

    /// Operation was rejected.
    #[strum(serialize = "rejected")]
    Rejected,

    /// Operation did not find a matching target.
    #[strum(serialize = "not_found")]
    NotFound,

    /// Operation was already applied.
    #[strum(serialize = "already_applied")]
    AlreadyApplied,
}

/// Operator-visible failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdminFailure {
    /// Failure code.
    pub code: String,

    /// Failure message.
    pub message: String,

    /// Timestamp when failure occurred.
    pub occurred_at: Timestamp,

    /// Optional related federation.
    pub federation_id: Option<FederationId>,

    /// Optional related item id.
    pub item_id: Option<ItemId>,
}
