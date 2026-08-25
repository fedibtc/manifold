use std::{collections::BTreeSet, fs, net::SocketAddr, path::PathBuf};

use crate::{
    AppId, FcmProviderConfig, FirebaseCredentials, FirebaseCredentialsError,
    rate_limits::TrustedProxyCidrs,
};
use clap::{
    ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind,
    parser::ValueSource,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use reqwest::Url;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_DATABASE_URL: &str = "sqlite://push-gateway.sqlite?mode=rwc";
const DEFAULT_PROVIDER: &str = "noop";
const DEFAULT_OUTBOX_WORKER_CONCURRENCY: &str = "4";
const DEFAULT_PUBLIC_BASE_URL: &str = "https://push-gateway.invalid";
const DEFAULT_FCM_SEND_ENDPOINT_BASE: &str = "https://fcm.googleapis.com";
const DEFAULT_FCM_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_HOOK_INVOCATION_LIMIT: u64 = 60;
const DEFAULT_HOOK_CREATION_LIMIT: u64 = 5;
const DEFAULT_REGISTRATION_CHANGE_LIMIT: u64 = 10;
const DEFAULT_AUTH_EVENTS_PER_SOURCE_PREFIX: u64 = 120;
const DEFAULT_AUTH_EVENT_WINDOW_SECONDS: u64 = 60;
const DEFAULT_RATE_LIMIT_WINDOW_SECONDS: u64 = 3600;
const DEFAULT_MAX_ACTIVE_HOOKS_PER_RECIPIENT: u64 = 20;
const DEFAULT_MAX_ACTIVE_INSTALLATIONS_PER_RECIPIENT: u64 = 8;
const DEFAULT_MAX_ACTIVE_HOOKS_GLOBAL: u64 = 50_000;
const DEFAULT_MAX_ACTIVE_INSTALLATIONS_GLOBAL: u64 = 50_000;
const DEFAULT_MAX_HOOK_ROWS_GLOBAL: u64 = 100_000;
const DEFAULT_MAX_REGISTRATION_ROWS_GLOBAL: u64 = 200_000;
const DEFAULT_ADMISSION_GC_BATCH_SIZE: u64 = 1_000;
const DEFAULT_RETENTION_DAYS: u64 = 7;
const DEFAULT_REGISTRATION_TTL_DAYS: u64 = 30;
const SECONDS_PER_DAY: u64 = 86_400;

/// Configured push-provider implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PushProviderConfig {
    /// Do not contact an external push provider.
    Noop,
    /// Send notifications using FCM HTTP v1.
    Fcm(FcmProviderConfig),
}

/// Push gateway runtime configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct PushGatewayConfig {
    app_id: Option<AppId>,
    unsafe_allow_any_app_id_for_tests: bool,
    database_url: String,
    provider: PushProviderConfig,
    public_base_url: String,
    operator_bind: Option<SocketAddr>,
    operator_token: Option<OperatorToken>,
    public_metrics_enabled: bool,
    production_mode: bool,
    open_self_registration_enabled: bool,
    admission_allowed_recipients: BTreeSet<String>,
    legacy_notification_hook_enabled: bool,
    outbox_worker_concurrency: usize,
    retention_seconds: u64,
    registration_ttl_seconds: u64,
    rate_limits: RateLimitConfig,
    telemetry: Option<TelemetryReceiverConfig>,
}

/// Complete fail-closed telemetry receiver configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct TelemetryReceiverConfig {
    environment: ManifoldEnvironment,
    encryption_key: [u8; 32],
}

impl std::fmt::Debug for TelemetryReceiverConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelemetryReceiverConfig")
            .field("environment", &self.environment)
            .field("encryption_key", &"<redacted>")
            .finish()
    }
}

impl TelemetryReceiverConfig {
    /// Build a complete receiver configuration. Partial configuration is never
    /// representable at runtime.
    pub fn new(
        environment: ManifoldEnvironment,
        encryption_key_hex: &str,
    ) -> Result<Self, PushGatewayConfigError> {
        let key = hex::decode(encryption_key_hex.trim())
            .map_err(|_| PushGatewayConfigError::InvalidTelemetryConfiguration)?;
        let encryption_key: [u8; 32] = key
            .try_into()
            .map_err(|_| PushGatewayConfigError::InvalidTelemetryConfiguration)?;
        Ok(Self {
            environment,
            encryption_key,
        })
    }

    #[must_use]
    pub fn environment(&self) -> ManifoldEnvironment {
        self.environment
    }

    #[must_use]
    pub fn encryption_key(&self) -> &[u8; 32] {
        &self.encryption_key
    }
}

impl std::fmt::Debug for PushGatewayConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushGatewayConfig")
            .field("app_id", &self.app_id.as_ref().map(|_| "<redacted>"))
            .field(
                "unsafe_allow_any_app_id_for_tests",
                &self.unsafe_allow_any_app_id_for_tests,
            )
            .field("database_url", &redact_database_url(&self.database_url))
            .field("provider", &self.provider)
            .field("public_base_url", &self.public_base_url)
            .field("operator_bind", &self.operator_bind)
            .field(
                "operator_token",
                &self.operator_token.as_ref().map(|_| "<redacted>"),
            )
            .field("public_metrics_enabled", &self.public_metrics_enabled)
            .field("production_mode", &self.production_mode)
            .field(
                "open_self_registration_enabled",
                &self.open_self_registration_enabled,
            )
            .field(
                "admission_allowed_recipients",
                &self
                    .admission_allowed_recipients
                    .iter()
                    .map(|_| "<redacted>")
                    .collect::<Vec<_>>(),
            )
            .field(
                "legacy_notification_hook_enabled",
                &self.legacy_notification_hook_enabled,
            )
            .field("outbox_worker_concurrency", &self.outbox_worker_concurrency)
            .field("retention_seconds", &self.retention_seconds)
            .field("registration_ttl_seconds", &self.registration_ttl_seconds)
            .field("rate_limits", &self.rate_limits)
            .field("telemetry", &self.telemetry)
            .finish()
    }
}

/// Bearer token protecting push-gateway operator endpoints.
#[derive(Clone, Eq, PartialEq)]
pub struct OperatorToken(String);

