//! Boot configuration: CLI arguments, environment variables, and the data-dir
//! layout.
//!
//! Most settings have an environment fallback so deployment wiring need not
//! pass arguments. The canonical Manifold environment is not optional: it must
//! be stated, by flag or by `FLIP_MANIFOLD_ENVIRONMENT`, so no deployment can
//! silently select development trust.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Args, Parser, Subcommand};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;

pub(crate) const FLIP_DATA_DIR_ENV: &str = "FLIP_DATA_DIR";
pub(crate) const FLIP_SQLITE_PATH_ENV: &str = "FLIP_SQLITE_PATH";
pub(crate) const FLIP_ADMIN_BIND_ADDRESS_ENV: &str = "FLIP_ADMIN_BIND_ADDRESS";
pub(crate) const FLIP_PUBLIC_BIND_ADDRESS_ENV: &str = "FLIP_PUBLIC_BIND_ADDRESS";
pub(crate) const FLIP_BOOTSTRAP_ADMIN_TOKEN_ENV: &str = "FLIP_BOOTSTRAP_ADMIN_TOKEN";
pub(crate) const FLIP_SECRET_KEY_ENV: &str = "FLIP_SECRET_KEY";
pub(crate) const FLIP_RESTORE_MODE_ENV: &str = "FLIP_RESTORE_MODE";
pub(crate) const FLIP_BOOTSTRAP_TOKEN_FALLBACK_ENV: &str = "FLIP_ALLOW_BOOTSTRAP_TOKEN_FALLBACK";
pub(crate) const FLIP_PROVIDER_NOSTR_SECRET_KEY_ENV: &str = "FLIP_PROVIDER_NOSTR_SECRET_KEY";
pub(crate) const FLIP_TRUST_FIXTURES_ENV: &str = "FLIP_TRUST_FIXTURES";
pub(crate) const FLIP_MANIFOLD_ENVIRONMENT_ENV: &str = "FLIP_MANIFOLD_ENVIRONMENT";
pub(crate) const FLIP_MAX_OPEN_TARGET_CLIENTS_ENV: &str = "FLIP_MAX_OPEN_TARGET_CLIENTS";
pub(crate) const FLIP_ALLOW_PRIVATE_FEDERATION_ENDPOINTS_ENV: &str =
    "FLIP_ALLOW_PRIVATE_FEDERATION_ENDPOINTS";

const DEFAULT_DATA_DIR: &str = ".flip";
const DEFAULT_SQLITE_FILENAME: &str = "flip.sqlite";
const DEFAULT_SECRET_STORE_KEY_FILENAME: &str = "secret-store.key";

/// Data-directory lock file.
///
/// One definition, because two would be a silent hazard rather than a
/// duplication: `commit_live_restore` decides what to retain across a restore by
/// comparing file names against this, so a config that derived a different name
/// would have the live lock moved aside mid-restore with nothing to catch it.
pub(crate) const LOCK_FILE_NAME: &str = "flip.lock";
const DEFAULT_ADMIN_BIND_ADDRESS: &str = "127.0.0.1:8173";
const DEFAULT_PUBLIC_BIND_ADDRESS: &str = "127.0.0.1:8174";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    /// Run the FLIP daemon process.
    RunDaemon(DaemonArgs),
}

/// Runtime surface selected for one daemon process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonMode {
    /// Normal provider, admin, public API, and background-worker operation.
    Normal,
    /// Isolated health, backup-inspection, and backup-restore operation.
    Restore,
}

/// Boot-only daemon arguments.
#[derive(Clone, Eq, PartialEq)]
pub struct DaemonArgs {
    /// Canonical Manifold environment used to construct FLIP's trust inputs.
    pub manifold_environment: ManifoldEnvironment,

    /// FLIP application data directory.
    pub data_dir: PathBuf,

    /// SQLite database path. Defaults to `{data_dir}/flip.sqlite`.
    pub sqlite_path: PathBuf,

    /// Private Operator Admin API bind address.
    pub admin_bind_address: SocketAddr,

    /// App-facing Public Liquidity API bind address.
    pub public_bind_address: SocketAddr,

    /// Optional first-run bootstrap token for local/package administration.
    pub bootstrap_admin_token: Option<String>,

    /// Optional hex-encoded 32-byte secret-store key override.
    pub secret_store_key: Option<String>,

    /// Break-glass: accept the bootstrap token when the rotated one cannot be
    /// read at all.
    ///
    /// A rotated token normally supersedes the bootstrap token outright, and a
    /// failure to read it locks the Admin API rather than falling back — so an
    /// induced storage failure cannot resurrect a retired credential. That is
    /// the right default, and it means unreadable storage locks the operator
    /// out of the API they would use to diagnose it. Setting this re-opens the
    /// fallback, and requires a restart, so using it means proving control of
    /// the deployment rather than merely reaching the port.
    pub allow_bootstrap_token_fallback: bool,