impl OperatorToken {
    /// Creates a normalized operator token.
    ///
    /// Leading/trailing whitespace is trimmed and empty values are rejected so
    /// unset/blank environment variables do not accidentally enable an
    /// impossible-to-satisfy operator-token mode.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Option<Self> {
        let token = token.into().trim().to_owned();
        if token.is_empty() {
            return None;
        }
        Some(Self(token))
    }

    /// Returns the token value for Authorization header comparison.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for OperatorToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    /// Auth events admitted per source prefix in one window; zero disables the limit.
    pub auth_events_per_source_prefix: u64,
    /// Auth-event fixed-window duration in seconds.
    pub auth_event_window_seconds: u64,
    /// Hook invocations admitted per source prefix in one window; zero disables the limit.
    pub hook_invocations_per_source_prefix: u64,
    /// Hook invocations admitted per hook in one window; zero disables the limit.
    pub hook_invocations_per_hook: u64,
    /// Hook-invocation fixed-window duration in seconds.
    pub hook_invocation_window_seconds: u64,
    /// Hook creations admitted per recipient in one window; zero disables the limit.
    pub hook_creations_per_recipient: u64,
    /// Hook-creation fixed-window duration in seconds.
    pub hook_creation_window_seconds: u64,
    /// Registration changes admitted per recipient/source pair; zero disables the limit.
    pub registration_changes_per_recipient_source: u64,
    /// Registration changes admitted per source prefix; zero disables the limit.
    pub registration_changes_per_source_prefix: u64,
    /// Registration-change fixed-window duration in seconds.
    pub registration_change_window_seconds: u64,
    /// Active hooks allowed per recipient; zero disables the limit.
    pub max_active_hooks_per_recipient: u64,
    /// Active installations allowed per recipient; zero disables the limit.
    pub max_active_installations_per_recipient: u64,
    /// Active hooks allowed across the gateway; zero disables the limit.
    pub max_active_hooks_global: u64,
    /// Active installations allowed across the gateway; zero disables the limit.
    pub max_active_installations_global: u64,
    /// Physical hook and idempotency rows allowed per table; zero disables the limit.
    pub max_hook_rows_global: u64,
    /// Physical registration and owner rows allowed together; zero disables the limit.
    pub max_registration_rows_global: u64,
    /// Maximum stale or terminal rows reclaimed by one admission transaction.
    pub admission_gc_batch_size: u64,
    /// Active outbox rows allowed across the gateway; zero disables the limit.
    pub max_global_outbox_backlog: u64,
    /// Active outbox rows allowed per recipient; zero disables the limit.
    pub max_recipient_outbox_backlog: u64,
    /// Proxy networks trusted to supply forwarded client addresses.
    pub trusted_proxy_cidrs: Option<TrustedProxyCidrs>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            auth_events_per_source_prefix: DEFAULT_AUTH_EVENTS_PER_SOURCE_PREFIX,
            auth_event_window_seconds: DEFAULT_AUTH_EVENT_WINDOW_SECONDS,
            hook_invocations_per_source_prefix: DEFAULT_HOOK_INVOCATION_LIMIT,
            hook_invocations_per_hook: DEFAULT_HOOK_INVOCATION_LIMIT,
            hook_invocation_window_seconds: DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
            hook_creations_per_recipient: DEFAULT_HOOK_CREATION_LIMIT,
            hook_creation_window_seconds: DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
            registration_changes_per_recipient_source: DEFAULT_REGISTRATION_CHANGE_LIMIT,
            registration_changes_per_source_prefix: DEFAULT_REGISTRATION_CHANGE_LIMIT * 3,
            registration_change_window_seconds: DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
            max_active_hooks_per_recipient: DEFAULT_MAX_ACTIVE_HOOKS_PER_RECIPIENT,
            max_active_installations_per_recipient: DEFAULT_MAX_ACTIVE_INSTALLATIONS_PER_RECIPIENT,
            max_active_hooks_global: DEFAULT_MAX_ACTIVE_HOOKS_GLOBAL,
            max_active_installations_global: DEFAULT_MAX_ACTIVE_INSTALLATIONS_GLOBAL,
            max_hook_rows_global: DEFAULT_MAX_HOOK_ROWS_GLOBAL,
            max_registration_rows_global: DEFAULT_MAX_REGISTRATION_ROWS_GLOBAL,
            admission_gc_batch_size: DEFAULT_ADMISSION_GC_BATCH_SIZE,
            max_global_outbox_backlog: 0,
            max_recipient_outbox_backlog: 0,
            trusted_proxy_cidrs: None,
        }
    }
}

impl PushGatewayConfig {
    /// Creates a config from explicit values.
    ///
    /// Passing `Some(credentials)` programmatically selects FCM provider mode;
    /// passing `None` selects the default no-op provider.
    #[must_use]
    pub fn new(
        app_id: Option<AppId>,
        database_url: impl Into<String>,
        firebase_credentials: Option<FirebaseCredentials>,
    ) -> Self {
        Self {
            app_id,
            unsafe_allow_any_app_id_for_tests: false,
            database_url: database_url.into(),
            provider: firebase_credentials
                .map(|credentials| PushProviderConfig::Fcm(FcmProviderConfig::new(credentials)))
                .unwrap_or(PushProviderConfig::Noop),
            public_base_url: DEFAULT_PUBLIC_BASE_URL.to_owned(),
            operator_bind: None,
            operator_token: None,
            public_metrics_enabled: false,
            production_mode: false,
            open_self_registration_enabled: false,
            admission_allowed_recipients: BTreeSet::new(),
            legacy_notification_hook_enabled: true,
            outbox_worker_concurrency: 4,
            retention_seconds: DEFAULT_RETENTION_DAYS * SECONDS_PER_DAY,
            registration_ttl_seconds: DEFAULT_REGISTRATION_TTL_DAYS * SECONDS_PER_DAY,
            rate_limits: RateLimitConfig::default(),
            telemetry: None,
        }
    }

    /// Creates config with an explicit push-provider configuration.
    #[must_use]
    pub fn with_provider(mut self, provider: PushProviderConfig) -> Self {
        self.provider = provider;
        self
    }

    /// Creates a config with an explicit production public HTTPS origin.
    ///
    /// # Errors
    ///
    /// Returns [`PushGatewayConfigError::InvalidPublicBaseUrl`] when the value is
    /// not an absolute `https://` origin with no path, query, fragment, or userinfo.
    pub fn try_with_public_base_url(
        mut self,
        public_base_url: impl AsRef<str>,
    ) -> Result<Self, PushGatewayConfigError> {
        self.public_base_url = validate_public_base_url(public_base_url.as_ref())?;
        Ok(self)
    }

    /// Creates a config with an explicit local/test-only insecure public base URL.
    ///
    /// This is intentionally named as a test/development escape hatch. It accepts
    /// an empty base URL (path-only hook URLs) or loopback `http://` origins.
    ///
    /// # Errors
    ///
    /// Returns [`PushGatewayConfigError::InvalidPublicBaseUrl`] for any non-local
    /// insecure origin or malformed URL.
    pub fn try_with_local_test_public_base_url(
        mut self,
        public_base_url: impl AsRef<str>,
    ) -> Result<Self, PushGatewayConfigError> {
        self.public_base_url = validate_local_test_public_base_url(public_base_url.as_ref())?;
        Ok(self)
    }

    /// Sets the legacy local/test app-id bypass compatibility flag.
    ///
    /// Management and registration endpoint auth is Nostr/NIP-98 based and no
    /// longer consults request `app_id` values. This flag is retained only so
    /// older local `defe`/test configuration keeps parsing.
    #[must_use]
    pub fn with_unsafe_allow_any_app_id_for_tests(mut self, allow: bool) -> Self {
        self.unsafe_allow_any_app_id_for_tests = allow;
        self
    }

    /// Sets the outbox worker concurrency limit.
    #[must_use]
    pub fn with_outbox_worker_concurrency(mut self, outbox_worker_concurrency: usize) -> Self {
        self.outbox_worker_concurrency = outbox_worker_concurrency.max(1);
        self
    }

    /// Sets how long sensitive terminal delivery data is retained before purge.
    #[must_use]
    pub fn with_retention_seconds(mut self, retention_seconds: u64) -> Self {
        self.retention_seconds = retention_seconds.max(1);
        self
    }

    /// Sets how long an installation may go without a signed refresh.
    #[must_use]
    pub fn with_registration_ttl_seconds(mut self, registration_ttl_seconds: u64) -> Self {
        self.registration_ttl_seconds = registration_ttl_seconds.max(1);
        self
    }

    /// Sets an optional operator listener bind address.
    #[must_use]
    pub fn with_operator_bind(mut self, operator_bind: Option<SocketAddr>) -> Self {
        self.operator_bind = operator_bind;
        self
    }

    /// Sets an optional bearer token required for operator endpoints.
    #[must_use]
    pub fn with_operator_token(mut self, operator_token: Option<OperatorToken>) -> Self {
        self.operator_token = operator_token;
        self
    }

    /// Enables unauthenticated public `/metrics` exposure.
    #[must_use]
    pub fn with_public_metrics_enabled(mut self, enabled: bool) -> Self {
        self.public_metrics_enabled = enabled;
        self
    }

    /// Sets whether startup/config validation should enforce production safety gates.
    #[must_use]
    pub fn with_production_mode(mut self, enabled: bool) -> Self {
        self.production_mode = enabled;
        if enabled {
            self.legacy_notification_hook_enabled = false;
        }
        self
    }

    /// Explicitly enables open NIP-98-authenticated self-registration.
    ///
    /// Production defaults fail closed unless this is enabled or a static
    /// emergency allowlist is configured.
    #[must_use]
    pub fn with_open_self_registration_enabled(mut self, enabled: bool) -> Self {
        self.open_self_registration_enabled = enabled;
        self
    }

    /// Sets the recipient public keys allowed to use management/registration APIs.
    #[must_use]
    pub fn with_admission_allowed_recipients<I, S>(mut self, recipients: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.admission_allowed_recipients =
            recipients.into_iter().map(|value| value.into()).collect();
        self
    }

    /// Enables or disables the legacy direct notification endpoint.
    #[must_use]
    pub fn with_legacy_notification_hook_enabled(mut self, enabled: bool) -> Self {
        self.legacy_notification_hook_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_rate_limits(mut self, rate_limits: RateLimitConfig) -> Self {
        self.rate_limits = rate_limits;
        self
    }

    /// Enables the verified guardian telemetry receiver with complete config.
    #[must_use]
    pub fn with_telemetry_receiver(mut self, telemetry: TelemetryReceiverConfig) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Loads config from environment variables.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when FCM mode is explicitly requested but its
    /// credential/configuration environment is missing or invalid, or when the
    /// public base URL is not a valid production HTTPS origin or explicit local
    /// test origin.
    pub fn from_env() -> Result<Self, PushGatewayConfigError> {
        PushGatewayConfigArgs::try_parse_from(["push-gateway"])
            .map_err(PushGatewayConfigError::CommandLine)?
            .try_into_config()
    }

    /// Returns the configured legacy app id, if supplied.
    ///
    /// This value is no longer enforced for management/registration endpoint
    /// auth; recipient identity is derived from signed Nostr auth events.
    #[must_use]
    pub fn app_id(&self) -> Option<&AppId> {
        self.app_id.as_ref()
    }

    /// Returns whether the legacy local/test app-id bypass flag is enabled.
    ///
    /// The flag is retained for configuration compatibility only and is not
    /// endpoint authorization.
    #[must_use]
    pub fn unsafe_allow_any_app_id_for_tests(&self) -> bool {
        self.unsafe_allow_any_app_id_for_tests
    }

    /// Returns the database URL. Supports `sqlite://`, `postgres://`, and `postgresql://`.
    #[must_use]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// Returns Firebase credentials, if configured.
    #[must_use]
    pub fn firebase_credentials(&self) -> Option<&FirebaseCredentials> {
        match &self.provider {
            PushProviderConfig::Noop => None,
            PushProviderConfig::Fcm(config) => Some(config.credentials()),
        }
    }

    /// Returns the configured push provider.
    #[must_use]
    pub fn provider(&self) -> &PushProviderConfig {
        &self.provider
    }

    /// Returns public base URL used for generated hook capability URLs.
    #[must_use]
    pub fn public_base_url(&self) -> &str {
        &self.public_base_url
    }

    /// Returns the optional operator listener bind address.
    #[must_use]
    pub fn operator_bind(&self) -> Option<SocketAddr> {
        self.operator_bind
    }

    /// Returns the optional operator bearer token.
    #[must_use]
    pub fn operator_token(&self) -> Option<&OperatorToken> {
        self.operator_token.as_ref()
    }

    /// Returns whether unauthenticated public `/metrics` is enabled.
    #[must_use]
    pub fn public_metrics_enabled(&self) -> bool {
        self.public_metrics_enabled
    }

    /// Returns whether production safety validation is enabled.
    #[must_use]
    pub fn production_mode(&self) -> bool {
        self.production_mode
    }

    /// Returns whether a recipient is admitted to management/registration APIs.
    #[must_use]
    pub fn recipient_admitted(&self, recipient_id: &crate::RecipientId) -> bool {
        (!self.production_mode && self.admission_allowed_recipients.is_empty())
            || self.open_self_registration_enabled
            || self.admission_allowed_recipients.contains(&recipient_id.0)
    }

    /// Returns whether the legacy direct notification endpoint is enabled.
    #[must_use]
    pub fn legacy_notification_hook_enabled(&self) -> bool {
        self.legacy_notification_hook_enabled
    }

    /// Returns the durable outbox worker concurrency limit.
    #[must_use]
    pub fn outbox_worker_concurrency(&self) -> usize {
        self.outbox_worker_concurrency
    }

    /// Returns how long sensitive terminal delivery data is retained before purge.
    #[must_use]
    pub fn retention_seconds(&self) -> u64 {
        self.retention_seconds
    }

    /// Returns how long a registration remains eligible without refresh.
    #[must_use]
    pub fn registration_ttl_seconds(&self) -> u64 {
        self.registration_ttl_seconds
    }

    #[must_use]
    pub fn rate_limits(&self) -> &RateLimitConfig {
        &self.rate_limits
    }

    #[must_use]
    pub fn telemetry(&self) -> Option<&TelemetryReceiverConfig> {
        self.telemetry.as_ref()
    }

    /// Returns the configured push provider mode for readiness and diagnostics.
    #[must_use]
    pub fn provider_mode(&self) -> &'static str {
        match self.provider {
            PushProviderConfig::Noop => "noop",
            PushProviderConfig::Fcm(_) => "fcm",
        }
    }
}

impl Default for PushGatewayConfig {
    fn default() -> Self {
        Self::new(None, DEFAULT_DATABASE_URL, None)
    }
}

/// Push gateway command-line arguments and environment-backed configuration.
pub struct PushGatewayArgs {
    /// HTTP address the push gateway listens on.
    bind: SocketAddr,
    /// Runtime configuration shared by the HTTP server and delivery worker.
    config: PushGatewayConfigArgs,
}

impl std::fmt::Debug for PushGatewayArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushGatewayArgs")
            .field("bind", &self.bind)
            .field("config", &self.config)
            .finish()
    }
}

impl PushGatewayArgs {
    /// Parses command-line arguments and environment variables from the process.
    #[must_use]
    pub fn parse() -> Self {
        match Self::try_parse_from(std::env::args_os()) {
            Ok(args) => args,
            Err(err) => err.exit(),
        }
    }