    /// Runtime surface selected for this process.
    pub mode: DaemonMode,

    /// Optional hex-encoded Nostr/secp256k1 provider service secret key import.
    pub provider_nostr_secret_key: Option<String>,

    /// Optional test-deployment directory substituting the federation preview
    /// and FMan trust-material verification inputs with fixture files.
    /// Refused for Bitcoin mainnet.
    pub trust_fixtures_dir: Option<PathBuf>,

    /// Ceiling on target federation clients held open at once.
    ///
    /// Boot-only rather than provider policy: it sizes a host resource pool —
    /// RocksDB handles, file locks, and per-client background tasks — not what
    /// FLIP offers anyone. FI-supplied federation ids decide how many distinct
    /// targets ask for a client, so the ceiling must not come from them.
    pub max_open_target_clients: NonZeroUsize,

    /// Dial requester-supplied federation endpoints on loopback and
    /// deployment-private addresses.
    ///
    /// Off by default, and refused on mainnet. Local harnesses run their
    /// federation on loopback, so the address policy needs a way to say so;
    /// without one every test would disable the policy outright, and a guard
    /// every test disables protects nothing.
    pub allow_private_federation_endpoints: bool,
}

impl fmt::Debug for DaemonArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DaemonArgs")
            .field("manifold_environment", &self.manifold_environment)
            .field("data_dir", &self.data_dir)
            .field("sqlite_path", &self.sqlite_path)
            .field("admin_bind_address", &self.admin_bind_address)
            .field("public_bind_address", &self.public_bind_address)
            .field(
                "bootstrap_admin_token",
                &self.bootstrap_admin_token.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "secret_store_key",
                &self.secret_store_key.as_ref().map(|_| "<redacted>"),
            )
            .field("mode", &self.mode)
            .field(
                "provider_nostr_secret_key",
                &self
                    .provider_nostr_secret_key
                    .as_ref()
                    .map(|_| "<redacted>"),
            )
            .field("trust_fixtures_dir", &self.trust_fixtures_dir)
            .field("max_open_target_clients", &self.max_open_target_clients)
            .field(
                "allow_private_federation_endpoints",
                &self.allow_private_federation_endpoints,
            )
            .finish()
    }
}

impl DaemonArgs {
    /// Returns the derived on-disk layout for this daemon.
    #[must_use]
    pub fn paths(&self) -> DaemonPaths {
        DaemonPaths {
            data_dir: self.data_dir.clone(),
            sqlite_path: self.sqlite_path.clone(),
            secret_store_key: self.data_dir.join(DEFAULT_SECRET_STORE_KEY_FILENAME),
            federations_dir: self.data_dir.join("federations"),
            lock_file: self.data_dir.join(LOCK_FILE_NAME),
        }
    }
}

/// Derived boot-time filesystem layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonPaths {
    /// FLIP application data directory.
    pub data_dir: PathBuf,

    /// SQLite database file.
    pub sqlite_path: PathBuf,

    /// Generated local secret-store key file.
    pub secret_store_key: PathBuf,

    /// Target federation client storage directory.
    pub federations_dir: PathBuf,

    /// Process lock file.
    pub lock_file: PathBuf,
}

/// Parses CLI arguments from the current process.
///
/// Prints help/version output or an argument error and exits the process when
/// parsing does not produce a command.
#[must_use]
pub fn parse_cli() -> CliCommand {
    match parse_cli_from(env::args_os()) {
        Ok(command) => command,
        Err(error) => error.exit(),
    }
}

fn parse_cli_from<I, T>(args: I) -> Result<CliCommand, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Ok(Cli::try_parse_from(args)?.into_command())
}

/// Clap parser shape for the FLIP daemon command line.
#[derive(Parser)]
#[command(
    name = "liquidity-manager-daemon",
    version,
    about = "FLIP Federation Liquidity Provisioner daemon"
)]
struct Cli {
    #[command(subcommand)]
    command: CliSubcommand,
}

impl Cli {
    fn into_command(self) -> CliCommand {
        match self.command {
            CliSubcommand::Run {
                command: RunCommand::Daemon(args),
            } => CliCommand::RunDaemon(args.into_daemon_args()),
        }
    }
}

#[derive(Subcommand)]
enum CliSubcommand {
    /// Run a FLIP process.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
}

#[derive(Subcommand)]
enum RunCommand {
    /// Run the FLIP daemon process.
    Daemon(DaemonCliArgs),
}