    /// Parses command-line arguments and environment variables from an iterator.
    ///
    /// # Errors
    ///
    /// Returns a `clap` error when arguments or typed values are invalid.
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut command = PushGatewayCli::command();
        let matches = command.try_get_matches_from_mut(itr)?;
        let cli = PushGatewayCli::from_arg_matches(&matches)?;
        if cli.command.is_some() {
            return Err(clap::Error::raw(
                ErrorKind::InvalidSubcommand,
                "PushGatewayArgs parses server options only; use PushGatewayCommand for subcommands",
            ));
        }
        Ok(Self {
            bind: cli.bind,
            config: cli.config.with_credential_sources(&matches),
        })
    }

    /// Returns the HTTP bind address.
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Builds the runtime push-gateway configuration.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the selected provider or numeric limits are
    /// missing or invalid.
    pub fn into_config(self) -> Result<PushGatewayConfig, PushGatewayConfigError> {
        self.config.try_into_config()
    }
}

/// Top-level push-gateway CLI command.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum PushGatewayCommand {
    /// Run the HTTP server and durable outbox worker.
    Serve(PushGatewayArgs),
    /// Inspect or mutate delivery outbox rows.
    Outbox(PushGatewayOutboxCommand),
}

impl PushGatewayCommand {
    /// Parses command-line arguments and environment variables from the process.
    #[must_use]
    pub fn parse() -> Self {
        match Self::try_parse_from(std::env::args_os()) {
            Ok(command) => command,
            Err(err) => err.exit(),
        }
    }

    /// Parses command-line arguments and environment variables from an iterator.
    ///
    /// # Errors
    ///
    /// Returns a `clap` error when arguments or typed values are invalid.
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut command = PushGatewayCli::command();
        let matches = command.try_get_matches_from_mut(itr)?;
        let cli = PushGatewayCli::from_arg_matches(&matches)?;
        let config = cli.config.with_credential_sources(&matches);
        Ok(match cli.command {
            None => Self::Serve(PushGatewayArgs {
                bind: cli.bind,
                config,
            }),
            Some(PushGatewaySubcommand::Outbox(command)) => {
                Self::Outbox(PushGatewayOutboxCommand {
                    database_url: config.database_url,
                    action: command.action,
                })
            }
        })
    }
}

/// Delivery outbox operator/admin command with database configuration.
pub struct PushGatewayOutboxCommand {
    /// Database URL parsed from CLI/env.
    database_url: String,
    /// Requested outbox action.
    action: PushGatewayOutboxAction,
}

impl std::fmt::Debug for PushGatewayOutboxCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushGatewayOutboxCommand")
            .field("database_url", &redact_database_url(&self.database_url))
            .field("action", &self.action)
            .finish()
    }
}

impl PushGatewayOutboxCommand {
    /// Returns database URL and requested action.
    pub fn into_parts(self) -> (String, PushGatewayOutboxAction) {
        (self.database_url, self.action)
    }
}

/// Delivery outbox action requested by the operator.
#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum PushGatewayOutboxAction {
    /// List sanitized dead-letter rows.
    #[command(name = "list-dead-letter")]
    ListDeadLetter {
        /// Maximum rows to display.
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(i64).range(1..=1000))]
        limit: i64,
        /// Emit JSON instead of a human-readable table.
        #[arg(long)]
        json: bool,
    },
    /// Count dead-letter rows by sanitized reason.
    #[command(name = "dead-letter-reasons")]
    DeadLetterReasons {
        /// Emit JSON instead of a human-readable table.
        #[arg(long)]
        json: bool,
    },
    /// Move selected dead-letter rows back to pending.
    #[command(name = "replay-dead-letter")]
    ReplayDeadLetter(DeadLetterMutationArgs),
    /// Permanently delete selected dead-letter rows.
    #[command(name = "delete-dead-letter")]
    DeleteDeadLetter(DeadLetterMutationArgs),
}

/// Arguments shared by dead-letter replay/delete mutations.
#[derive(Args, Clone, Debug, Eq, PartialEq)]
pub struct DeadLetterMutationArgs {
    /// Explicit outbox ids to mutate.
    #[arg(long = "outbox-id")]
    pub outbox_ids: Vec<String>,
    /// Optional bounded oldest-first limit when no ids are supplied.
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..=1000))]
    pub limit: Option<i64>,
    /// Optional sanitized last-error reason filter.
    #[arg(long)]
    pub reason: Option<String>,
    /// Preview selected rows without mutating them.
    #[arg(long)]
    pub dry_run: bool,
    /// Required confirmation for mutations.
    #[arg(long)]
    pub yes: bool,
    /// Print selected/mutated rows as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Clap parser shape for push gateway command-line arguments.
#[derive(Parser)]
#[command(version, about = "HTTP push notification hook gateway")]
struct PushGatewayCli {
    /// HTTP address the push gateway listens on.
    #[arg(
        long,
        env = "PUSH_GATEWAY_BIND",
        hide_env_values = true,
        default_value = DEFAULT_BIND_ADDR
    )]
    bind: SocketAddr,
    /// Runtime configuration shared by the HTTP server and delivery worker.
    #[command(flatten)]
    config: PushGatewayConfigArgs,
    /// Optional operator/admin command. Omit to run the HTTP server.
    #[command(subcommand)]
    command: Option<PushGatewaySubcommand>,
}

#[derive(Subcommand)]
enum PushGatewaySubcommand {
    /// Inspect or manage delivery outbox rows.
    Outbox(PushGatewayOutboxCli),
}

/// Delivery outbox operator/admin CLI.
#[derive(Parser)]
struct PushGatewayOutboxCli {
    /// Delivery outbox action.
    #[command(subcommand)]
    action: PushGatewayOutboxAction,
}

/// Environment-backed push gateway runtime configuration arguments.
#[derive(Parser)]
struct PushGatewayConfigArgs {
    /// Legacy/deprecated compatibility option; management and registration auth
    /// no longer uses request app-id equality.
    #[arg(long, env = "PUSH_GATEWAY_APP_ID", hide_env_values = true)]
    app_id: Option<String>,
    /// Legacy/deprecated local/test compatibility flag; no longer authorizes endpoints.
    #[arg(
        long,
        env = "PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS",
        hide_env_values = true,
        default_value_t = false
    )]
    unsafe_allow_any_app_id_for_tests: bool,
    /// SQLite or PostgreSQL database URL.
    #[arg(
        long,
        env = "PUSH_GATEWAY_DATABASE_URL",
        hide_env_values = true,
        default_value = DEFAULT_DATABASE_URL
    )]
    database_url: String,
    /// Public HTTPS origin prepended to generated hook invocation paths.
    #[arg(
        long,
        env = "PUSH_GATEWAY_PUBLIC_BASE_URL",
        hide_env_values = true,
        default_value = DEFAULT_PUBLIC_BASE_URL
    )]
    public_base_url: String,
    /// Optional bind address for operator-only `/ready` and `/metrics`.
    #[arg(long, env = "PUSH_GATEWAY_OPERATOR_BIND", hide_env_values = true)]
    operator_bind: Option<SocketAddr>,
    /// Optional bearer token required for operator `/ready` and `/metrics`.
    #[arg(long, env = "PUSH_GATEWAY_OPERATOR_TOKEN", hide_env_values = true)]
    operator_token: Option<String>,
    /// Explicitly expose `/metrics` on the public listener without operator auth.
    #[arg(
        long,
        env = "PUSH_GATEWAY_PUBLIC_METRICS_ENABLED",
        hide_env_values = true,
        default_value_t = false
    )]
    public_metrics_enabled: bool,
    /// Manifold environment used for live PeerBadge verification. Supplying
    /// any telemetry option requires the complete set.
    #[arg(
        long,
        env = "PUSH_GATEWAY_TELEMETRY_MANIFOLD_ENVIRONMENT",
        hide_env_values = true
    )]
    telemetry_manifold_environment: Option<ManifoldEnvironment>,
    /// 32-byte hex AES-GCM key for target/invite encryption at rest.
    #[arg(
        long,
        env = "PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY",
        hide_env_values = true
    )]
    telemetry_encryption_key: Option<String>,
    /// Enforce production safety gates at config load time.
    #[arg(
        long,
        env = "PUSH_GATEWAY_PRODUCTION_MODE",
        hide_env_values = true,
        default_value_t = false
    )]
    production_mode: bool,
    /// Permit any correctly NIP-98-authenticated installation to self-register.
    #[arg(
        long,
        env = "PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED",
        hide_env_values = true,
        default_value_t = false
    )]
    open_self_registration_enabled: bool,
    /// Emergency comma-separated Nostr public-key admission restriction.
    ///
    /// Production permits an empty value only when open self-registration is
    /// explicitly enabled.
    #[arg(
        long,
        env = "PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS",
        hide_env_values = true
    )]
    admission_allowed_recipients: Option<String>,
    /// Enable legacy `/hooks/notification`, which acknowledges but does not enqueue delivery.
    #[arg(
        long,
        env = "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
        hide_env_values = true,
        action = ArgAction::Set,
        default_value_t = true
    )]
    legacy_notification_hook_enabled: bool,
    /// Permit local/test public-base URLs that are not production-safe.
    #[arg(
        long,
        env = "PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL",
        hide_env_values = true,
        default_value_t = false
    )]
    allow_insecure_public_base_url: bool,
    /// Push provider mode: `noop` or `fcm`.
    #[arg(
        long,
        env = "PUSH_GATEWAY_PROVIDER",
        hide_env_values = true,
        default_value = DEFAULT_PROVIDER
    )]
    provider: String,
    /// Maximum number of concurrent outbox deliveries processed by the worker.
    #[arg(
        long,
        env = "PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY",
        hide_env_values = true,
        default_value = DEFAULT_OUTBOX_WORKER_CONCURRENCY
    )]
    outbox_worker_concurrency: String,
    /// Retain sensitive terminal push data for this many days before startup purge.
    #[arg(
        long,
        env = "PUSH_GATEWAY_RETENTION_DAYS",
        default_value_t = DEFAULT_RETENTION_DAYS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    retention_days: u64,
    /// Signed registration refresh interval before an installation is stale.
    #[arg(
        long,
        env = "PUSH_GATEWAY_REGISTRATION_TTL_DAYS",
        default_value_t = DEFAULT_REGISTRATION_TTL_DAYS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    registration_ttl_days: u64,
    /// Valid signed management requests accepted per source prefix before replay-cache insertion.
    #[arg(long, env = "PUSH_GATEWAY_AUTH_EVENTS_PER_SOURCE_PREFIX", default_value_t = DEFAULT_AUTH_EVENTS_PER_SOURCE_PREFIX)]
    auth_events_per_source_prefix: u64,
    /// Authentication source-prefix fixed-window length in seconds.
    #[arg(long, env = "PUSH_GATEWAY_AUTH_EVENT_WINDOW_SECONDS", default_value_t = DEFAULT_AUTH_EVENT_WINDOW_SECONDS)]
    auth_event_window_seconds: u64,
    /// Public hook invocations accepted per source IPv4 address or IPv6 /64 per window.
    #[arg(long, env = "PUSH_GATEWAY_HOOK_INVOCATIONS_PER_SOURCE_PREFIX", default_value_t = DEFAULT_HOOK_INVOCATION_LIMIT)]
    hook_invocations_per_source_prefix: u64,
    /// Public hook invocations accepted per hook token per window.
    #[arg(long, env = "PUSH_GATEWAY_HOOK_INVOCATIONS_PER_HOOK", default_value_t = DEFAULT_HOOK_INVOCATION_LIMIT)]
    hook_invocations_per_hook: u64,
    /// Hook invocation fixed-window length in seconds.
    #[arg(long, env = "PUSH_GATEWAY_HOOK_INVOCATION_WINDOW_SECONDS", default_value_t = DEFAULT_RATE_LIMIT_WINDOW_SECONDS)]
    hook_invocation_window_seconds: u64,
    /// Hook creations accepted per recipient per window.
    #[arg(long, env = "PUSH_GATEWAY_HOOK_CREATIONS_PER_RECIPIENT", default_value_t = DEFAULT_HOOK_CREATION_LIMIT)]
    hook_creations_per_recipient: u64,
    /// Hook creation fixed-window length in seconds.
    #[arg(long, env = "PUSH_GATEWAY_HOOK_CREATION_WINDOW_SECONDS", default_value_t = DEFAULT_RATE_LIMIT_WINDOW_SECONDS)]
    hook_creation_window_seconds: u64,
    /// Registration writes accepted per recipient/source-prefix per window.
    #[arg(long, env = "PUSH_GATEWAY_REGISTRATION_CHANGES_PER_RECIPIENT_SOURCE", default_value_t = DEFAULT_REGISTRATION_CHANGE_LIMIT)]
    registration_changes_per_recipient_source: u64,
    /// Registration writes accepted per source-prefix across all recipient keys per window.
    #[arg(long, env = "PUSH_GATEWAY_REGISTRATION_CHANGES_PER_SOURCE_PREFIX", default_value_t = DEFAULT_REGISTRATION_CHANGE_LIMIT * 3)]
    registration_changes_per_source_prefix: u64,
    /// Registration write fixed-window length in seconds.
    #[arg(long, env = "PUSH_GATEWAY_REGISTRATION_CHANGE_WINDOW_SECONDS", default_value_t = DEFAULT_RATE_LIMIT_WINDOW_SECONDS)]
    registration_change_window_seconds: u64,
    /// Maximum active hooks per recipient (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_ACTIVE_HOOKS_PER_RECIPIENT", default_value_t = DEFAULT_MAX_ACTIVE_HOOKS_PER_RECIPIENT)]
    max_active_hooks_per_recipient: u64,
    /// Maximum active installations per recipient (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_ACTIVE_INSTALLATIONS_PER_RECIPIENT", default_value_t = DEFAULT_MAX_ACTIVE_INSTALLATIONS_PER_RECIPIENT)]
    max_active_installations_per_recipient: u64,
    /// Maximum active hooks across the gateway (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_ACTIVE_HOOKS_GLOBAL", default_value_t = DEFAULT_MAX_ACTIVE_HOOKS_GLOBAL)]
    max_active_hooks_global: u64,
    /// Maximum active installations across the gateway (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_ACTIVE_INSTALLATIONS_GLOBAL", default_value_t = DEFAULT_MAX_ACTIVE_INSTALLATIONS_GLOBAL)]
    max_active_installations_global: u64,
    /// Maximum physical hook rows across the gateway (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_HOOK_ROWS_GLOBAL", default_value_t = DEFAULT_MAX_HOOK_ROWS_GLOBAL)]
    max_hook_rows_global: u64,
    /// Maximum physical registration rows across the gateway (0 disables this cap).
    #[arg(long, env = "PUSH_GATEWAY_MAX_REGISTRATION_ROWS_GLOBAL", default_value_t = DEFAULT_MAX_REGISTRATION_ROWS_GLOBAL)]
    max_registration_rows_global: u64,
    /// Maximum stale/terminal rows reclaimed in one admission transaction.
    #[arg(long, env = "PUSH_GATEWAY_ADMISSION_GC_BATCH_SIZE", default_value_t = DEFAULT_ADMISSION_GC_BATCH_SIZE)]
    admission_gc_batch_size: u64,
    /// Maximum global active outbox backlog before public hooks return 503 (0 disables).
    #[arg(
        long,
        env = "PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG",
        default_value_t = 0
    )]
    max_global_outbox_backlog: u64,
    /// Maximum active outbox backlog per recipient before public hooks return 429 (0 disables).
    #[arg(
        long,
        env = "PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG",
        default_value_t = 0
    )]
    max_recipient_outbox_backlog: u64,
    /// Trusted direct proxy CIDRs allowed to supply X-Forwarded-For/Forwarded client IPs.
    #[arg(long, env = "PUSH_GATEWAY_TRUSTED_PROXY_CIDRS", hide_env_values = true)]
    trusted_proxy_cidrs: Option<String>,
    /// Path to a Firebase service-account JSON file for FCM mode.
    #[arg(long, env = "FCM_SERVICE_ACCOUNT_FILE", hide_env_values = true)]
    fcm_service_account_file: Option<PathBuf>,
    /// Raw Firebase service-account JSON for FCM mode.
    #[arg(long, env = "FCM_SERVICE_ACCOUNT_JSON", hide_env_values = true)]
    fcm_service_account_json: Option<String>,
    /// Legacy raw Firebase service-account JSON alias for FCM mode.
    #[arg(
        long,
        env = "FIREBASE_CREDENTIALS_JSON",
        hide = true,
        hide_env_values = true
    )]
    firebase_credentials_json: Option<String>,
    /// FCM endpoint base override for fake-server tests.
    #[arg(long, env = "FCM_SEND_ENDPOINT_BASE", hide_env_values = true)]
    fcm_send_endpoint_base: Option<String>,
    /// Maximum number of concurrent FCM HTTP requests.
    #[arg(long, env = "FCM_MAX_CONCURRENCY", hide_env_values = true)]
    fcm_max_concurrency: Option<String>,
    /// Source used for the FCM service-account file value.
    #[arg(skip)]
    fcm_service_account_file_source: Option<ValueSource>,
    /// Source used for the FCM service-account JSON value.
    #[arg(skip)]
    fcm_service_account_json_source: Option<ValueSource>,
    /// Source used for the legacy Firebase credential JSON value.
    #[arg(skip)]
    firebase_credentials_json_source: Option<ValueSource>,
}