/// Clap argument shape for `run daemon`. Flags override the matching `FLIP_*`
/// environment variables.
#[derive(Args)]
struct DaemonCliArgs {
    /// Canonical environment trust profile shared with FI and FMan.
    #[arg(
        long,
        env = FLIP_MANIFOLD_ENVIRONMENT_ENV
    )]
    manifold_environment: ManifoldEnvironment,

    /// FLIP application data directory.
    #[arg(long, env = FLIP_DATA_DIR_ENV, default_value = DEFAULT_DATA_DIR)]
    data_dir: PathBuf,

    /// SQLite database path. Defaults to `{data_dir}/flip.sqlite`.
    #[arg(long, env = FLIP_SQLITE_PATH_ENV)]
    sqlite_path: Option<PathBuf>,

    /// Private Operator Admin API bind address.
    #[arg(
        long,
        env = FLIP_ADMIN_BIND_ADDRESS_ENV,
        default_value = DEFAULT_ADMIN_BIND_ADDRESS,
        value_name = "HOST:PORT"
    )]
    admin_bind_address: SocketAddr,

    /// App-facing Public Liquidity API bind address.
    #[arg(
        long,
        env = FLIP_PUBLIC_BIND_ADDRESS_ENV,
        default_value = DEFAULT_PUBLIC_BIND_ADDRESS,
        value_name = "HOST:PORT"
    )]
    public_bind_address: SocketAddr,

    /// Optional first-run bootstrap token for local/package administration.
    #[arg(
        long,
        env = FLIP_BOOTSTRAP_ADMIN_TOKEN_ENV,
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    bootstrap_admin_token: Option<String>,

    /// Optional hex-encoded 32-byte secret-store key override.
    #[arg(
        long,
        env = FLIP_SECRET_KEY_ENV,
        hide_env_values = true,
        value_name = "HEX"
    )]
    secret_store_key: Option<String>,

    /// Run only the Admin restore API instead of the normal daemon.
    #[arg(
        long,
        env = FLIP_RESTORE_MODE_ENV,
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new()
    )]
    restore_mode: bool,

    /// Break-glass: accept the bootstrap token when the rotated one cannot be
    /// read. For recovering from unreadable secret storage; leave off normally.
    #[arg(
        long,
        env = FLIP_BOOTSTRAP_TOKEN_FALLBACK_ENV,
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new()
    )]
    allow_bootstrap_token_fallback: bool,

    /// Optional hex-encoded Nostr/secp256k1 provider service secret key import.
    #[arg(
        long,
        env = FLIP_PROVIDER_NOSTR_SECRET_KEY_ENV,
        hide_env_values = true,
        value_name = "HEX"
    )]
    provider_nostr_secret_key: Option<String>,

    /// Optional test-deployment directory substituting the federation preview
    /// and FMan trust-material verification inputs with fixture files.
    /// Refused for Bitcoin mainnet.
    #[arg(long = "trust-fixtures", env = FLIP_TRUST_FIXTURES_ENV, value_name = "DIR")]
    trust_fixtures_dir: Option<PathBuf>,

    /// Ceiling on target federation clients held open at once.
    #[arg(
        long,
        env = FLIP_MAX_OPEN_TARGET_CLIENTS_ENV,
        default_value_t = crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        value_name = "COUNT"
    )]
    max_open_target_clients: NonZeroUsize,

    /// Dial requester-supplied federation endpoints on loopback and
    /// deployment-private addresses. For local harnesses; refused on mainnet.
    #[arg(
        long,
        env = FLIP_ALLOW_PRIVATE_FEDERATION_ENDPOINTS_ENV,
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new()
    )]
    allow_private_federation_endpoints: bool,
}

impl DaemonCliArgs {
    /// Resolves derived defaults into boot-only daemon arguments.
    fn into_daemon_args(self) -> DaemonArgs {
        let sqlite_path = self
            .sqlite_path
            .unwrap_or_else(|| self.data_dir.join(DEFAULT_SQLITE_FILENAME));

        DaemonArgs {
            manifold_environment: self.manifold_environment,
            data_dir: self.data_dir,
            sqlite_path,
            admin_bind_address: self.admin_bind_address,
            public_bind_address: self.public_bind_address,
            bootstrap_admin_token: self.bootstrap_admin_token,
            secret_store_key: self.secret_store_key,
            allow_bootstrap_token_fallback: self.allow_bootstrap_token_fallback,
            mode: if self.restore_mode {
                DaemonMode::Restore
            } else {
                DaemonMode::Normal
            },
            provider_nostr_secret_key: self.provider_nostr_secret_key,
            trust_fixtures_dir: self.trust_fixtures_dir,
            max_open_target_clients: self.max_open_target_clients,
            allow_private_federation_endpoints: self.allow_private_federation_endpoints,
        }
    }
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;