impl std::fmt::Debug for PushGatewayConfigArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushGatewayConfigArgs")
            .field("app_id", &self.app_id.as_ref().map(|_| "<redacted>"))
            .field(
                "unsafe_allow_any_app_id_for_tests",
                &self.unsafe_allow_any_app_id_for_tests,
            )
            .field("database_url", &redact_database_url(&self.database_url))
            .field("public_base_url", &self.public_base_url)
            .field("operator_bind", &self.operator_bind)
            .field(
                "operator_token",
                &self.operator_token.as_ref().map(|_| "<redacted>"),
            )
            .field("public_metrics_enabled", &self.public_metrics_enabled)
            .field(
                "telemetry_manifold_environment",
                &self.telemetry_manifold_environment,
            )
            .field(
                "telemetry_encryption_key",
                &self.telemetry_encryption_key.as_ref().map(|_| "<redacted>"),
            )
            .field("production_mode", &self.production_mode)
            .field(
                "open_self_registration_enabled",
                &self.open_self_registration_enabled,
            )
            .field(
                "admission_allowed_recipients",
                &self
                    .admission_allowed_recipients
                    .as_ref()
                    .map(|_| "<redacted>"),
            )
            .field(
                "legacy_notification_hook_enabled",
                &self.legacy_notification_hook_enabled,
            )
            .field(
                "allow_insecure_public_base_url",
                &self.allow_insecure_public_base_url,
            )
            .field("provider", &self.provider)
            .field("outbox_worker_concurrency", &self.outbox_worker_concurrency)
            .field("retention_days", &self.retention_days)
            .field("registration_ttl_days", &self.registration_ttl_days)
            .field(
                "auth_events_per_source_prefix",
                &self.auth_events_per_source_prefix,
            )
            .field("auth_event_window_seconds", &self.auth_event_window_seconds)
            .field(
                "hook_invocations_per_source_prefix",
                &self.hook_invocations_per_source_prefix,
            )
            .field("hook_invocations_per_hook", &self.hook_invocations_per_hook)
            .field(
                "hook_invocation_window_seconds",
                &self.hook_invocation_window_seconds,
            )
            .field(
                "hook_creations_per_recipient",
                &self.hook_creations_per_recipient,
            )
            .field(
                "hook_creation_window_seconds",
                &self.hook_creation_window_seconds,
            )
            .field(
                "registration_changes_per_recipient_source",
                &self.registration_changes_per_recipient_source,
            )
            .field(
                "registration_changes_per_source_prefix",
                &self.registration_changes_per_source_prefix,
            )
            .field(
                "registration_change_window_seconds",
                &self.registration_change_window_seconds,
            )
            .field(
                "max_active_hooks_per_recipient",
                &self.max_active_hooks_per_recipient,
            )
            .field(
                "max_active_installations_per_recipient",
                &self.max_active_installations_per_recipient,
            )
            .field("max_active_hooks_global", &self.max_active_hooks_global)
            .field(
                "max_active_installations_global",
                &self.max_active_installations_global,
            )
            .field("max_hook_rows_global", &self.max_hook_rows_global)
            .field(
                "max_registration_rows_global",
                &self.max_registration_rows_global,
            )
            .field("admission_gc_batch_size", &self.admission_gc_batch_size)
            .field("max_global_outbox_backlog", &self.max_global_outbox_backlog)
            .field(
                "max_recipient_outbox_backlog",
                &self.max_recipient_outbox_backlog,
            )
            .field(
                "trusted_proxy_cidrs",
                &self.trusted_proxy_cidrs.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "fcm_service_account_file",
                &self.fcm_service_account_file.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "fcm_service_account_json",
                &self.fcm_service_account_json.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "firebase_credentials_json",
                &self
                    .firebase_credentials_json
                    .as_ref()
                    .map(|_| "<redacted>"),
            )
            .field("fcm_send_endpoint_base", &self.fcm_send_endpoint_base)
            .field("fcm_max_concurrency", &self.fcm_max_concurrency)
            .finish_non_exhaustive()
    }
}

impl PushGatewayConfigArgs {
    /// Records clap value sources needed for cross-field precedence.
    fn with_credential_sources(mut self, matches: &clap::ArgMatches) -> Self {
        self.fcm_service_account_file_source = matches.value_source("fcm_service_account_file");
        self.fcm_service_account_json_source = matches.value_source("fcm_service_account_json");
        self.firebase_credentials_json_source = matches.value_source("firebase_credentials_json");
        self
    }

    /// Builds the runtime configuration from parsed CLI/env arguments.
    fn try_into_config(self) -> Result<PushGatewayConfig, PushGatewayConfigError> {
        let provider = self.provider_config()?;
        let public_base_url = if self.allow_insecure_public_base_url {
            validate_public_base_url(&self.public_base_url)
                .or_else(|_| validate_local_test_public_base_url(&self.public_base_url))?
        } else {
            validate_public_base_url(&self.public_base_url)?
        };
        let telemetry = parse_telemetry_config(
            self.telemetry_manifold_environment,
            self.telemetry_encryption_key.as_deref(),
        )?;
        let mut config = PushGatewayConfig::new(self.app_id.map(AppId), self.database_url, None)
            .with_unsafe_allow_any_app_id_for_tests(self.unsafe_allow_any_app_id_for_tests)
            .with_provider(provider)
            .with_operator_bind(self.operator_bind)
            .with_operator_token(self.operator_token.and_then(OperatorToken::new))
            .with_public_metrics_enabled(self.public_metrics_enabled)
            .with_production_mode(self.production_mode)
            .with_open_self_registration_enabled(self.open_self_registration_enabled)
            .with_admission_allowed_recipients(parse_allowed_recipients(
                self.admission_allowed_recipients.as_deref(),
            )?)
            .with_legacy_notification_hook_enabled(self.legacy_notification_hook_enabled)
            .with_outbox_worker_concurrency(parse_concurrency(&self.outbox_worker_concurrency)?)
            .with_retention_seconds(parse_retention_days(self.retention_days)?)
            .with_registration_ttl_seconds(parse_registration_ttl_days(self.registration_ttl_days)?)
            .with_rate_limits(RateLimitConfig {
                auth_events_per_source_prefix: self.auth_events_per_source_prefix,
                auth_event_window_seconds: self.auth_event_window_seconds,
                hook_invocations_per_source_prefix: self.hook_invocations_per_source_prefix,
                hook_invocations_per_hook: self.hook_invocations_per_hook,
                hook_invocation_window_seconds: self.hook_invocation_window_seconds,
                hook_creations_per_recipient: self.hook_creations_per_recipient,
                hook_creation_window_seconds: self.hook_creation_window_seconds,
                registration_changes_per_recipient_source: self
                    .registration_changes_per_recipient_source,
                registration_changes_per_source_prefix: self.registration_changes_per_source_prefix,
                registration_change_window_seconds: self.registration_change_window_seconds,
                max_active_hooks_per_recipient: self.max_active_hooks_per_recipient,
                max_active_installations_per_recipient: self.max_active_installations_per_recipient,
                max_active_hooks_global: self.max_active_hooks_global,
                max_active_installations_global: self.max_active_installations_global,
                max_hook_rows_global: self.max_hook_rows_global,
                max_registration_rows_global: self.max_registration_rows_global,
                admission_gc_batch_size: self.admission_gc_batch_size,
                max_global_outbox_backlog: self.max_global_outbox_backlog,
                max_recipient_outbox_backlog: self.max_recipient_outbox_backlog,
                trusted_proxy_cidrs: self
                    .trusted_proxy_cidrs
                    .as_deref()
                    .map(TrustedProxyCidrs::parse_csv)
                    .transpose()
                    .map_err(PushGatewayConfigError::InvalidTrustedProxyCidrs)?,
            });
        config.telemetry = telemetry;
        config.public_base_url = public_base_url;
        validate_production_safety(&config)?;
        Ok(config)
    }

    /// Builds the push provider configuration.
    fn provider_config(&self) -> Result<PushProviderConfig, PushGatewayConfigError> {
        let mode = self.provider.trim().to_ascii_lowercase();
        match mode.as_str() {
            "" | "noop" => return Ok(PushProviderConfig::Noop),
            "fcm" => {}
            _ => return Err(PushGatewayConfigError::UnknownProviderMode(mode)),
        }

        let credential_json =
            if self.fcm_service_account_file_source == Some(ValueSource::CommandLine) {
                let path = self
                    .fcm_service_account_file
                    .as_ref()
                    .expect("clap source only exists with a value");
                fs::read_to_string(path).map_err(PushGatewayConfigError::ReadCredentialFile)?
            } else if self.fcm_service_account_json_source == Some(ValueSource::CommandLine) {
                self.fcm_service_account_json
                    .clone()
                    .expect("clap source only exists with a value")
            } else if self.firebase_credentials_json_source == Some(ValueSource::CommandLine) {
                self.firebase_credentials_json
                    .clone()
                    .expect("clap source only exists with a value")
            } else if let Some(path) = &self.fcm_service_account_file {
                fs::read_to_string(path).map_err(PushGatewayConfigError::ReadCredentialFile)?
            } else if let Some(json) = &self.fcm_service_account_json {
                json.clone()
            } else if let Some(json) = &self.firebase_credentials_json {
                json.clone()
            } else {
                return Err(PushGatewayConfigError::MissingFcmCredentials);
            };

        let credentials = FirebaseCredentials::from_json(&credential_json)
            .map_err(PushGatewayConfigError::InvalidFcmCredentials)?;
        let mut config = FcmProviderConfig::new(credentials);
        if let Some(base) = &self.fcm_send_endpoint_base {
            config = config.with_send_endpoint_base(base);
        }
        if let Some(concurrency) = &self.fcm_max_concurrency {
            config = config.with_max_concurrency(parse_concurrency(concurrency)?);
        }

        Ok(PushProviderConfig::Fcm(config))
    }
}

/// Sanitized configuration loading error.
#[derive(Debug)]
pub enum PushGatewayConfigError {
    /// Command-line or environment parsing failed.
    CommandLine(clap::Error),
    /// FCM mode was requested without credentials.
    MissingFcmCredentials,
    /// Credential file could not be read.
    ReadCredentialFile(std::io::Error),
    /// Credential JSON was invalid.
    InvalidFcmCredentials(FirebaseCredentialsError),
    /// Concurrency limit was invalid.
    InvalidConcurrency,
    /// Retention horizon was invalid.
    InvalidRetentionDays,
    /// Registration freshness days overflow seconds.
    InvalidRegistrationTtlDays,
    /// Provider mode was not recognized.
    UnknownProviderMode(String),
    /// Public base URL is not a production-safe HTTPS origin.
    InvalidPublicBaseUrl,
    /// Trusted proxy CIDR list is invalid.
    InvalidTrustedProxyCidrs(String),
    /// Admission allowlist contains an invalid recipient id.
    InvalidAdmissionAllowedRecipients(String),
    /// Telemetry receiver configuration was partial or unsafe.
    InvalidTelemetryConfiguration,
    /// Production mode requires an explicit production-safe setting.
    ProductionSafety(String),
}

impl std::fmt::Display for PushGatewayConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandLine(err) => write!(formatter, "{err}"),
            Self::MissingFcmCredentials => formatter.write_str("missing FCM credentials"),
            Self::ReadCredentialFile(_) => {
                formatter.write_str("failed to read FCM credentials file")
            }
            Self::InvalidFcmCredentials(err) => write!(formatter, "{err}"),
            Self::InvalidConcurrency => formatter.write_str("invalid concurrency limit"),
            Self::InvalidRetentionDays => formatter.write_str("invalid retention days"),
            Self::InvalidRegistrationTtlDays => {
                formatter.write_str("invalid registration TTL days")
            }
            Self::UnknownProviderMode(_) => formatter.write_str("unknown push provider mode"),
            Self::InvalidPublicBaseUrl => formatter.write_str(
                "public base URL must be an absolute https:// origin; set \
                 PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL=true only for local tests",
            ),
            Self::InvalidTrustedProxyCidrs(err) => write!(formatter, "{err}"),
            Self::InvalidAdmissionAllowedRecipients(message) => formatter.write_str(message),
            Self::InvalidTelemetryConfiguration => {
                formatter.write_str("telemetry requires an environment and a 64-hex encryption key")
            }
            Self::ProductionSafety(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PushGatewayConfigError {}

fn parse_concurrency(value: &str) -> Result<usize, PushGatewayConfigError> {
    value
        .parse()
        .map_err(|_| PushGatewayConfigError::InvalidConcurrency)
}

fn parse_retention_days(value: u64) -> Result<u64, PushGatewayConfigError> {
    value
        .checked_mul(SECONDS_PER_DAY)
        .ok_or(PushGatewayConfigError::InvalidRetentionDays)
}

fn parse_registration_ttl_days(value: u64) -> Result<u64, PushGatewayConfigError> {
    value
        .checked_mul(SECONDS_PER_DAY)
        .ok_or(PushGatewayConfigError::InvalidRegistrationTtlDays)
}

fn parse_allowed_recipients(
    value: Option<&str>,
) -> Result<BTreeSet<String>, PushGatewayConfigError> {
    let recipients = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if let Some(invalid) = recipients
        .iter()
        .find(|recipient| !is_canonical_nostr_pubkey_hex(recipient))
    {
        return Err(PushGatewayConfigError::InvalidAdmissionAllowedRecipients(
            format!("invalid admitted recipient id: {invalid}"),
        ));
    }
    Ok(recipients)
}

fn parse_telemetry_config(
    environment: Option<ManifoldEnvironment>,
    encryption_key: Option<&str>,
) -> Result<Option<TelemetryReceiverConfig>, PushGatewayConfigError> {
    let supplied = [environment.is_some(), encryption_key.is_some()];
    if supplied.iter().all(|value| !value) {
        return Ok(None);
    }
    if !supplied.iter().all(|value| *value) {
        return Err(PushGatewayConfigError::InvalidTelemetryConfiguration);
    }
    TelemetryReceiverConfig::new(
        environment.expect("checked complete telemetry config"),
        encryption_key.expect("checked complete telemetry config"),
    )
    .map(Some)
}

fn is_canonical_nostr_pubkey_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate_production_safety(config: &PushGatewayConfig) -> Result<(), PushGatewayConfigError> {
    if !config.production_mode {
        return Ok(());
    }
    if !matches!(config.provider, PushProviderConfig::Fcm(_)) {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires PUSH_GATEWAY_PROVIDER=fcm".to_owned(),
        ));
    }
    if config.public_base_url == DEFAULT_PUBLIC_BASE_URL
        || !config.public_base_url.starts_with("https://")
    {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires explicit HTTPS PUSH_GATEWAY_PUBLIC_BASE_URL".to_owned(),
        ));
    }
    if config.telemetry.is_some()
        && config.operator_bind.is_none()
        && config.operator_token.is_none()
    {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production telemetry requires PUSH_GATEWAY_OPERATOR_BIND or PUSH_GATEWAY_OPERATOR_TOKEN"
                .to_owned(),
        ));
    }
    let allowlist_enabled = !config.admission_allowed_recipients.is_empty();
    if allowlist_enabled == config.open_self_registration_enabled {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires exactly one admission mode: either a non-empty PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS allowlist or PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true".to_owned(),
        ));
    }
    if config.rate_limits.max_global_outbox_backlog == 0
        || config.rate_limits.max_recipient_outbox_backlog == 0
    {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires nonzero global and per-recipient outbox backlog caps"
                .to_owned(),
        ));
    }
    if config.rate_limits.auth_events_per_source_prefix == 0
        || config.rate_limits.auth_event_window_seconds == 0
        || config.rate_limits.hook_invocations_per_source_prefix == 0
        || config.rate_limits.hook_invocations_per_hook == 0
        || config.rate_limits.hook_invocation_window_seconds == 0
        || config.rate_limits.hook_creations_per_recipient == 0
        || config.rate_limits.hook_creation_window_seconds == 0
        || config.rate_limits.registration_changes_per_source_prefix == 0
        || config.rate_limits.registration_changes_per_recipient_source == 0
        || config.rate_limits.registration_change_window_seconds == 0
        || config.rate_limits.max_active_hooks_per_recipient == 0
        || config.rate_limits.max_active_installations_per_recipient == 0
        || config.rate_limits.max_active_hooks_global == 0
        || config.rate_limits.max_active_installations_global == 0
        || config.rate_limits.max_hook_rows_global == 0
        || config.rate_limits.max_registration_rows_global == 0
        || config.rate_limits.admission_gc_batch_size == 0
    {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires nonzero source, recipient, and global admission caps"
                .to_owned(),
        ));
    }
    if config.legacy_notification_hook_enabled {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false"
                .to_owned(),
        ));
    }
    let PushProviderConfig::Fcm(fcm_config) = &config.provider else {
        unreachable!("provider checked above");
    };
    if fcm_config.send_endpoint_base() != DEFAULT_FCM_SEND_ENDPOINT_BASE {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode does not permit FCM_SEND_ENDPOINT_BASE overrides".to_owned(),
        ));
    }
    if fcm_config.credentials().token_uri() != DEFAULT_FCM_TOKEN_URI {
        return Err(PushGatewayConfigError::ProductionSafety(
            "production mode requires the default Google OAuth token URI".to_owned(),
        ));
    }
    Ok(())
}

fn validate_public_base_url(public_base_url: &str) -> Result<String, PushGatewayConfigError> {
    let url = parse_public_base_url(public_base_url)?;
    if url.scheme() != "https" {
        return Err(PushGatewayConfigError::InvalidPublicBaseUrl);
    }
    Ok(origin_string(&url))
}

fn validate_local_test_public_base_url(
    public_base_url: &str,
) -> Result<String, PushGatewayConfigError> {
    let public_base_url = public_base_url.trim();
    if public_base_url.is_empty() {
        return Ok(String::new());
    }

    let url = parse_public_base_url(public_base_url)?;
    if url.scheme() != "http" {
        return Err(PushGatewayConfigError::InvalidPublicBaseUrl);
    }
    let Some(host) = url.host_str() else {
        return Err(PushGatewayConfigError::InvalidPublicBaseUrl);
    };
    if !matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
        return Err(PushGatewayConfigError::InvalidPublicBaseUrl);
    }
    Ok(origin_string(&url))
}

fn parse_public_base_url(public_base_url: &str) -> Result<Url, PushGatewayConfigError> {
    let url = Url::parse(public_base_url.trim())
        .map_err(|_| PushGatewayConfigError::InvalidPublicBaseUrl)?;
    if url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PushGatewayConfigError::InvalidPublicBaseUrl);
    }
    Ok(url)
}

fn origin_string(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_owned()
}

fn redact_database_url(database_url: &str) -> String {
    let Some((scheme, rest)) = database_url.split_once("://") else {
        return "<redacted>".to_owned();
    };
    let Some(at_index) = rest.find('@') else {
        return database_url.to_owned();
    };
    format!("{scheme}://<redacted>@{}", &rest[at_index + 1..])
}

#[cfg(test)]
mod tests;
