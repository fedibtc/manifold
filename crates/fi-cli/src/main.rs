use std::fs::OpenOptions;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use clap::{Args as ClapArgs, Parser, Subcommand};
use fedi_decentralized_domain::BitcoinNetwork;
use fedi_decentralized_manifold_environment::{ManifoldEnvironment, ManifoldEnvironmentProfile};
use fedi_decentralized_nostr_clients::{
    FiNostrClient, NostrClientError, NostrClientResult, NostrFiClient, NostrRelayClient,
    ROLE_FETCHED_EVENT_MAX_BYTES,
};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_fleet_manager::*;
use fedi_decentralized_service_liquidity_manager::{
    PUBLIC_LIQUIDITY_API_ALPN, PublicLiquidityApiClient,
};
use fedimint_core::Amount;
use fedimint_core::core::OperationId as FedimintOperationId;
use fedimint_core::encoding::Encodable as _;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fi_client::{
    FederationConsensusError, FederationConsensusReader, FederationConsensusSnapshot,
    FedimintFederationId, FedimintdVersionRange, FiClient, FiFeeAccountError, FiFeeAccountProvider,
    FiIdentity, FiPaymentError, FiPayments, FiStatus, FleetManagerCallError, FleetManagerConnector,
    FleetManagerConnectorError, FmanCandidateRequirements, FmanDiscoveryOptions, FmanRegistryQuery,
    FmanSelectionRequest, FormationActionRequired, FormationIntent, FormationPhase,
    FormationRunOptions, GuardianFeePpm, LiquidityOperationId, LiquidityProviderConnector,
    LiquidityProviderConnectorError, LiquidityRequestIntent, MaintenanceRunOptions,
    PaymentAuthorizationId, PaymentReservationRecovery, PlanPreference, PreparedSeatPayment,
    SeatPaymentRecovery, SeatPaymentRequirement, SettledSeatRefund,
};
use iroh::Endpoint;
use iroh::endpoint::presets;
use locked_payments::locked_payment;
use locked_payments::locked_payment_v2;
use locked_payments::refund::{PreparedRefund, PreparedRefundV2};
use nostr_sdk::{Event as NostrEvent, Keys as NostrKeys, PublicKey as NostrPublicKey};
use payer::{
    InsufficientLockedPaymentFundsWithoutReservation, LockedPaymentPreflight,
    LockedPaymentRecovery, LockedPaymentReservation, LockedPaymentTerminalRelease,
};
use rand::rngs::OsRng;
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use wallet::{Wallet, WalletSecret};
use zeroize::{Zeroize, Zeroizing};

#[cfg(unix)]
mod funding_token_journal;
#[cfg(not(unix))]
#[path = "funding_token_journal_nonunix.rs"]
mod funding_token_journal;
mod output;
mod payer;
mod wallet;

use funding_token_journal::FundingTokenJournal;
use output::CliOutput;
use output::OutputFormat;

const IDENTITY_FILE: &str = "fi-identity";
const DATABASE_DIR: &str = "fi-client.db";

type CliClient =
    FiClient<CliIdentity, CliPayments, CliRegistry, CliFmanConnector, CliConsensusReader>;

#[derive(Debug, Parser)]
#[command(
    name = "fi-cli",
    about = "Development/test-only Federation Initiator client. Unsupported for production use. \
             Use only test credentials/material and test funds."
)]
struct AppArgs {
    /// Disposable development/test FI identity and database directory.
    #[arg(long, default_value = ".fi")]
    state_dir: PathBuf,

    /// Render command output as JSON.
    #[arg(long)]
    json: bool,

    /// Complete signed kind-37707 event used for setup-payment policy.
    #[arg(long, global = true, requires = "setup_payment_publisher")]
    setup_payment_event_file: Option<PathBuf>,

    /// Deployment-pinned Fedi publisher key for setup-payment policy and stored fallback.
    #[arg(long, global = true)]
    setup_payment_publisher: Option<String>,

    /// Canonical Manifold environment used by this development/test client.
    #[arg(long, global = true, default_value = "development")]
    manifold_environment: ManifoldEnvironment,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize a persistent FI identity and database.
    Init,
    /// Validate intent and start formation when capabilities are connected.
    Create(Box<CreateArgs>),
    /// Resume the active durable formation.
    Resume(ResumeArgs),
    /// Explicitly authorize the exact parked aggregate payment requirements.
    AuthorizePayments(Box<AuthorizePaymentsArgs>),
    /// Print the latest durable/observed formation state.
    Status,
    /// Discover currently eligible FMan advertisements from the environment
    /// registry (read-only; no durable effects).
    Discover(RegistryQueryArgs),
    /// Preview the verified seat selection for an intent shape (read-only;
    /// no durable effects, no reservation).
    Preview(RegistryQueryArgs),
    /// Discover, request, and recover post-formation liquidity.
    Liquidity(LiquidityArgs),
    /// Maintain consensus metadata and configure guardian fees after formation.
    Maintenance(MaintenanceArgs),
    /// Join and directly fund the Fedimint wallet later supplied to formation.
    PaymentWallet(PaymentWalletArgs),
}

#[derive(Debug, ClapArgs)]
struct MaintenanceArgs {
    #[command(flatten)]
    timing: MaintenanceTimingArgs,
    #[command(subcommand)]
    command: MaintenanceCommand,
}

#[derive(Debug, ClapArgs)]
struct MaintenanceTimingArgs {
    /// Delay between consensus convergence attempts.
    #[arg(long, global = true, default_value_t = 2)]
    poll_interval_secs: u64,
    /// Maximum time for the maintenance operation after reconciliation.
    #[arg(long, global = true, default_value_t = 600)]
    run_timeout_secs: u64,
    /// Maximum time for one consensus read, connection, signature, or RPC.
    #[arg(long, global = true, default_value_t = 30)]
    request_timeout_secs: u64,
}

#[derive(Debug, Subcommand)]
enum MaintenanceCommand {
    /// Set the Guardianito-compatible federation display name.
    SetName(MetadataValueArgs),
    /// Set the Guardianito-compatible public HTTP(S) federation icon URL.
    SetIconUrl(MetadataValueArgs),
    /// Set the Guardianito-compatible welcome message/description.
    SetWelcomeMessage(MetadataValueArgs),
    /// Install Guardianito's fixed terms-of-service document.
    SetTermsOfService,
    /// Change the rate of the canonical fee policy installed at formation.
    ConfigureGuardianFees(ConfigureGuardianFeesArgs),
}

#[derive(Debug, ClapArgs)]
struct MetadataValueArgs {
    /// Exact metadata value; fi-client preserves the accepted original string.
    #[arg(long)]
    value: String,
}

#[derive(Debug, ClapArgs)]
struct ConfigureGuardianFeesArgs {
    /// Ongoing fee rate in ppm; fi-client enforces the admitted published minimum, up to 210000.
    #[arg(long, default_value_t = GuardianFeePpm::MANIFOLD_DEFAULT.value())]
    send_ppm: u32,
}

#[derive(Debug)]
enum MaintenancePreflight {
    Metadata {
        update: FederationMetadataUpdate,
        field: String,
        value: String,
        options: MaintenanceRunOptions,
    },
    GuardianFees {
        send_ppm: GuardianFeePpm,
        options: FormationRunOptions,
    },
}

#[derive(Debug)]
enum CliFiFeeAccountProvider {
    Unavailable,
    FromFile(fi_client::GuardianFeeAccount),
}

impl CliFiFeeAccountProvider {
    fn unavailable() -> Self {
        Self::Unavailable
    }

    fn from_file(path: Option<&Path>) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::Unavailable);
        };
        let bytes = std::fs::read(path)
            .with_context(|| format!("read FI SPv2 BtcDepositor account {}", path.display()))?;
        let account = serde_json::from_slice::<GuardianFeeAccount>(&bytes).with_context(|| {
            format!(
                "parse FI SPv2 account {} as a single-signature BtcDepositor account",
                path.display()
            )
        })?;
        Ok(Self::FromFile(account.into_account()))
    }
}

impl FiFeeAccountProvider for CliFiFeeAccountProvider {
    fn formed_federation_fee_account(
        &self,
        _federation_id: &FedimintFederationId,
    ) -> Result<fi_client::GuardianFeeAccount, FiFeeAccountError> {
        match self {
            Self::Unavailable => Err(FiFeeAccountError::new(
                "fi-cli FI fee-account file was not supplied",
            )),
            Self::FromFile(account) => Ok(account.clone()),
        }
    }
}

impl MaintenanceArgs {
    fn preflight(&self) -> anyhow::Result<MaintenancePreflight> {
        let maintenance_options = || {
            MaintenanceRunOptions::new(fi_client::MaintenanceRunOptionsConfig {
                poll_interval: Duration::from_secs(self.timing.poll_interval_secs),
                run_timeout: Duration::from_secs(self.timing.run_timeout_secs),
                request_timeout: Duration::from_secs(self.timing.request_timeout_secs),
            })
            .map_err(anyhow::Error::from)
        };
        let formation_options = || {
            FormationRunOptions::new(fi_client::FormationRunOptionsConfig {
                poll_interval: Duration::from_secs(self.timing.poll_interval_secs),
                run_timeout: Duration::from_secs(self.timing.run_timeout_secs),
                request_timeout: Duration::from_secs(self.timing.request_timeout_secs),
            })
            .map_err(anyhow::Error::from)
        };
        let metadata = |update: FederationMetadataUpdate| -> anyhow::Result<_> {
            let (field, value) = update.clone().into_field();
            Ok(MaintenancePreflight::Metadata {
                update,
                field: field.0,
                value: value.0,
                options: maintenance_options()?,
            })
        };

        match &self.command {
            MaintenanceCommand::SetName(args) => metadata(
                FederationMetadataUpdate::name(args.value.clone())
                    .context("validate federation metadata name")?,
            ),
            MaintenanceCommand::SetIconUrl(args) => metadata(
                FederationMetadataUpdate::icon_url(args.value.clone())
                    .context("validate federation metadata icon URL")?,
            ),
            MaintenanceCommand::SetWelcomeMessage(args) => metadata(
                FederationMetadataUpdate::welcome_message(args.value.clone())
                    .context("validate federation metadata welcome message")?,
            ),
            MaintenanceCommand::SetTermsOfService => {
                metadata(FederationMetadataUpdate::TermsOfService)
            }
            MaintenanceCommand::ConfigureGuardianFees(args) => {
                ensure!(
                    args.send_ppm <= fi_client::MAX_GUARDIAN_FEE_PPM,
                    "--send-ppm must not exceed {}",
                    fi_client::MAX_GUARDIAN_FEE_PPM
                );
                let send_ppm = GuardianFeePpm::try_from(args.send_ppm)
                    .context("validate --send-ppm as a guardian-fee rate")?;
                Ok(MaintenancePreflight::GuardianFees {
                    send_ppm,
                    options: formation_options()?,
                })
            }
        }
    }
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletArgs {
    /// Persistent FI payment-wallet directory shared with `create` and `resume`.
    #[arg(long)]
    wallet_data_dir: PathBuf,
    /// Secure wallet-secret file; falls back to FI_CLI_WALLET_SECRET_FILE.
    #[arg(long, value_name = "PATH")]
    wallet_secret_file: Option<PathBuf>,
    #[command(subcommand)]
    command: PaymentWalletCommand,
}

#[derive(Debug, Subcommand)]
enum PaymentWalletCommand {
    /// Join the setup-payment federation and persist its Fedimint client database.
    Join(PaymentWalletJoinArgs),
    /// Print the current spendable Bitcoin-unit balance.
    Balance(PaymentWalletFederationArgs),
    /// Audit accepted setup-payment and ecash-receive transaction accounting.
    Accounting(PaymentWalletFederationArgs),
    /// Generate an on-chain deposit address.
    DepositAddress(PaymentWalletDepositAddressArgs),
    /// Wait until the spendable balance reaches a minimum.
    WaitBalance(PaymentWalletWaitBalanceArgs),
    /// Generate a Lightning invoice that credits this wallet.
    Invoice(PaymentWalletInvoiceArgs),
    /// Await a previously generated Lightning invoice operation.
    AwaitInvoice(PaymentWalletAwaitInvoiceArgs),
    /// Deposit a guardian-fee remittance into an explicit BtcDepositor account.
    ///
    /// This development-only command exercises the payer wire operation; the
    /// production payer remains the Fedi app.
    RemitGuardianFee(PaymentWalletRemitGuardianFeeArgs),
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletJoinArgs {
    /// Invite for the setup-payment federation, including private test federations.
    #[arg(long)]
    payment_invite_code: String,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletFederationArgs {
    /// Setup-payment federation joined by an earlier `payment-wallet join` command.
    #[arg(long)]
    payment_federation_id: String,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletDepositAddressArgs {
    #[command(flatten)]
    federation: PaymentWalletFederationArgs,
    /// Maximum time to wait for wallet-v2 to derive an address.
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletWaitBalanceArgs {
    #[command(flatten)]
    federation: PaymentWalletFederationArgs,
    /// Minimum spendable balance in satoshis.
    #[arg(long)]
    minimum_sats: u64,
    /// Maximum time to wait for the balance to arrive.
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletInvoiceArgs {
    #[command(flatten)]
    federation: PaymentWalletFederationArgs,
    /// Invoice amount in satoshis.
    #[arg(long)]
    amount_sats: u64,
    /// Invoice expiry in seconds.
    #[arg(long, default_value_t = 3600)]
    expiry_secs: u32,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletAwaitInvoiceArgs {
    #[command(flatten)]
    federation: PaymentWalletFederationArgs,
    /// Operation ID printed by `payment-wallet invoice`.
    #[arg(long)]
    operation_id: String,
}

#[derive(Debug, ClapArgs)]
struct PaymentWalletRemitGuardianFeeArgs {
    #[command(flatten)]
    federation: PaymentWalletFederationArgs,
    /// Recipient BtcDepositor account id advertised by the guardian.
    #[arg(long)]
    account_id: stability_pool_common::AccountId,
    /// Amount to remit in millisatoshis.
    #[arg(long)]
    amount_msats: u64,
    /// File containing the sealed accounting metadata bytes.
    #[arg(long)]
    metadata_file: PathBuf,
}

#[derive(Debug)]
struct PaymentWalletPreflight {
    federation_id: FedimintFederationId,
    invite: Option<FedimintInviteCode>,
}

impl PaymentWalletArgs {
    fn preflight(&self) -> anyhow::Result<PaymentWalletPreflight> {
        let (federation_id, invite) = match &self.command {
            PaymentWalletCommand::Join(join) => {
                let invite = join
                    .payment_invite_code
                    .parse::<FedimintInviteCode>()
                    .context("parse --payment-invite-code")?;
                (invite.federation_id(), Some(invite))
            }
            PaymentWalletCommand::Balance(args) | PaymentWalletCommand::Accounting(args) => (
                parse_payment_federation_id(&args.payment_federation_id)?,
                None,
            ),
            PaymentWalletCommand::DepositAddress(args) => {
                ensure!(
                    args.timeout_secs > 0,
                    "--timeout-secs must be at least one second"
                );
                (
                    parse_payment_federation_id(&args.federation.payment_federation_id)?,
                    None,
                )
            }
            PaymentWalletCommand::WaitBalance(args) => {
                ensure!(
                    args.timeout_secs > 0,
                    "--timeout-secs must be at least one second"
                );
                let _ = sats_amount(args.minimum_sats)?;
                (
                    parse_payment_federation_id(&args.federation.payment_federation_id)?,
                    None,
                )
            }
            PaymentWalletCommand::Invoice(args) => {
                ensure!(
                    args.amount_sats > 0,
                    "--amount-sats must be at least one satoshi"
                );
                ensure!(
                    args.expiry_secs > 0,
                    "--expiry-secs must be at least one second"
                );
                let _ = sats_amount(args.amount_sats)?;
                (
                    parse_payment_federation_id(&args.federation.payment_federation_id)?,
                    None,
                )
            }
            PaymentWalletCommand::AwaitInvoice(args) => {
                let _ = args
                    .operation_id
                    .parse::<FedimintOperationId>()
                    .context("parse --operation-id")?;
                (
                    parse_payment_federation_id(&args.federation.payment_federation_id)?,
                    None,
                )
            }
            PaymentWalletCommand::RemitGuardianFee(args) => {
                ensure!(
                    args.amount_msats > 0,
                    "--amount-msats must be at least one millisatoshi"
                );
                (
                    parse_payment_federation_id(&args.federation.payment_federation_id)?,
                    None,
                )
            }
        };
        Ok(PaymentWalletPreflight {
            federation_id,
            invite,
        })
    }
}

fn parse_payment_federation_id(value: &str) -> anyhow::Result<FedimintFederationId> {
    value.parse().context("parse --payment-federation-id")
}

fn sats_amount(sats: u64) -> anyhow::Result<Amount> {
    let msats = sats
        .checked_mul(1_000)
        .context("satoshi amount exceeds the Fedimint amount domain")?;
    Ok(Amount::from_msats(msats))
}

#[derive(Debug, ClapArgs)]
struct LiquidityArgs {
    #[command(subcommand)]
    command: LiquidityCommand,
}

#[derive(Debug, Subcommand)]
enum LiquidityCommand {
    /// Discover currently admitted FLIP providers without disclosing an invite.
    Discover(LiquidityDiscoveryArgs),
    /// Select an admitted provider and start one exact liquidity request.
    Request(LiquidityRequestArgs),
    /// Resume one exact durable liquidity operation.
    Resume(LiquidityOperationArgs),
    /// Print one durable liquidity operation without network access.
    Status(LiquidityOperationArgs),
    /// List durable liquidity operations in stable operation-id order.
    List(LiquidityListArgs),
}

#[derive(Debug, ClapArgs)]
struct LiquidityIntentArgs {
    /// Federation Bitcoin network (bitcoin, testnet, signet, or regtest).
    #[arg(long)]
    network: BitcoinNetwork,
    /// Minimum gateway liquidity requested, in satoshis.
    #[arg(long)]
    gateway_min_sats: u64,
    /// Optional maximum gateway liquidity, in satoshis.
    #[arg(long)]
    gateway_max_sats: Option<u64>,
}

impl LiquidityIntentArgs {
    fn intent(&self) -> LiquidityRequestIntent {
        LiquidityRequestIntent::gateway(self.gateway_min_sats, self.gateway_max_sats)
    }
}

#[derive(Debug, ClapArgs)]
struct LiquidityDiscoveryArgs {
    #[command(flatten)]
    intent: LiquidityIntentArgs,
}

#[derive(Debug, ClapArgs)]
struct LiquidityRequestArgs {
    #[command(flatten)]
    intent: LiquidityIntentArgs,
    /// Exact provider public key; defaults to the first admitted provider.
    #[arg(long)]
    provider_pubkey: Option<String>,
}

#[derive(Debug, ClapArgs)]
struct LiquidityOperationArgs {
    /// Exact durable liquidity operation id.
    #[arg(long)]
    operation_id: String,
}

#[derive(Debug, ClapArgs)]
struct LiquidityListArgs {
    /// Exclusive cursor returned by a previous list result.
    #[arg(long)]
    after: Option<String>,
    /// Maximum operations to return (1..=100).
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

/// Shared arguments of the read-only registry queries (discover, preview).
#[derive(Debug, ClapArgs)]
struct RegistryQueryArgs {
    /// Federation size the eligibility filter and selection target.
    #[arg(long)]
    federation_size: u16,
    /// Inclusive lower three-number Fedimint release bound.
    #[arg(long, requires = "fedimintd_version_maximum_exclusive")]
    fedimintd_version_minimum: Option<String>,
    /// Exclusive upper three-number Fedimint release bound.
    #[arg(long, requires = "fedimintd_version_minimum")]
    fedimintd_version_maximum_exclusive: Option<String>,
    /// Absolute run deadline in seconds, at least 1 (enumeration plus
    /// verification).
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

impl RegistryQueryArgs {
    /// Convert the timeout flag into clamped library discovery options.
    ///
    /// A zero timeout is rejected at the argument boundary — like the
    /// pinned-driver timing flags — rather than being clamped up to the
    /// one-millisecond runtime quantum, which the user certainly did not
    /// mean.
    fn discovery_options(&self) -> anyhow::Result<FmanDiscoveryOptions> {
        ensure!(
            self.timeout_secs > 0,
            "--timeout-secs must be at least one second"
        );
        Ok(FmanDiscoveryOptions::with_timeout(Duration::from_secs(
            self.timeout_secs,
        )))
    }
}

#[derive(Debug, ClapArgs)]
struct ResumeArgs {
    /// JSON file containing this federation's FI SPv2 BtcDepositor account.
    #[arg(long, value_name = "PATH")]
    fi_spv2_account_file: Option<PathBuf>,
    /// Payment federation selected by an interrupted paid formation.
    #[arg(long)]
    payment_federation_id: Option<String>,
    /// Existing FI payment-wallet directory.
    #[arg(long)]
    wallet_data_dir: Option<PathBuf>,
    /// Secure wallet-secret file; falls back to FI_CLI_WALLET_SECRET_FILE.
    #[arg(long, value_name = "PATH")]
    wallet_secret_file: Option<PathBuf>,
    /// Optional invite used to re-join (reopen) the payment federation
    /// client so interrupted paid work can recover its wallet-side
    /// payment operations.
    #[arg(long)]
    payment_invite_code: Option<String>,
    /// Owner-owned regular mode-0600 file containing an OOB funding token.
    ///
    /// The file is limited to 256 KiB, atomically moved to a restart journal
    /// before import, and deleted only after the wallet confirms receipt.
    #[arg(long)]
    funding_token_file: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
struct AuthorizePaymentsArgs {
    /// Exact authorization binding printed by the parked formation.
    #[arg(long)]
    authorization_id: String,
    #[command(flatten)]
    resume: ResumeArgs,
}

#[derive(Debug, ClapArgs)]
struct CreateArgs {
    /// JSON file containing the FI's SPv2 BtcDepositor account for the new federation.
    #[arg(long, value_name = "PATH")]
    fi_spv2_account_file: PathBuf,
    /// Pinned Fleet Manager locator JSON; repeat once per guardian.
    #[arg(long = "locator")]
    locators: Vec<String>,
    /// INSECURE: discover fresh compatible ads but use them as unverified pinned locators.
    #[arg(long)]
    insecure_skip_fman_trust: bool,
    /// TOML file containing formation intent.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Federation display name override.
    #[arg(long)]
    federation_name: Option<String>,
    /// Guardian count override.
    #[arg(long)]
    federation_size: Option<u16>,
    /// Aggregate spending cap in millisatoshis for paid seats.
    ///
    /// `fi-client` self-authorizes only the initial aggregate when it is
    /// within the cap. Any later replacement always requires the explicit
    /// `authorize-payments` command.
    #[arg(long)]
    max_total_msats: Option<u64>,
    /// Inclusive lower three-number Fedimint release bound.
    #[arg(long, requires = "fedimintd_version_maximum_exclusive")]
    fedimintd_version_minimum: Option<String>,
    /// Exclusive upper three-number Fedimint release bound.
    #[arg(long, requires = "fedimintd_version_minimum")]
    fedimintd_version_maximum_exclusive: Option<String>,
    /// Pinned-driver probe interval, 1..=2,147,483 seconds; ignored without locators.
    #[arg(long, default_value_t = 2)]
    poll_interval_secs: u64,
    /// Per-invocation pinned-driver deadline, 1..=2,147,483 seconds; ignored without locators.
    #[arg(long, default_value_t = 600)]
    poll_timeout_secs: u64,
    /// Secure 0600 file containing the push-gateway hook URL bearer.
    #[arg(long, requires = "completion_callback_idempotency_key")]
    completion_callback_url_file: Option<PathBuf>,
    /// Stable push-gateway deduplication key paired with the callback URL.
    #[arg(long, requires = "completion_callback_url_file")]
    completion_callback_idempotency_key: Option<String>,
    /// Payment federation for a paid plan.
    #[arg(long)]
    payment_federation_id: Option<String>,
    /// Existing FI payment-wallet directory.
    #[arg(long)]
    wallet_data_dir: Option<PathBuf>,
    /// Secure wallet-secret file; falls back to FI_CLI_WALLET_SECRET_FILE.
    #[arg(long, value_name = "PATH")]
    wallet_secret_file: Option<PathBuf>,
    /// Optional invite used to join the selected payment federation.
    #[arg(long)]
    payment_invite_code: Option<String>,
    /// Owner-owned regular mode-0600 file containing an OOB funding token.
    ///
    /// The file is limited to 256 KiB, atomically moved to a restart journal
    /// before import, and deleted only after the wallet confirms receipt.
    #[arg(long)]
    funding_token_file: Option<PathBuf>,
}

struct CreatePreflight {
    intent: FormationIntent,
    options: FormationRunOptions,
    fi_fee_account_provider: CliFiFeeAccountProvider,
    completion_callback: Option<DkgCompletionCallback>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentFile {
    federation_name: Option<FederationName>,
    federation_size: Option<FederationSize>,
    plan: Option<PlanPreference>,
    fedimintd_version_minimum: Option<FedimintdVersion>,
    fedimintd_version_maximum_exclusive: Option<FedimintdVersion>,
    max_total_msats: Option<u64>,
}

#[derive(Clone)]
struct CliSetupPayment {
    event: Option<NostrEvent>,
    publisher: Option<NostrPublicKey>,
}

impl CliSetupPayment {
    fn load(args: &AppArgs) -> anyhow::Result<Self> {
        let Some(publisher) = &args.setup_payment_publisher else {
            return Ok(Self {
                event: None,
                publisher: None,
            });
        };
        let publisher = NostrPublicKey::parse(publisher)
            .context("parse --setup-payment-publisher as a Nostr public key")?;
        let Some(event_file) = &args.setup_payment_event_file else {
            return Ok(Self {
                event: None,
                publisher: Some(publisher),
            });
        };
        let mut event_bytes = Vec::new();
        std::fs::File::open(event_file)
            .with_context(|| format!("open setup-payment event {}", event_file.display()))?
            .take(
                u64::try_from(ROLE_FETCHED_EVENT_MAX_BYTES).expect("role event bound fits u64") + 1,
            )
            .read_to_end(&mut event_bytes)
            .with_context(|| format!("read setup-payment event {}", event_file.display()))?;
        ensure!(
            event_bytes.len() <= ROLE_FETCHED_EVENT_MAX_BYTES,
            "setup-payment event file exceeds transport bound"
        );
        let event = serde_json::from_slice::<NostrEvent>(&event_bytes)
            .with_context(|| format!("parse setup-payment event {}", event_file.display()))?;
        Ok(Self {
            event: Some(event),
            publisher: Some(publisher),
        })
    }

    fn registry(&self, live: Option<NostrFiClient>) -> CliRegistry {
        CliRegistry {
            live,
            setup_payment_event: self.event.clone(),
        }
    }
}

#[derive(Clone)]
struct CliRegistry {
    live: Option<NostrFiClient>,
    setup_payment_event: Option<NostrEvent>,
}

impl FiNostrClient for CliRegistry {
    async fn fetch_fman_advertisement(
        &self,
        fman_pubkey: NostrPublicKey,
        timeout: Duration,
    ) -> NostrClientResult<NostrEvent> {
        self.live
            .as_ref()
            .ok_or(NostrClientError::MissingEvent {
                context: "fi-cli live registry capability unavailable",
            })?
            .fetch_fman_advertisement(fman_pubkey, timeout)
            .await
    }

    async fn fetch_setup_payment_federations(
        &self,
        publisher: NostrPublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Vec<NostrEvent>> {
        if let Some(event) = &self.setup_payment_event {
            return Ok(vec![event.clone()]);
        }
        self.live
            .as_ref()
            .ok_or(NostrClientError::MissingEvent {
                context: "fi-cli live setup-payment registry capability unavailable",
            })?
            .fetch_setup_payment_federations(publisher, timeout)
            .await
    }

    async fn fetch_fman_advertisements(
        &self,
        timeout: Duration,
    ) -> NostrClientResult<Vec<NostrEvent>> {
        self.live
            .as_ref()
            .ok_or(NostrClientError::MissingEvent {
                context: "fi-cli live registry capability unavailable",
            })?
            .fetch_fman_advertisements(timeout)
            .await
    }

    async fn fetch_liquidity_provider_advertisements(
        &self,
        timeout: Duration,
    ) -> NostrClientResult<Vec<NostrEvent>> {
        self.live
            .as_ref()
            .ok_or(NostrClientError::MissingEvent {
                context: "fi-cli live liquidity registry capability unavailable",
            })?
            .fetch_liquidity_provider_advertisements(timeout)
            .await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = AppArgs::parse();
    execute(args).await
}

async fn execute(args: AppArgs) -> anyhow::Result<()> {
    let create_preflight = match &args.command {
        Command::Create(create) => {
            let intent = load_intent(create)?;
            let completion_callback = create
                .completion_callback_url_file
                .as_ref()
                .zip(create.completion_callback_idempotency_key.as_ref())
                .map(|(callback_url_file, idempotency_key)| {
                    DkgCompletionCallback::new(DkgCompletionCallbackInput {
                        callback_url: read_completion_callback_url(callback_url_file)?,
                        idempotency_key: idempotency_key.clone(),
                    })
                    .map_err(anyhow::Error::from)
                })
                .transpose()?;
            ensure!(
                !create.locators.is_empty()
                    || create.insecure_skip_fman_trust
                    || completion_callback.is_none(),
                "completion callbacks currently require pinned --locator formation"
            );
            let options = if create.locators.is_empty() && !create.insecure_skip_fman_trust {
                FormationRunOptions::default()
            } else {
                FormationRunOptions::new(fi_client::FormationRunOptionsConfig {
                    poll_interval: Duration::from_secs(create.poll_interval_secs),
                    run_timeout: Duration::from_secs(create.poll_timeout_secs),
                    ..Default::default()
                })?
            };
            let fi_fee_account_provider =
                CliFiFeeAccountProvider::from_file(Some(&create.fi_spv2_account_file))?;
            Some(CreatePreflight {
                intent,
                options,
                fi_fee_account_provider,
                completion_callback,
            })
        }
        _ => None,
    };
    let payment_wallet_preflight = match &args.command {
        Command::PaymentWallet(payment_wallet) => Some(payment_wallet.preflight()?),
        _ => None,
    };
    let maintenance_preflight = match &args.command {
        Command::Maintenance(maintenance) => Some(maintenance.preflight()?),
        _ => None,
    };
    let wallet_secret = WalletRootSecret::read_for(&args)?;
    run(
        args,
        wallet_secret,
        create_preflight,
        payment_wallet_preflight,
        maintenance_preflight,
    )
    .await
}

const MAX_CALLBACK_URL_FILE_BYTES: u64 = 2_050;

#[cfg(unix)]
fn read_completion_callback_url(path: &Path) -> anyhow::Result<String> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options
        .open(path)
        .map_err(|_| anyhow::anyhow!("could not open completion callback URL file"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow::anyhow!("could not inspect completion callback URL file"))?;
    ensure!(
        metadata.file_type().is_file(),
        "completion callback URL file is not a regular file"
    );
    // SAFETY: `geteuid` has no preconditions and does not dereference memory.
    let current_uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.uid() == current_uid,
        "completion callback URL file is not owned by the current user"
    );
    ensure!(
        metadata.mode() & 0o7777 == 0o600,
        "completion callback URL file permissions must be exactly 0600"
    );
    ensure!(
        metadata.len() <= MAX_CALLBACK_URL_FILE_BYTES,
        "completion callback URL file is too long"
    );

    let mut bytes = SecretBuffer::new(Vec::new());
    file.take(MAX_CALLBACK_URL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("could not read completion callback URL file"))?;
    ensure!(
        bytes.len() as u64 <= MAX_CALLBACK_URL_FILE_BYTES,
        "completion callback URL file is too long"
    );
    let callback_url = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("completion callback URL file is not valid UTF-8"))?;
    let callback_url = callback_url
        .strip_suffix("\r\n")
        .or_else(|| callback_url.strip_suffix('\n'))
        .unwrap_or(callback_url);
    Ok(callback_url.to_owned())
}

#[cfg(not(unix))]
fn read_completion_callback_url(_path: &Path) -> anyhow::Result<String> {
    anyhow::bail!("secure completion callback URL files are unsupported on this platform")
}

async fn run(
    args: AppArgs,
    wallet_secret: Option<WalletRootSecret>,
    create_preflight: Option<CreatePreflight>,
    payment_wallet_preflight: Option<PaymentWalletPreflight>,
    maintenance_preflight: Option<MaintenancePreflight>,
) -> anyhow::Result<()> {
    let mut output = CliOutput::stdio();
    let format = OutputFormat::from_json_flag(args.json);
    let setup_payment = CliSetupPayment::load(&args)?;
    let profile = args
        .manifold_environment
        .profile()
        .context("resolve Manifold environment profile")?;
    let peer_badge_verifier = command_requires_peer_badge_verifier(&args.command)
        .then(|| PeerBadgeVerifier::try_from_profile(&profile))
        .transpose()
        .context("construct PeerBadge verifier for Manifold environment")?;
    match args.command {
        Command::Init => {
            let identity = CliIdentity::load_or_create(&args.state_dir, true)?;
            let client = open_client(
                &args.state_dir,
                identity,
                CliPayments::unavailable(),
                &setup_payment,
                None,
                CliFmanConnector::unavailable(),
                peer_badge_verifier
                    .clone()
                    .expect("commands other than discover construct a PeerBadge verifier"),
                profile.clone(),
            )
            .await?;
            let fi_pubkey = identity.public_key().map_err(anyhow::Error::msg)?;
            if args.json {
                output.init(fi_pubkey, &client.status())?;
            } else {
                println!("initialized FI {}", hex::encode(fi_pubkey.0.serialize()));
            }
        }
        Command::Create(create) => {
            let CreatePreflight {
                intent,
                options,
                fi_fee_account_provider,
                completion_callback,
            } = create_preflight.expect("create input is parsed before external input access");
            let mut locators = create
                .locators
                .iter()
                .map(|input| serde_json::from_str(input).context("parse pinned FMan locator"))
                .collect::<anyhow::Result<Vec<_>>>()?;
            ensure!(
                !create.insecure_skip_fman_trust || locators.is_empty(),
                "--insecure-skip-fman-trust cannot be combined with --locator"
            );
            ensure!(
                !create.insecure_skip_fman_trust
                    || args.manifold_environment != ManifoldEnvironment::Production,
                "--insecure-skip-fman-trust is forbidden in production"
            );
            let selected_mode = locators.is_empty() && !create.insecure_skip_fman_trust;
            let payment_preflight = CliPayments::preflight(&create, wallet_secret)?;
            let payment_federation_id = payment_preflight
                .as_ref()
                .map(|payment| fi_client::FederationId(payment.selected.to_string()));
            if selected_mode && payment_federation_id.is_some() {
                ensure!(
                    intent.max_total_msats().is_some(),
                    "registry-backed paid creation requires --max-total-msats"
                );
            }
            let diagnostic_registry = if create.insecure_skip_fman_trust {
                let registry = connect_environment_registry(args.manifold_environment).await?;
                let requirements = FmanCandidateRequirements {
                    federation_size: intent.federation_size(),
                    fedimintd_versions: intent.fedimintd_versions().clone(),
                };
                let discovery = FmanRegistryQuery::new(registry.clone())
                    .insecure_discover_untrusted_pinned_fmans(
                        &requirements,
                        FmanDiscoveryOptions::default(),
                    )
                    .await?;
                let required = usize::from(intent.federation_size().0);
                ensure!(
                    discovery.candidates.len() >= required,
                    "insecure diagnostic discovery found {} compatible FMan(s), but {} are required",
                    discovery.candidates.len(),
                    required
                );
                locators = discovery
                    .candidates
                    .into_iter()
                    .take(required)
                    .map(|candidate| candidate.locator)
                    .collect();
                if format == OutputFormat::Human {
                    eprintln!(
                        "WARNING: bypassing FMan HolderAuthorization/PeerBadge trust for {} pinned diagnostic seat(s)",
                        locators.len()
                    );
                }
                Some(registry)
            } else {
                None
            };
            if !selected_mode {
                CliClient::preflight_create_with_pinned_fmans(&intent, &locators)?;
            }
            // The endpoint carries no identity or durable state; binding it
            // before the preview lets selection probe live availability.
            let endpoint = bind_iroh_endpoint().await?;
            // Preview before opening identity, durable FI state, or the payment wallet. The
            // sealed approval stays in this process and is consumed below.
            let selected = if selected_mode {
                let registry = connect_environment_registry(args.manifold_environment).await?;
                let request = FmanSelectionRequest::new(
                    intent.federation_size(),
                    intent.fedimintd_versions().clone(),
                    intent.plan(),
                )?;
                let preview = FmanRegistryQuery::new(registry.clone())
                    .with_verifier(
                        peer_badge_verifier
                            .clone()
                            .expect("selected creation constructs a PeerBadge verifier"),
                    )
                    .with_fman_connector(CliFmanConnector::new(endpoint.clone()))
                    .preview_fman_selection(&request, FmanDiscoveryOptions::default())
                    .await?;
                let approval = preview.approve(intent.max_total_msats().unwrap_or(0))?;
                Some((registry, approval))
            } else {
                None
            };
            let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
            let payments = CliPayments::open(payment_preflight, format, &mut output).await?;
            let live_registry = if let Some((registry, _)) = &selected {
                Some(registry.clone())
            } else if let Some(registry) = diagnostic_registry {
                Some(registry)
            } else if setup_payment.event.is_none() && payment_federation_id.is_some() {
                Some(connect_environment_registry(args.manifold_environment).await?)
            } else {
                None
            };
            let client = open_client_with_fee_account_provider(
                &args.state_dir,
                identity,
                payments,
                &setup_payment,
                live_registry,
                CliFmanConnector::new(endpoint.clone()),
                peer_badge_verifier
                    .clone()
                    .expect("commands other than discover construct a PeerBadge verifier"),
                profile.clone(),
                fi_fee_account_provider,
            )
            .await?;
            let mut observer = client.observe();
            let result = if selected_mode {
                let (_, approval) = selected.expect("selected creation retains its approval");
                match payment_federation_id {
                    Some(payer) => client
                        .pay_and_create(intent.clone(), approval, payer, options)
                        .await
                        .map_err(anyhow::Error::from),
                    None => client
                        .create_without_payer(intent.clone(), approval, options)
                        .await
                        .map_err(anyhow::Error::from),
                }
            } else {
                let mut formed = match completion_callback {
                    Some(callback) => {
                        client
                            .create_with_pinned_fmans_and_callback(
                                intent.clone(),
                                locators,
                                callback,
                                options,
                            )
                            .await
                    }
                    None => {
                        client
                            .create_with_pinned_fmans(intent.clone(), locators, options)
                            .await
                    }
                }
                .map_err(anyhow::Error::from);
                if formed.is_ok() {
                    formed =
                        authorize_pending_payments(&client, options, format, &mut output).await;
                }
                formed
            };
            if observer.has_changed().unwrap_or(false) {
                observer.borrow_and_update();
            }
            output.snapshot(&observer.borrow(), format)?;
            endpoint.close().await;
            result?;
        }
        Command::Resume(resume) => {
            let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
            let endpoint = if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
                Endpoint::bind(presets::N0DisableRelay).await?
            } else {
                Endpoint::bind(presets::N0).await?
            };
            let live_registry = connect_environment_registry(args.manifold_environment).await?;
            let client = open_client_with_fee_account_provider(
                &args.state_dir,
                identity,
                CliPayments::open_for_resume(&resume, wallet_secret).await?,
                &setup_payment,
                Some(live_registry),
                CliFmanConnector::new(endpoint.clone()),
                peer_badge_verifier
                    .clone()
                    .expect("commands other than discover construct a PeerBadge verifier"),
                profile.clone(),
                CliFiFeeAccountProvider::from_file(resume.fi_spv2_account_file.as_deref())?,
            )
            .await?;
            let result = client.resume().await;
            output.snapshot(&client.status(), format)?;
            endpoint.close().await;
            result?;
        }
        Command::AuthorizePayments(authorize) => {
            let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
            let endpoint = if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
                Endpoint::bind(presets::N0DisableRelay).await?
            } else {
                Endpoint::bind(presets::N0).await?
            };
            let live_registry = connect_environment_registry(args.manifold_environment).await?;
            let client = open_client_with_fee_account_provider(
                &args.state_dir,
                identity,
                CliPayments::open_for_resume(&authorize.resume, wallet_secret).await?,
                &setup_payment,
                Some(live_registry),
                CliFmanConnector::new(endpoint.clone()),
                peer_badge_verifier
                    .clone()
                    .expect("commands other than discover construct a PeerBadge verifier"),
                profile.clone(),
                CliFiFeeAccountProvider::from_file(
                    authorize.resume.fi_spv2_account_file.as_deref(),
                )?,
            )
            .await?;
            let result = client
                .authorize_payments(
                    PaymentAuthorizationId::try_from_opaque(authorize.authorization_id)
                        .map_err(anyhow::Error::msg)?,
                    FormationRunOptions::default(),
                )
                .await;
            output.snapshot(&client.status(), format)?;
            endpoint.close().await;
            result?;
        }
        Command::Status => {
            let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
            let client = open_client(
                &args.state_dir,
                identity,
                CliPayments::unavailable(),
                &setup_payment,
                None,
                CliFmanConnector::unavailable(),
                peer_badge_verifier
                    .expect("commands other than discover construct a PeerBadge verifier"),
                profile.clone(),
            )
            .await?;
            output.snapshot(&client.status(), format)?;
        }
        Command::Discover(discover) => {
            let requirements = FmanCandidateRequirements {
                federation_size: FederationSize(discover.federation_size),
                fedimintd_versions: parse_fedimintd_range(
                    discover.fedimintd_version_minimum.as_deref(),
                    discover.fedimintd_version_maximum_exclusive.as_deref(),
                )?,
            };
            let options = discover.discovery_options()?;
            let registry = connect_environment_registry(args.manifold_environment).await?;
            let query = FmanRegistryQuery::new(registry);
            let discovery = query
                .discover_fman_candidates(&requirements, options)
                .await?;
            output.discovery(&discovery, format)?;
        }
        Command::Preview(preview) => {
            let request = FmanSelectionRequest::new(
                FederationSize(preview.federation_size),
                parse_fedimintd_range(
                    preview.fedimintd_version_minimum.as_deref(),
                    preview.fedimintd_version_maximum_exclusive.as_deref(),
                )?,
                PlanPreference::InfiniteBestEffort,
            )?;
            let options = preview.discovery_options()?;
            let registry = connect_environment_registry(args.manifold_environment).await?;
            let endpoint = bind_iroh_endpoint().await?;
            let query = FmanRegistryQuery::new(registry)
                .with_verifier(
                    peer_badge_verifier
                        .expect("commands other than discover construct a PeerBadge verifier"),
                )
                .with_fman_connector(CliFmanConnector::new(endpoint));
            let selection = query.preview_fman_selection(&request, options).await?;
            output.selection_preview(&selection, format)?;
        }
        Command::Maintenance(_) => {
            let operation = maintenance_preflight
                .expect("maintenance input is validated before FI state or network access");
            let endpoint = bind_iroh_endpoint().await?;
            let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
            let client = open_client_with_fee_account_provider(
                &args.state_dir,
                identity,
                CliPayments::unavailable(),
                &setup_payment,
                None,
                CliFmanConnector::new(endpoint.clone()),
                peer_badge_verifier
                    .clone()
                    .expect("maintenance constructs a PeerBadge verifier"),
                profile.clone(),
                CliFiFeeAccountProvider::unavailable(),
            )
            .await?;
            let result = match operation {
                MaintenancePreflight::Metadata {
                    update,
                    field,
                    value,
                    options,
                } => reconcile_then_run_post_formation(
                    client.resume(),
                    client.update_federation_metadata(update, options),
                )
                .await
                .context("reconcile and update federation metadata")
                .and_then(|()| output.metadata_consensus(&field, &value, format)),
                MaintenancePreflight::GuardianFees { send_ppm, options } => {
                    reconcile_then_run_post_formation(
                        client.resume(),
                        client.propose_guardian_fees(send_ppm, options),
                    )
                    .await
                    .context("reconcile and configure guardian fees")
                    .and_then(|()| output.guardian_fee_consensus(send_ppm.value(), format))
                }
            };
            endpoint.close().await;
            result?;
        }
        Command::PaymentWallet(payment_wallet) => {
            let PaymentWalletPreflight {
                federation_id,
                invite,
            } = payment_wallet_preflight
                .expect("payment-wallet input is parsed before wallet access");
            let wallet_secret = wallet_secret.context(
                "payment-wallet requires --wallet-secret-file or FI_CLI_WALLET_SECRET_FILE",
            )?;
            let wallet = open_wallet(payment_wallet.wallet_data_dir, wallet_secret).await?;
            if let Some(invite) = invite {
                wallet.join(&invite).await?;
            } else {
                wallet.open_federation(federation_id).await?;
            }
            match payment_wallet.command {
                PaymentWalletCommand::Join(_) => {
                    let balance = wallet.balance(federation_id).await?;
                    output.payment_wallet_joined(federation_id, balance, format)?;
                }
                PaymentWalletCommand::Balance(_) => {
                    let balance = wallet.balance(federation_id).await?;
                    output.payment_wallet_balance(federation_id, balance, format)?;
                }
                PaymentWalletCommand::Accounting(_) => {
                    let balance = wallet.balance(federation_id).await?;
                    let accounting = wallet.payment_accounting(federation_id).await?;
                    output.payment_wallet_accounting(federation_id, balance, accounting, format)?;
                }
                PaymentWalletCommand::DepositAddress(deposit_address) => {
                    let address = wallet
                        .deposit_address(
                            federation_id,
                            Duration::from_secs(deposit_address.timeout_secs),
                        )
                        .await?;
                    output.payment_wallet_deposit_address(federation_id, &address, format)?;
                }
                PaymentWalletCommand::WaitBalance(wait) => {
                    let minimum = sats_amount(wait.minimum_sats)?;
                    let balance = wallet
                        .wait_for_balance(
                            federation_id,
                            minimum,
                            Duration::from_secs(wait.timeout_secs),
                        )
                        .await?;
                    output.payment_wallet_balance_reached(
                        federation_id,
                        balance,
                        minimum,
                        format,
                    )?;
                }
                PaymentWalletCommand::Invoice(invoice) => {
                    let amount = sats_amount(invoice.amount_sats)?;
                    let (invoice, operation_id) = wallet
                        .create_invoice(federation_id, amount, invoice.expiry_secs)
                        .await?;
                    output.payment_wallet_invoice(
                        federation_id,
                        &invoice,
                        operation_id,
                        amount,
                        format,
                    )?;
                }
                PaymentWalletCommand::AwaitInvoice(await_invoice) => {
                    let operation_id = await_invoice
                        .operation_id
                        .parse::<FedimintOperationId>()
                        .context("parse --operation-id")?;
                    let state = wallet.await_invoice(federation_id, operation_id).await?;
                    let balance = wallet.balance(federation_id).await?;
                    output.payment_wallet_invoice_settled(
                        federation_id,
                        operation_id,
                        state,
                        balance,
                        format,
                    )?;
                }
                PaymentWalletCommand::RemitGuardianFee(remittance) => {
                    let metadata = tokio::fs::read(&remittance.metadata_file)
                        .await
                        .with_context(|| {
                            format!(
                                "read sealed guardian-fee metadata {}",
                                remittance.metadata_file.display()
                            )
                        })?;
                    let operation_id = wallet
                        .deposit_to_btc_balance(
                            federation_id,
                            remittance.account_id,
                            Amount::from_msats(remittance.amount_msats),
                            metadata,
                        )
                        .await?;
                    output.payment_wallet_guardian_fee_remitted(
                        federation_id,
                        operation_id,
                        Amount::from_msats(remittance.amount_msats),
                        format,
                    )?;
                }
            }
        }
        Command::Liquidity(liquidity) => match liquidity.command {
            LiquidityCommand::Discover(discover) => {
                let endpoint = bind_iroh_endpoint().await?;
                let registry = connect_environment_registry(args.manifold_environment).await?;
                let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
                let client = open_client(
                    &args.state_dir,
                    identity,
                    CliPayments::unavailable(),
                    &setup_payment,
                    Some(registry),
                    CliFmanConnector::new(endpoint.clone()),
                    peer_badge_verifier
                        .clone()
                        .expect("liquidity discovery constructs a PeerBadge verifier"),
                    profile.clone(),
                )
                .await?;
                let discovery = client
                    .discover_liquidity_providers(
                        &discover.intent.intent(),
                        discover.intent.network,
                    )
                    .await?;
                output.liquidity_discovery(&discovery, format)?;
                endpoint.close().await;
            }
            LiquidityCommand::Request(request) => {
                let endpoint = bind_iroh_endpoint().await?;
                let registry = connect_environment_registry(args.manifold_environment).await?;
                let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
                let client = open_client(
                    &args.state_dir,
                    identity,
                    CliPayments::unavailable(),
                    &setup_payment,
                    Some(registry),
                    CliFmanConnector::new(endpoint.clone()),
                    peer_badge_verifier
                        .clone()
                        .expect("liquidity request constructs a PeerBadge verifier"),
                    profile.clone(),
                )
                .await?;
                let intent = request.intent.intent();
                let discovery = client
                    .discover_liquidity_providers(&intent, request.intent.network)
                    .await?;
                let provider = match request.provider_pubkey.as_deref() {
                    Some(requested) => discovery
                        .providers
                        .iter()
                        .find(|provider| provider.provider_pubkey().0 == requested)
                        .with_context(|| {
                            format!("requested liquidity provider {requested} was not admitted")
                        })?,
                    None => discovery
                        .providers
                        .first()
                        .context("no admitted liquidity provider is available")?,
                }
                .provider_pubkey()
                .clone();
                let formation_id = match client.status() {
                    FiStatus::Formation(snapshot) if snapshot.phase == FormationPhase::Formed => {
                        snapshot.formation_id
                    }
                    _ => anyhow::bail!("liquidity requires an active formed federation"),
                };
                let connector = CliLiquidityConnector::new(endpoint.clone());
                let snapshot = reconcile_then_run_post_formation(
                    client.resume(),
                    client.start_liquidity(&formation_id, &provider, intent, &connector),
                )
                .await?;
                output.liquidity_snapshot(&snapshot, format)?;
                endpoint.close().await;
            }
            LiquidityCommand::Resume(resume) => {
                let endpoint = bind_iroh_endpoint().await?;
                let registry = connect_environment_registry(args.manifold_environment).await?;
                let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
                let client = open_client(
                    &args.state_dir,
                    identity,
                    CliPayments::unavailable(),
                    &setup_payment,
                    Some(registry),
                    CliFmanConnector::new(endpoint.clone()),
                    peer_badge_verifier
                        .clone()
                        .expect("liquidity resume constructs a PeerBadge verifier"),
                    profile.clone(),
                )
                .await?;
                let operation_id = LiquidityOperationId(resume.operation_id);
                client.liquidity_status(&operation_id).await?;
                let connector = CliLiquidityConnector::new(endpoint.clone());
                let snapshot = reconcile_then_run_post_formation(
                    client.resume(),
                    client.resume_liquidity(&operation_id, &connector),
                )
                .await?;
                output.liquidity_snapshot(&snapshot, format)?;
                endpoint.close().await;
            }
            LiquidityCommand::Status(status) => {
                let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
                let client = open_client(
                    &args.state_dir,
                    identity,
                    CliPayments::unavailable(),
                    &setup_payment,
                    None,
                    CliFmanConnector::unavailable(),
                    peer_badge_verifier
                        .clone()
                        .expect("liquidity status constructs a PeerBadge verifier"),
                    profile.clone(),
                )
                .await?;
                let snapshot = client
                    .liquidity_status(&LiquidityOperationId(status.operation_id))
                    .await?;
                output.liquidity_snapshot(&snapshot, format)?;
            }
            LiquidityCommand::List(list) => {
                let identity = CliIdentity::load_or_create(&args.state_dir, false)?;
                let client = open_client(
                    &args.state_dir,
                    identity,
                    CliPayments::unavailable(),
                    &setup_payment,
                    None,
                    CliFmanConnector::unavailable(),
                    peer_badge_verifier.expect("liquidity listing constructs a PeerBadge verifier"),
                    profile,
                )
                .await?;
                let after = list.after.map(LiquidityOperationId);
                let page = client
                    .list_liquidity_operations(after.as_ref(), list.limit)
                    .await?;
                output.liquidity_page(&page, format)?;
            }
        },
    }
    Ok(())
}

async fn reconcile_then_run_post_formation<T, E>(
    reconcile: impl std::future::Future<Output = Result<(), E>>,
    operation: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    reconcile.await?;
    operation.await
}

fn command_requires_peer_badge_verifier(command: &Command) -> bool {
    !matches!(command, Command::Discover(_) | Command::PaymentWallet(_))
}

fn parse_fedimintd_range(
    minimum: Option<&str>,
    maximum_exclusive: Option<&str>,
) -> anyhow::Result<FedimintdVersionRange> {
    match (minimum, maximum_exclusive) {
        (Some(minimum), Some(maximum)) => FedimintdVersionRange::new(
            minimum
                .parse()
                .context("parse --fedimintd-version-minimum as a semantic version")?,
            maximum
                .parse()
                .context("parse --fedimintd-version-maximum-exclusive as a semantic version")?,
        )
        .map_err(Into::into),
        (None, None) => FedimintdVersionRange::one_core(
            FEDIMINTD_VERSION_0_1
                .parse::<FedimintdVersion>()
                .expect("the supported fedimintd version constant is valid SemVer")
                .core(),
        )
        .map_err(Into::into),
        _ => bail!("fedimintd version range requires both minimum and maximum-exclusive bounds"),
    }
}

/// Connect the read-only registry enumeration to every canonical Nostr
/// relay in the environment profile.
///
/// The read-only enumeration publishes nothing, so ephemeral keys are used
/// rather than the FI identity. One reachable relay is enough for discovery
/// to work; the rest merge best-effort.
async fn connect_environment_registry(
    environment: ManifoldEnvironment,
) -> anyhow::Result<NostrFiClient> {
    let profile = environment.profile()?;
    let relays = profile.nostr_relays().as_urls();
    let relay_client =
        NostrRelayClient::connect_pool(relays, NostrKeys::generate(), REGISTRY_CONNECT_TIMEOUT)
            .await
            .with_context(|| format!("connect to canonical Nostr relays {relays:?}"))?;
    Ok(NostrFiClient::new(relay_client))
}

const REGISTRY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

async fn bind_iroh_endpoint() -> anyhow::Result<Endpoint> {
    if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
        Ok(Endpoint::bind(presets::N0DisableRelay).await?)
    } else {
        Ok(Endpoint::bind(presets::N0).await?)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "test consumer capabilities remain explicit at the fi-client construction boundary"
)]
async fn open_client(
    state_dir: &Path,
    identity: CliIdentity,
    payments: CliPayments,
    setup_payment: &CliSetupPayment,
    live_registry: Option<NostrFiClient>,
    connector: CliFmanConnector,
    peer_badge_verifier: PeerBadgeVerifier,
    profile: ManifoldEnvironmentProfile,
) -> anyhow::Result<CliClient> {
    open_client_with_fee_account_provider(
        state_dir,
        identity,
        payments,
        setup_payment,
        live_registry,
        connector,
        peer_badge_verifier,
        profile,
        CliFiFeeAccountProvider::unavailable(),
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "test consumer capabilities remain explicit at the fi-client construction boundary"
)]
async fn open_client_with_fee_account_provider(
    state_dir: &Path,
    identity: CliIdentity,
    payments: CliPayments,
    setup_payment: &CliSetupPayment,
    live_registry: Option<NostrFiClient>,
    connector: CliFmanConnector,
    peer_badge_verifier: PeerBadgeVerifier,
    profile: ManifoldEnvironmentProfile,
    fi_fee_account_provider: CliFiFeeAccountProvider,
) -> anyhow::Result<CliClient> {
    let database = fedimint_rocksdb::RocksDb::build(state_dir.join(DATABASE_DIR))
        .open()
        .await
        .context("open FI database")?
        .into();
    let registry = setup_payment.registry(live_registry);
    match setup_payment.publisher {
        Some(publisher) => FiClient::open_with_setup_payment_publisher(
            database,
            identity,
            payments,
            registry,
            connector,
            peer_badge_verifier,
            CliConsensusReader::new().await?,
            fi_fee_account_provider,
            publisher,
            profile.guardian_verification_fee_account().cloned(),
        )
        .await
        .context("open FI client"),
        None => FiClient::open_with_manifold_profile(
            database,
            identity,
            payments,
            registry,
            connector,
            peer_badge_verifier,
            CliConsensusReader::new().await?,
            fi_fee_account_provider,
            profile,
        )
        .await
        .context("open FI client"),
    }
}

fn load_intent(args: &CreateArgs) -> anyhow::Result<FormationIntent> {
    let mut file = match &args.config {
        Some(path) => toml::from_str::<IntentFile>(
            &std::fs::read_to_string(path)
                .with_context(|| format!("read intent config {}", path.display()))?,
        )
        .with_context(|| format!("parse intent config {}", path.display()))?,
        None => IntentFile::default(),
    };
    if let Some(value) = &args.federation_name {
        file.federation_name = Some(FederationName(value.clone()));
    }
    if let Some(value) = args.federation_size {
        file.federation_size = Some(FederationSize(value));
    }
    if let Some(value) = &args.fedimintd_version_minimum {
        file.fedimintd_version_minimum = Some(
            value
                .parse()
                .context("parse --fedimintd-version-minimum as a semantic version")?,
        );
    }
    if let Some(value) = &args.fedimintd_version_maximum_exclusive {
        file.fedimintd_version_maximum_exclusive = Some(
            value
                .parse()
                .context("parse --fedimintd-version-maximum-exclusive as a semantic version")?,
        );
    }
    if let Some(value) = args.max_total_msats {
        file.max_total_msats = Some(value);
    }
    let fedimintd_versions = match (
        file.fedimintd_version_minimum,
        file.fedimintd_version_maximum_exclusive,
    ) {
        (Some(minimum), Some(maximum)) => FedimintdVersionRange::new(minimum, maximum)?,
        (None, None) => FedimintdVersionRange::one_core(
            FEDIMINTD_VERSION_0_1
                .parse::<FedimintdVersion>()
                .expect("the supported fedimintd version constant is valid SemVer")
                .core(),
        )?,
        _ => bail!("fedimintd version range requires both minimum and maximum-exclusive bounds"),
    };
    let intent = FormationIntent::new(
        file.federation_name,
        file.federation_size
            .context("formation intent requires federation_size")?,
        file.plan.unwrap_or_default(),
        fedimintd_versions,
    )?;
    match file.max_total_msats {
        Some(max_total_msats) => intent
            .with_max_total_msats(max_total_msats)
            .map_err(Into::into),
        None => Ok(intent),
    }
}

async fn authorize_pending_payments(
    client: &CliClient,
    options: FormationRunOptions,
    format: OutputFormat,
    output: &mut CliOutput<'_>,
) -> anyhow::Result<()> {
    let status = client.status();
    let FiStatus::Formation(snapshot) = status else {
        return Ok(());
    };
    if snapshot.phase != FormationPhase::AwaitingPaymentReadiness {
        return Ok(());
    }
    let Some(FormationActionRequired::AuthorizePayments(requirements)) = snapshot.action_required
    else {
        anyhow::bail!("payment readiness did not expose aggregate requirements");
    };
    if payment_requirements_exceed_cap(&requirements) {
        output.payment_authorization_required(&requirements, format)?;
        return Ok(());
    }
    output.payment_requirements(&requirements, format)?;
    client
        .authorize_payments(requirements.authorization_id, options)
        .await
        .context("authorize aggregate seat payments")
}

fn payment_requirements_exceed_cap(requirements: &fi_client::PaymentRequirements) -> bool {
    requirements
        .max_total_msats
        .is_some_and(|cap| requirements.total_msats > cap)
}

/// Reads federation consensus through a real invite-code preview.
///
/// This is the implementation `FederationConsensusReader` expects: it performs
/// the config download and the `meta` consensus query itself. `fi-client`
/// cannot check that the query happened, so returning anything not obtained
/// this way would silently defeat the post-DKG readback.
struct CliConsensusReader {
    connectors: fedi_decentralized_federation_preview::ConnectorRegistry,
}

impl CliConsensusReader {
    async fn new() -> anyhow::Result<Self> {
        Ok(Self {
            connectors: fedi_decentralized_federation_preview::bind_client_connectors()
                .await
                .context("bind federation preview connectors")?,
        })
    }
}

impl FederationConsensusReader for CliConsensusReader {
    async fn read_consensus(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        let snapshot =
            fedi_decentralized_federation_preview::read_consensus(&self.connectors, invite_code)
                .await
                .map_err(|error| FederationConsensusError::new(error.to_string()))?;

        Ok(FederationConsensusSnapshot {
            config: snapshot.config,
            meta_value: snapshot.meta_value,
            meta_revision: snapshot.meta_revision,
            network: snapshot.network,
        })
    }

    async fn read_lnv2_gateways(
        &self,
        invite_code: &InviteCode,
    ) -> Result<Vec<fi_client::GatewayApiUrl>, FederationConsensusError> {
        fedi_decentralized_federation_preview::read_lnv2_gateways(&self.connectors, invite_code)
            .await
            .map_err(|error| FederationConsensusError::new(error.to_string()))
    }
}

#[derive(Clone)]
struct CliFmanConnector {
    endpoint: Option<Endpoint>,
}

impl CliFmanConnector {
    fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint: Some(endpoint),
        }
    }

    fn unavailable() -> Self {
        Self { endpoint: None }
    }
}

impl FleetManagerConnector for CliFmanConnector {
    type Client = FleetManagerServiceClient;

    async fn connect(&self, locator: &Locator) -> Result<Self::Client, FleetManagerConnectorError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| FleetManagerConnectorError::new("FMan transport is not configured"))?;
        endpoint
            .connect(locator.endpoint_addr.clone(), FLEET_MANAGER_ALPN)
            .await
            .map(FleetManagerServiceClient::new)
            .map_err(|error| FleetManagerConnectorError::new(error.to_string()))
    }

    async fn get_availability(
        &self,
        client: &Self::Client,
        request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, FleetManagerCallError> {
        client
            .transport()
            .get_availability(request)
            .await
            .map_err(|error| FleetManagerCallError::new(error.to_string()))
    }

    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, FleetManagerCallError> {
        client
            .transport()
            .get_quote(request)
            .await
            .map_err(|error| FleetManagerCallError::new(error.to_string()))
    }
}

#[derive(Clone)]
struct CliLiquidityConnector {
    endpoint: Endpoint,
}

impl CliLiquidityConnector {
    fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl LiquidityProviderConnector for CliLiquidityConnector {
    type Client = PublicLiquidityApiClient;

    async fn connect(
        &self,
        endpoint: &iroh::EndpointAddr,
    ) -> Result<Self::Client, LiquidityProviderConnectorError> {
        self.endpoint
            .connect(endpoint.clone(), PUBLIC_LIQUIDITY_API_ALPN)
            .await
            .map(PublicLiquidityApiClient::new)
            .map_err(|error| LiquidityProviderConnectorError::new(error.to_string()))
    }
}

enum PreparedRefundAny {
    V1 {
        federation_id: fedimint_core::config::FederationId,
        prepared: PreparedRefund,
        reservation_id: fi_client::PaymentReservationId,
        quote_id: QuoteId,
    },
    V2 {
        federation_id: fedimint_core::config::FederationId,
        module: fedimint_core::core::ModuleInstanceId,
        prepared: PreparedRefundV2,
        reservation_id: fi_client::PaymentReservationId,
        quote_id: QuoteId,
    },
}

struct CliPayments {
    wallet: Option<Wallet>,
    selected_federation: Option<fi_client::FederationId>,
}

/// The validated payment arguments a paid `create` needs, all together.
///
/// A run that passes none of them has no wallet and can only form seats an
/// FMan gives away — [`CliPayments::preflight`] answers `None` for it.
struct PaymentPreflight {
    wallet_data_dir: PathBuf,
    wallet_secret: WalletRootSecret,
    funding_token_file: Option<PathBuf>,
    selected: fedimint_core::config::FederationId,
    invite: Option<FedimintInviteCode>,
}

#[derive(Clone, Copy)]
enum PaymentAttempt<'a> {
    RecoverOnly,
    Create(&'a LockedPaymentReservation),
}

/// A decoded wallet root secret that clears its storage when dropped.
///
/// This type intentionally does not implement `Debug` or `Display`.
struct WalletRootSecret(WalletSecret);

/// A zeroizing plaintext buffer that deliberately cannot be formatted.
struct SecretBuffer<T: Zeroize>(Zeroizing<T>);

const MAX_ENCODED_WALLET_SECRET_BYTES: usize = 131;

impl<T: Zeroize> SecretBuffer<T> {
    fn new(value: T) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl<T: Zeroize> std::ops::Deref for SecretBuffer<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Zeroize> std::ops::DerefMut for SecretBuffer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for WalletRootSecret {
    fn drop(&mut self) {
        self.0.0.zeroize();
    }
}

impl WalletRootSecret {
    fn read_for(args: &AppArgs) -> anyhow::Result<Option<Self>> {
        let explicit_path = match &args.command {
            Command::Create(args) => args.wallet_secret_file.as_ref(),
            Command::Resume(args) => args.wallet_secret_file.as_ref(),
            Command::AuthorizePayments(args) => args.resume.wallet_secret_file.as_ref(),
            Command::PaymentWallet(args) => args.wallet_secret_file.as_ref(),
            Command::Init
            | Command::Status
            | Command::Discover(_)
            | Command::Preview(_)
            | Command::Maintenance(_)
            | Command::Liquidity(_) => {
                return Ok(None);
            }
        };
        if let Some(path) = explicit_path {
            return Self::read_file(path).map(Some);
        }
        std::env::var_os("FI_CLI_WALLET_SECRET_FILE")
            .map(PathBuf::from)
            .map(|path| Self::read_file(&path))
            .transpose()
    }

    #[cfg(unix)]
    fn read_file(path: &Path) -> anyhow::Result<Self> {
        use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = options
            .open(path)
            .map_err(|_| anyhow::anyhow!("could not open wallet root secret file"))?;
        let metadata = file
            .metadata()
            .map_err(|_| anyhow::anyhow!("could not inspect wallet root secret file"))?;
        ensure!(
            metadata.file_type().is_file(),
            "wallet root secret file is not a regular file"
        );
        // SAFETY: `geteuid` has no preconditions and does not dereference memory.
        let current_uid = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == current_uid,
            "wallet root secret file is not owned by the current user"
        );
        ensure!(
            metadata.mode() & 0o7777 == 0o600,
            "wallet root secret file permissions must be exactly 0600"
        );

        let (encoded, encoded_len) = read_wallet_secret_input(&file)?;
        ensure!(
            encoded_len < MAX_ENCODED_WALLET_SECRET_BYTES,
            "wallet root secret input is too long"
        );
        let encoded = std::str::from_utf8(&encoded[..encoded_len])
            .map_err(|_| anyhow::anyhow!("wallet root secret is not valid UTF-8"))?
            .trim_end_matches(['\r', '\n']);
        ensure!(
            encoded.len() == 128,
            "wallet root secret must encode exactly 64 bytes"
        );
        let mut secret = SecretBuffer::new([0_u8; 64]);
        hex::decode_to_slice(encoded, &mut *secret)
            .map_err(|_| anyhow::anyhow!("wallet root secret is not valid hexadecimal"))?;
        Ok(Self(WalletSecret(*secret)))
    }

    #[cfg(not(unix))]
    fn read_file(_path: &Path) -> anyhow::Result<Self> {
        anyhow::bail!("secure wallet root secret files are unsupported on this platform")
    }

    fn wallet_secret(&self) -> &WalletSecret {
        &self.0
    }
}

fn read_wallet_secret_input(
    mut reader: impl Read,
) -> anyhow::Result<(SecretBuffer<[u8; MAX_ENCODED_WALLET_SECRET_BYTES]>, usize)> {
    let mut encoded = SecretBuffer::new([0_u8; MAX_ENCODED_WALLET_SECRET_BYTES]);
    let mut encoded_len = 0;
    while encoded_len < encoded.len() {
        match reader.read(&mut encoded[encoded_len..]) {
            Ok(0) => break,
            Ok(read) => encoded_len += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => anyhow::bail!("could not read wallet root secret file"),
        }
    }
    Ok((encoded, encoded_len))
}

impl CliPayments {
    fn unavailable() -> Self {
        Self {
            wallet: None,
            selected_federation: None,
        }
    }

    fn preflight(
        args: &CreateArgs,
        wallet_secret: Option<WalletRootSecret>,
    ) -> anyhow::Result<Option<PaymentPreflight>> {
        if args.payment_federation_id.is_none()
            && args.wallet_data_dir.is_none()
            && wallet_secret.is_none()
        {
            return Ok(None);
        }
        ensure!(
            args.payment_federation_id.is_some()
                && args.wallet_data_dir.is_some()
                && wallet_secret.is_some(),
            "paying requires --payment-federation-id, --wallet-data-dir and \
             --wallet-secret-file (or FI_CLI_WALLET_SECRET_FILE) together; pass none of \
             them to form without paying"
        );
        ensure!(
            args.funding_token_file.is_none() || args.payment_invite_code.is_some(),
            "--funding-token-file requires --payment-invite-code"
        );
        let selected = args
            .payment_federation_id
            .as_deref()
            .expect("validated above")
            .parse::<fedimint_core::config::FederationId>()
            .context("parse --payment-federation-id")?;
        let invite = parse_payment_invite(args.payment_invite_code.as_deref(), selected)?;
        Ok(Some(PaymentPreflight {
            wallet_data_dir: args.wallet_data_dir.clone().expect("validated above"),
            wallet_secret: wallet_secret.expect("validated above"),
            funding_token_file: args.funding_token_file.clone(),
            selected,
            invite,
        }))
    }

    async fn open(
        preflight: Option<PaymentPreflight>,
        format: OutputFormat,
        output: &mut CliOutput<'_>,
    ) -> anyhow::Result<Self> {
        let Some(preflight) = preflight else {
            return Ok(Self::unavailable());
        };
        let PaymentPreflight {
            wallet_data_dir,
            wallet_secret,
            funding_token_file,
            selected,
            invite,
        } = preflight;
        let wallet = open_wallet(wallet_data_dir, wallet_secret).await?;
        let joined = if let Some(invite) = invite {
            wallet.join(&invite).await?
        } else {
            wallet.open_federation(selected).await?
        };
        if let Some(path) = &funding_token_file {
            let journal = FundingTokenJournal::prepare(path)?;
            let amount = match wallet.receive(joined, journal.token()).await {
                Ok(amount) => amount,
                Err(wallet::WalletError::InvalidToken(_)) => {
                    wallet.receive_v2(joined, journal.token()).await?
                }
                Err(error) => return Err(error.into()),
            };
            journal.complete()?;
            output.wallet_funded(amount, format)?;
        }
        Ok(Self {
            wallet: Some(wallet),
            selected_federation: Some(fi_client::FederationId(selected.to_string())),
        })
    }

    async fn open_for_resume(
        args: &ResumeArgs,
        wallet_secret: Option<WalletRootSecret>,
    ) -> anyhow::Result<Self> {
        match (
            &args.payment_federation_id,
            &args.wallet_data_dir,
            wallet_secret,
        ) {
            (None, None, None) => {
                ensure!(
                    args.payment_invite_code.is_none(),
                    "--payment-invite-code requires --payment-federation-id, --wallet-data-dir \
                     and --wallet-secret-file (or FI_CLI_WALLET_SECRET_FILE)"
                );
                ensure!(
                    args.funding_token_file.is_none(),
                    "--funding-token-file requires --payment-federation-id, --wallet-data-dir, \
                     --wallet-secret-file (or FI_CLI_WALLET_SECRET_FILE), and \
                     --payment-invite-code"
                );
                Ok(Self::unavailable())
            }
            (Some(federation_id), Some(data_dir), Some(wallet_secret)) => {
                let selected: fedimint_core::config::FederationId = federation_id.parse()?;
                let invite = parse_payment_invite(args.payment_invite_code.as_deref(), selected)?;
                ensure!(
                    args.funding_token_file.is_none() || invite.is_some(),
                    "--funding-token-file requires --payment-invite-code"
                );
                let wallet = open_wallet(data_dir.clone(), wallet_secret).await?;
                let joined = if let Some(invite) = &invite {
                    // Re-joining an already-joined federation reopens its
                    // existing client database, so non-terminal recovery can
                    // reach the payment federation again.
                    wallet.join(invite).await?
                } else {
                    wallet.open_federation(selected).await?
                };
                if let Some(path) = &args.funding_token_file {
                    let journal = FundingTokenJournal::prepare(path)?;
                    match wallet.receive(joined, journal.token()).await {
                        Ok(amount) => amount,
                        Err(wallet::WalletError::InvalidToken(_)) => {
                            wallet.receive_v2(joined, journal.token()).await?
                        }
                        Err(error) => return Err(error.into()),
                    };
                    journal.complete()?;
                }
                Ok(Self {
                    wallet: Some(wallet),
                    selected_federation: Some(fi_client::FederationId(selected.to_string())),
                })
            }
            _ => anyhow::bail!(
                "paid resume requires --payment-federation-id, --wallet-data-dir and \
                 --wallet-secret-file (or FI_CLI_WALLET_SECRET_FILE) together"
            ),
        }
    }
}

/// Parse an optional payment invite and require it to name the selected
/// payment federation before any wallet side effect.
fn parse_payment_invite(
    invite: Option<&str>,
    selected: fedimint_core::config::FederationId,
) -> anyhow::Result<Option<FedimintInviteCode>> {
    let invite = invite
        .map(|invite| {
            invite
                .parse::<FedimintInviteCode>()
                .context("parse --payment-invite-code")
        })
        .transpose()?;
    if let Some(invite) = &invite {
        ensure!(
            invite.federation_id() == selected,
            "payment invite belongs to a different federation"
        );
    }
    Ok(invite)
}

async fn open_wallet(data_dir: PathBuf, secret: WalletRootSecret) -> anyhow::Result<Wallet> {
    Wallet::open(data_dir, secret.wallet_secret()).await
}

impl FiPayments for CliPayments {
    type RefundContext = PreparedRefundAny;
    type PaymentReservation = LockedPaymentReservation;
    type TerminalReleaseProof = LockedPaymentTerminalRelease;

    async fn payable_federations(
        &self,
        admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FiPaymentError> {
        let Some(selected) = &self.selected_federation else {
            return Ok(Vec::new());
        };
        Ok(admitted
            .iter()
            .find(|candidate| *candidate == selected)
            .cloned()
            .into_iter()
            .collect())
    }

    async fn recover_payment_reservation(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        preflight: &fi_client::ExactPaymentPreflight<'_>,
    ) -> Result<PaymentReservationRecovery<Self::PaymentReservation>, FiPaymentError> {
        self.recover_exact_payment_reservation(reservation_id, preflight)
            .await
            .map(|reservation| match reservation {
                Some(reservation) => PaymentReservationRecovery::Existing(reservation),
                None => PaymentReservationRecovery::Absent,
            })
            .map_err(|error| FiPaymentError::new(error.to_string()))
    }

    async fn reserve_payment_requirements(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        preflight: &fi_client::ExactPaymentPreflight<'_>,
    ) -> Result<Self::PaymentReservation, FiPaymentError> {
        self.reserve_exact_payments(reservation_id, preflight)
            .await
            .map_err(map_reservation_error)
    }

    async fn release_payment_reservation(
        &self,
        reservation: Self::PaymentReservation,
    ) -> Result<(), FiPaymentError> {
        self.wallet
            .as_ref()
            .ok_or_else(|| FiPaymentError::new("payment wallet unavailable"))?
            .release_locked_payment_reservation(reservation)
            .await
            .map_err(|error| FiPaymentError::new(error.to_string()))
    }

    async fn release_seat_payment_reservation(
        &self,
        proof: Self::TerminalReleaseProof,
    ) -> Result<(), FiPaymentError> {
        self.wallet
            .as_ref()
            .ok_or_else(|| FiPaymentError::new("payment wallet unavailable"))?
            .release_locked_payment_member(proof)
            .await
            .map_err(|error| FiPaymentError::new(error.to_string()))
    }

    async fn prepare_quote_refund(
        &self,
        federation_id: &fi_client::FederationId,
        plan: &Plan,
    ) -> Result<RefundIssuance, FiPaymentError> {
        let Plan::InfiniteBestEffort { price_msats } = plan else {
            return Err(FiPaymentError::new(
                "refund requested for an unservable plan",
            ));
        };
        let federation_id = federation_id
            .0
            .parse()
            .map_err(|_| FiPaymentError::new("invalid payment federation id"))?;
        self.wallet
            .as_ref()
            .ok_or_else(|| FiPaymentError::new("payment wallet unavailable"))?
            .prepare_quote_refund(federation_id, *price_msats, rand::random())
            .await
            .map_err(|error| FiPaymentError::new(error.to_string()))
    }

    async fn recover_seat_payment(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext, Self::TerminalReleaseProof>, FiPaymentError>
    {
        self.prepare_seat_payment(reservation_id, quote, PaymentAttempt::RecoverOnly)
            .await
            .map_err(|error| FiPaymentError::new(error.to_string()))
    }

    async fn create_seat_payment(
        &self,
        reservation: &Self::PaymentReservation,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FiPaymentError> {
        match self
            .prepare_seat_payment(
                reservation.reservation_id(),
                quote,
                PaymentAttempt::Create(reservation),
            )
            .await
        {
            Ok(SeatPaymentRecovery::Prepared(payment)) => Ok(payment),
            Ok(SeatPaymentRecovery::NotStarted | SeatPaymentRecovery::Rejected(_)) => Err(
                FiPaymentError::new("wallet did not start the requested payment"),
            ),
            Err(error) => Err(FiPaymentError::new(error.to_string())),
        }
    }

    async fn settle_seat_refund(
        &self,
        context: Self::RefundContext,
        refund: RefundTransaction,
    ) -> Result<SettledSeatRefund<Self::TerminalReleaseProof>, FiPaymentError> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| FiPaymentError::new("payment wallet is not configured"))?;
        let settled = match context {
            PreparedRefundAny::V1 {
                federation_id,
                prepared,
                reservation_id,
                quote_id,
            } => {
                wallet
                    .submit_refund_v1(
                        federation_id,
                        &refund.0,
                        prepared,
                        &reservation_id,
                        quote_id,
                    )
                    .await
            }
            PreparedRefundAny::V2 {
                federation_id,
                module,
                prepared,
                reservation_id,
                quote_id,
            } => {
                wallet
                    .submit_refund_v2(
                        federation_id,
                        module,
                        &refund.0,
                        prepared,
                        &reservation_id,
                        quote_id,
                    )
                    .await
            }
        };
        let (amount, release_proof) = settled
            .map_err(|error| FiPaymentError::new(error.to_string()))?
            .into_parts();
        Ok(SettledSeatRefund {
            amount_msats: amount.msats,
            release_proof,
        })
    }
}

fn map_reservation_error(error: anyhow::Error) -> FiPaymentError {
    let message = error.to_string();
    if error
        .downcast_ref::<InsufficientLockedPaymentFundsWithoutReservation>()
        .is_some()
    {
        FiPaymentError::insufficient_funds_without_reservation(message)
    } else {
        FiPaymentError::new(message)
    }
}

impl CliPayments {
    async fn exact_wallet_preflights(
        &self,
        preflight: &fi_client::ExactPaymentPreflight<'_>,
    ) -> anyhow::Result<(
        fedimint_core::config::FederationId,
        Vec<LockedPaymentPreflight>,
    )> {
        let wallet = self.wallet.as_ref().context("paid quote requires wallet")?;
        let selected = self
            .selected_federation
            .as_ref()
            .context("paid quote requires an explicit payment federation")?;
        let selected_native: fedimint_core::config::FederationId = selected.0.parse()?;
        let mut wallet_preflights = Vec::with_capacity(preflight.seats().len());
        let mut checked_total_msats = 0u64;

        for seat in preflight.seats() {
            let requirement = seat.requirement();
            let quote = seat.quote();
            let mint_v2_module = match quote.terms.payment.as_ref() {
                Some(PaymentTerms::MintV2 { .. }) => {
                    // The module id is part of the exact v2 output-plan hash.
                    Some(wallet.first_mint_v2_module_id(selected_native).await?)
                }
                _ => None,
            };
            let (amount_msats, wallet_preflight) = concrete_wallet_preflight(
                selected,
                requirement,
                seat.quote_id(),
                &quote.terms,
                mint_v2_module,
            )?;
            checked_total_msats = add_payment_total(checked_total_msats, amount_msats)?;
            wallet_preflights.push(wallet_preflight);
        }
        validate_aggregate_payment_total(checked_total_msats, preflight.total_msats())?;
        Ok((selected_native, wallet_preflights))
    }

    async fn recover_exact_payment_reservation(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        preflight: &fi_client::ExactPaymentPreflight<'_>,
    ) -> anyhow::Result<Option<LockedPaymentReservation>> {
        let wallet = self.wallet.as_ref().context("paid quote requires wallet")?;
        let (selected, wallet_preflights) = self.exact_wallet_preflights(preflight).await?;
        // The fi-client reservation path reserves without a caller-owned
        // wallet floor (see `reserve_locked_payments`), so recovery expects
        // the same zero floor in the journal.
        wallet
            .recover_locked_payment_reservation(
                selected,
                reservation_id,
                &wallet_preflights,
                fedimint_core::Amount::ZERO,
            )
            .await
            .context("recover exact aggregate payment reservation")
    }

    async fn reserve_exact_payments(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        preflight: &fi_client::ExactPaymentPreflight<'_>,
    ) -> anyhow::Result<LockedPaymentReservation> {
        let wallet = self.wallet.as_ref().context("paid quote requires wallet")?;
        let (selected_native, wallet_preflights) = self.exact_wallet_preflights(preflight).await?;
        wallet
            .reserve_locked_payments(selected_native, reservation_id, &wallet_preflights)
            .await
            .context("exact aggregate payment is not ready")
    }

    async fn prepare_seat_payment(
        &self,
        reservation_id: &fi_client::PaymentReservationId,
        quote: &SignatureVerified<GetQuoteResponse>,
        attempt: PaymentAttempt<'_>,
    ) -> anyhow::Result<SeatPaymentRecovery<PreparedRefundAny, LockedPaymentTerminalRelease>> {
        let wallet = self.wallet.as_ref().context("paid quote requires wallet")?;
        match quote
            .terms
            .payment
            .as_ref()
            .context("paid quote has no payment terms")?
        {
            PaymentTerms::MintV1 {
                federation_id,
                issuance,
            } => {
                let Some(RefundIssuance::MintV1 { refund_nonce, .. }) =
                    &quote.terms.request.refund_issuance
                else {
                    anyhow::bail!("quote refund generation mismatch")
                };
                ensure!(
                    self.selected_federation.as_ref() == Some(federation_id),
                    "paid quote belongs to a different payment federation"
                );
                let federation_id: fedimint_core::config::FederationId = federation_id.0.parse()?;
                let issuance = issuance
                    .iter()
                    .map(|request| {
                        locked_payment::decode_issuance_request(
                            request.amount_msats,
                            &request.blind_nonce,
                        )
                        .map_err(Into::into)
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let (prepared, signatures) = match attempt {
                    PaymentAttempt::RecoverOnly => {
                        let signatures = match wallet
                            .recover_locked_v1(
                                federation_id,
                                &issuance,
                                quote.quote_id().0,
                                reservation_id,
                            )
                            .await?
                        {
                            LockedPaymentRecovery::Absent => {
                                return Ok(SeatPaymentRecovery::NotStarted);
                            }
                            LockedPaymentRecovery::Rejected(proof) => {
                                return Ok(SeatPaymentRecovery::Rejected(proof));
                            }
                            LockedPaymentRecovery::Funded(signatures) => signatures,
                        };
                        (
                            wallet
                                .prepare_refund_v1(federation_id, &issuance, *refund_nonce)
                                .await?,
                            signatures,
                        )
                    }
                    PaymentAttempt::Create(reservation) => {
                        let prepared = wallet
                            .prepare_refund_v1(federation_id, &issuance, *refund_nonce)
                            .await?;
                        let signatures = wallet
                            .pay_reserved_locked_v1(
                                reservation,
                                federation_id,
                                &issuance,
                                quote.quote_id().0,
                            )
                            .await?;
                        (prepared, signatures)
                    }
                };
                Ok(SeatPaymentRecovery::Prepared(PreparedSeatPayment {
                    payment_signatures: signatures
                        .into_iter()
                        .map(|signature| {
                            LockedBlindedSignature(signature.consensus_encode_to_vec())
                        })
                        .collect(),
                    settled_under: MintGeneration::MintV1,
                    refund_context: PreparedRefundAny::V1 {
                        federation_id,
                        prepared,
                        reservation_id: reservation_id.clone(),
                        quote_id: quote.quote_id(),
                    },
                }))
            }
            PaymentTerms::MintV2 {
                federation_id,
                issuance,
            } => {
                let Some(RefundIssuance::MintV2 { refund_nonce, .. }) =
                    &quote.terms.request.refund_issuance
                else {
                    anyhow::bail!("quote refund generation mismatch")
                };
                ensure!(
                    self.selected_federation.as_ref() == Some(federation_id),
                    "paid quote belongs to a different payment federation"
                );
                let federation_id: fedimint_core::config::FederationId = federation_id.0.parse()?;
                let module = wallet.first_mint_v2_module_id(federation_id).await?;
                let issuance = issuance
                    .iter()
                    .map(|request| {
                        Ok(locked_payment_v2::IssuanceRequest {
                            denomination: locked_payment_v2::denomination_from_amount(
                                request.amount_msats,
                            )?,
                            blind_nonce: locked_payment_v2::decode_blinded_message(
                                &request.blind_nonce,
                            )?,
                            tweak: request.tweak,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let (prepared, signatures) = match attempt {
                    PaymentAttempt::RecoverOnly => {
                        let signatures = match wallet
                            .recover_locked_v2(
                                federation_id,
                                module,
                                &issuance,
                                quote.quote_id().0,
                                reservation_id,
                            )
                            .await?
                        {
                            LockedPaymentRecovery::Absent => {
                                return Ok(SeatPaymentRecovery::NotStarted);
                            }
                            LockedPaymentRecovery::Rejected(proof) => {
                                return Ok(SeatPaymentRecovery::Rejected(proof));
                            }
                            LockedPaymentRecovery::Funded((signatures, _)) => signatures,
                        };
                        (
                            wallet
                                .prepare_refund_v2(federation_id, module, &issuance, *refund_nonce)
                                .await?,
                            signatures,
                        )
                    }
                    PaymentAttempt::Create(reservation) => {
                        let prepared = wallet
                            .prepare_refund_v2(federation_id, module, &issuance, *refund_nonce)
                            .await?;
                        let (signatures, _) = wallet
                            .pay_reserved_locked_v2(
                                reservation,
                                federation_id,
                                module,
                                &issuance,
                                quote.quote_id().0,
                            )
                            .await?;
                        (prepared, signatures)
                    }
                };
                Ok(SeatPaymentRecovery::Prepared(PreparedSeatPayment {
                    payment_signatures: signatures
                        .into_iter()
                        .map(|signature| {
                            LockedBlindedSignature(signature.consensus_encode_to_vec())
                        })
                        .collect(),
                    settled_under: MintGeneration::MintV2,
                    refund_context: PreparedRefundAny::V2 {
                        federation_id,
                        module,
                        prepared,
                        reservation_id: reservation_id.clone(),
                        quote_id: quote.quote_id(),
                    },
                }))
            }
        }
    }
}

fn concrete_wallet_preflight(
    selected: &fi_client::FederationId,
    requirement: &SeatPaymentRequirement,
    quote_id: QuoteId,
    terms: &QuoteTerms,
    mint_v2_module: Option<fedimint_core::core::ModuleInstanceId>,
) -> anyhow::Result<(u64, LockedPaymentPreflight)> {
    ensure!(
        &requirement.payment_federation_id == selected,
        "payment requirement belongs to a different payment federation"
    );
    let payment = terms
        .payment
        .as_ref()
        .context("paid quote has no payment terms")?;
    let (amount_msats, wallet_preflight) = match payment {
        PaymentTerms::MintV1 {
            federation_id,
            issuance,
        } => {
            ensure!(
                federation_id == selected,
                "paid quote belongs to a different payment federation"
            );
            ensure!(
                matches!(
                    terms.request.refund_issuance.as_ref(),
                    Some(RefundIssuance::MintV1 { .. })
                ),
                "quote refund generation mismatch"
            );
            let issuance = issuance
                .iter()
                .map(|request| {
                    let decoded = locked_payment::decode_issuance_request(
                        request.amount_msats,
                        &request.blind_nonce,
                    )?;
                    Ok(locked_payment::IssuanceRequest {
                        amount: decoded.amount,
                        blind_nonce: decoded.blind_nonce,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let amount_msats = issuance.iter().try_fold(0u64, |total, request| {
                total
                    .checked_add(request.amount.msats)
                    .context("mint-v1 quote amount overflow")
            })?;
            (
                amount_msats,
                LockedPaymentPreflight::mint_v1(quote_id, issuance),
            )
        }
        PaymentTerms::MintV2 {
            federation_id,
            issuance,
        } => {
            ensure!(
                federation_id == selected,
                "paid quote belongs to a different payment federation"
            );
            ensure!(
                matches!(
                    terms.request.refund_issuance.as_ref(),
                    Some(RefundIssuance::MintV2 { .. })
                ),
                "quote refund generation mismatch"
            );
            let module = mint_v2_module
                .context("quote requires mint-v2 but the selected wallet has no mint-v2 module")?;
            let issuance = issuance
                .iter()
                .map(|request| {
                    Ok(locked_payment_v2::IssuanceRequest {
                        denomination: locked_payment_v2::denomination_from_amount(
                            request.amount_msats,
                        )?,
                        blind_nonce: locked_payment_v2::decode_blinded_message(
                            &request.blind_nonce,
                        )?,
                        tweak: request.tweak,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let amount_msats = issuance.iter().try_fold(0u64, |total, request| {
                total
                    .checked_add(request.denomination.amount().msats)
                    .context("mint-v2 quote amount overflow")
            })?;
            (
                amount_msats,
                LockedPaymentPreflight::mint_v2(quote_id, module, issuance),
            )
        }
    };
    ensure!(
        amount_msats == requirement.amount_msats,
        "payment requirement amount differs from its quote"
    );
    Ok((amount_msats, wallet_preflight))
}

fn add_payment_total(total_msats: u64, amount_msats: u64) -> anyhow::Result<u64> {
    total_msats
        .checked_add(amount_msats)
        .context("aggregate payment amount overflow")
}

fn validate_aggregate_payment_total(actual_msats: u64, expected_msats: u64) -> anyhow::Result<()> {
    ensure!(
        actual_msats == expected_msats,
        "aggregate payment total differs from its quotes"
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct CliIdentity {
    secret_key: SecretKey,
}

impl CliIdentity {
    fn load_or_create(state_dir: &Path, create: bool) -> anyhow::Result<Self> {
        std::fs::create_dir_all(state_dir).context("create FI state directory")?;
        let path = state_dir.join(IDENTITY_FILE);
        if path.exists() {
            return Self::load(&path);
        }
        ensure!(
            create,
            "FI state is not initialized; run fi-cli --state-dir {} init",
            state_dir.display()
        );
        let secret_key = SecretKey::new(&mut OsRng);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).context("create FI identity")?;
        file.write_all(&secret_key.secret_bytes())
            .context("write FI identity")?;
        file.sync_all().context("sync FI identity")?;
        Ok(Self { secret_key })
    }

    fn load(path: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .context("open FI identity")?;
        let bytes = read_identity_bytes(file).context("read FI identity")?;
        ensure!(bytes.len() == 32, "invalid FI identity length");
        let secret_key = SecretKey::from_slice(&bytes).context("invalid FI identity key")?;
        Ok(Self { secret_key })
    }
}

fn read_identity_bytes(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(33);
    reader.take(33).read_to_end(&mut bytes)?;
    Ok(bytes)
}

impl FiIdentity for CliIdentity {
    fn public_key(&self) -> Result<FiId, String> {
        let keypair = Keypair::from_secret_key(&Secp256k1::new(), &self.secret_key);
        let (public_key, _) = XOnlyPublicKey::from_keypair(&keypair);
        Ok(FiId(public_key))
    }

    fn sign_digest(&self, digest: [u8; 32]) -> Result<FiSignature, String> {
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &self.secret_key);
        Ok(FiSignature(
            secp.sign_schnorr_no_aux_rand(&digest, &keypair),
        ))
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    fn test_payment_federation(byte: u8) -> fi_client::FederationId {
        fi_client::FederationId(format!("{byte:02x}").repeat(32))
    }

    fn test_locked_request(amount_msats: u64, binding: u8) -> LockedIssuanceRequest {
        let (issuance, _) = locked_payment::derive_issuance_requests(
            &[binding; 64],
            &[binding],
            &[fedimint_core::Amount::from_msats(amount_msats)],
        );
        LockedIssuanceRequest {
            amount_msats,
            blind_nonce: issuance[0].blind_nonce.consensus_encode_to_vec(),
        }
    }

    fn test_quote_terms(
        payment_federation: fi_client::FederationId,
        payment: PaymentTerms,
        refund_issuance: RefundIssuance,
        price_msats: u64,
    ) -> QuoteTerms {
        let secret = SecretKey::from_byte_array(&[5; 32]).unwrap();
        let keypair = Keypair::from_secret_key(&Secp256k1::new(), &secret);
        QuoteTerms {
            quote_nonce: [7; 32],
            offer_epoch: OfferEpoch::from_bytes([8; 32]),
            request: GetQuoteRequest {
                fi_id: FiId(keypair.x_only_public_key().0),
                fedimintd_version: "0.11.1-fedi10".parse().unwrap(),
                federation_size: FederationSize(7),
                plan: Plan::InfiniteBestEffort { price_msats },
                payment_federation_id: Some(payment_federation),
                refund_issuance: Some(refund_issuance),
            },
            price_msats,
            payment: Some(payment),
        }
    }

    fn test_requirement(
        federation_id: fi_client::FederationId,
        amount_msats: u64,
    ) -> SeatPaymentRequirement {
        SeatPaymentRequirement {
            index: 0,
            fman_id: None,
            quote_id: QuoteId([3; 32]),
            payment_federation_id: federation_id,
            amount_msats,
        }
    }

    #[test]
    fn registry_query_rejects_a_zero_timeout() {
        // Like the pinned-driver timing flags, a zero deadline is an
        // argument error rather than being clamped up to the runtime
        // quantum.
        let args = RegistryQueryArgs {
            federation_size: 7,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            timeout_secs: 0,
        };
        let error = args.discovery_options().unwrap_err();
        assert!(
            format!("{error:#}").contains("--timeout-secs must be at least one second"),
            "{error:#}"
        );
        assert!(
            RegistryQueryArgs {
                timeout_secs: 60,
                ..args
            }
            .discovery_options()
            .is_ok()
        );
    }

    #[test]
    fn payment_wallet_preflight_accepts_private_invites_and_followups_use_ids() {
        let federation_id: FedimintFederationId = "11".repeat(32).parse().unwrap();
        let invite = FedimintInviteCode::new(
            "https://guardian.example/".parse().unwrap(),
            fedimint_core::PeerId::from(0),
            federation_id,
            Some("test-api-secret".to_owned()),
        )
        .to_string();
        let joined = PaymentWalletArgs {
            wallet_data_dir: PathBuf::from("wallet"),
            wallet_secret_file: Some(PathBuf::from("secret")),
            command: PaymentWalletCommand::Join(PaymentWalletJoinArgs {
                payment_invite_code: invite,
            }),
        }
        .preflight()
        .unwrap();
        assert_eq!(joined.federation_id, federation_id);
        assert!(
            joined
                .invite
                .as_ref()
                .is_some_and(|invite| invite.api_secret().is_some())
        );

        let followup = PaymentWalletArgs {
            wallet_data_dir: PathBuf::from("wallet"),
            wallet_secret_file: Some(PathBuf::from("secret")),
            command: PaymentWalletCommand::DepositAddress(PaymentWalletDepositAddressArgs {
                federation: PaymentWalletFederationArgs {
                    payment_federation_id: federation_id.to_string(),
                },
                timeout_secs: 30,
            }),
        }
        .preflight()
        .unwrap();
        assert_eq!(followup.federation_id, federation_id);
        assert!(followup.invite.is_none());
    }

    #[test]
    fn payment_wallet_amounts_and_deadlines_fail_before_wallet_access() {
        assert!(sats_amount(u64::MAX).is_err());
        let deposit_error = PaymentWalletArgs {
            wallet_data_dir: PathBuf::from("wallet"),
            wallet_secret_file: Some(PathBuf::from("secret")),
            command: PaymentWalletCommand::DepositAddress(PaymentWalletDepositAddressArgs {
                federation: PaymentWalletFederationArgs {
                    payment_federation_id: "11".repeat(32),
                },
                timeout_secs: 0,
            }),
        }
        .preflight()
        .unwrap_err();
        assert!(deposit_error.to_string().contains("--timeout-secs"));

        let federation = PaymentWalletFederationArgs {
            payment_federation_id: "11".repeat(32),
        };
        let error = PaymentWalletArgs {
            wallet_data_dir: PathBuf::from("wallet"),
            wallet_secret_file: Some(PathBuf::from("secret")),
            command: PaymentWalletCommand::WaitBalance(PaymentWalletWaitBalanceArgs {
                federation,
                minimum_sats: 21_000,
                timeout_secs: 0,
            }),
        }
        .preflight()
        .unwrap_err();
        assert!(error.to_string().contains("--timeout-secs"));
    }

    #[test]
    fn top_level_help_states_security_scope() {
        let mut help = Vec::new();
        AppArgs::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(help.contains("Development/test-only Federation Initiator client"));
        assert!(help.contains("Unsupported for production use"));
        assert!(help.contains("Use only test credentials/material and test funds"));
    }

    #[test]
    fn completion_callback_flags_must_be_paired() {
        let base = [
            "fi-cli",
            "create",
            "--federation-size",
            "7",
            "--fi-spv2-account-file",
            "fi-account.json",
        ];
        assert!(
            AppArgs::try_parse_from(
                base.into_iter()
                    .chain(["--completion-callback-url-file", "callback-url"]),
            )
            .is_err()
        );
        assert!(
            AppArgs::try_parse_from(
                base.into_iter()
                    .chain(["--completion-callback-idempotency-key", "formation-1"]),
            )
            .is_err()
        );

        let args = AppArgs::try_parse_from(base.into_iter().chain([
            "--completion-callback-url-file",
            "callback-url",
            "--completion-callback-idempotency-key",
            "formation-1",
        ]))
        .unwrap();
        let Command::Create(create) = args.command else {
            panic!("expected create command")
        };
        assert_eq!(
            create.completion_callback_url_file.as_deref(),
            Some(Path::new("callback-url"))
        );
        assert_eq!(
            create.completion_callback_idempotency_key.as_deref(),
            Some("formation-1")
        );
    }

    #[tokio::test]
    async fn missing_completion_callback_file_precedes_state_access() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("fi-state");
        let callback_file = dir.path().join("missing-callback-url");
        let args = AppArgs::try_parse_from([
            "fi-cli",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "create",
            "--federation-size",
            "7",
            "--fi-spv2-account-file",
            "fi-account.json",
            "--completion-callback-url-file",
            callback_file.to_str().unwrap(),
            "--completion-callback-idempotency-key",
            "formation-1",
        ])
        .unwrap();

        let error = execute(args).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("could not open completion callback URL file"),
            "{error:#}"
        );
        assert!(!state_dir.exists());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn completion_callback_requires_pinned_formation_before_state_access() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("fi-state");
        let callback_file = dir.path().join("callback-url");
        std::fs::write(
            &callback_file,
            "https://push.example/hooks/hook-id/hook-secret\n",
        )
        .unwrap();
        std::fs::set_permissions(&callback_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let args = AppArgs::try_parse_from([
            "fi-cli",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "create",
            "--federation-size",
            "7",
            "--fi-spv2-account-file",
            "fi-account.json",
            "--completion-callback-url-file",
            callback_file.to_str().unwrap(),
            "--completion-callback-idempotency-key",
            "formation-1",
        ])
        .unwrap();

        let error = execute(args).await.unwrap_err();
        assert!(error.to_string().contains("pinned --locator"), "{error:#}");
        assert!(!state_dir.exists());
    }

    #[test]
    #[cfg(unix)]
    fn completion_callback_url_file_is_secure_and_bounded() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("callback-url");
        std::fs::write(&path, "https://push.example/hooks/id/secret\r\n").unwrap();
        assert!(read_completion_callback_url(&path).is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_completion_callback_url(&path).unwrap(),
            "https://push.example/hooks/id/secret"
        );

        let link = dir.path().join("callback-url-link");
        symlink(&path, &link).unwrap();
        assert!(read_completion_callback_url(&link).is_err());

        std::fs::write(&path, vec![b'a'; MAX_CALLBACK_URL_FILE_BYTES as usize + 1]).unwrap();
        assert!(read_completion_callback_url(&path).is_err());
    }

    #[test]
    fn liquidity_workflow_parses_for_e2e_use() {
        let args = AppArgs::try_parse_from([
            "fi-cli",
            "--manifold-environment",
            "staging",
            "liquidity",
            "request",
            "--network",
            "signet",
            "--gateway-min-sats",
            "100000",
            "--gateway-max-sats",
            "200000",
            "--provider-pubkey",
            "provider-1",
        ])
        .unwrap();

        assert_eq!(args.manifold_environment, ManifoldEnvironment::Staging);
        let Command::Liquidity(LiquidityArgs {
            command: LiquidityCommand::Request(request),
        }) = args.command
        else {
            panic!("expected liquidity request")
        };
        assert_eq!(request.intent.network, BitcoinNetwork::Signet);
        assert_eq!(request.intent.gateway_min_sats, 100_000);
        assert_eq!(request.intent.gateway_max_sats, Some(200_000));
        assert_eq!(request.provider_pubkey.as_deref(), Some("provider-1"));
    }

    #[test]
    fn maintenance_workflow_parses_and_prevalidates_library_types() {
        let args = AppArgs::try_parse_from([
            "fi-cli",
            "--manifold-environment",
            "staging",
            "maintenance",
            "set-name",
            "--value",
            "Staging Federation",
            "--run-timeout-secs",
            "90",
        ])
        .unwrap();
        let Command::Maintenance(maintenance) = args.command else {
            panic!("expected maintenance command")
        };
        assert_eq!(maintenance.timing.run_timeout_secs, 90);
        let MaintenancePreflight::Metadata {
            field,
            value,
            options: _,
            update: _,
        } = maintenance.preflight().unwrap()
        else {
            panic!("expected metadata maintenance preflight")
        };
        assert_eq!(field, FEDERATION_NAME_META_FIELD_KEY);
        assert_eq!(value, "Staging Federation");

        let invalid_name = MaintenanceArgs {
            timing: MaintenanceTimingArgs {
                poll_interval_secs: 2,
                run_timeout_secs: 600,
                request_timeout_secs: 30,
            },
            command: MaintenanceCommand::SetName(MetadataValueArgs {
                value: "no".to_owned(),
            }),
        };
        assert!(
            invalid_name
                .preflight()
                .unwrap_err()
                .to_string()
                .contains("validate federation metadata name")
        );

        let invalid_fee = MaintenanceArgs {
            timing: MaintenanceTimingArgs {
                poll_interval_secs: 2,
                run_timeout_secs: 600,
                request_timeout_secs: 30,
            },
            command: MaintenanceCommand::ConfigureGuardianFees(ConfigureGuardianFeesArgs {
                send_ppm: fi_client::MAX_GUARDIAN_FEE_PPM + 1,
            }),
        };
        assert_eq!(
            invalid_fee.preflight().unwrap_err().to_string(),
            format!(
                "--send-ppm must not exceed {}",
                fi_client::MAX_GUARDIAN_FEE_PPM
            )
        );
    }

    #[test]
    fn guardian_fee_preflight_accepts_only_the_rate() {
        let maintenance = MaintenanceArgs {
            timing: MaintenanceTimingArgs {
                poll_interval_secs: 2,
                run_timeout_secs: 600,
                request_timeout_secs: 30,
            },
            command: MaintenanceCommand::ConfigureGuardianFees(ConfigureGuardianFeesArgs {
                send_ppm: GuardianFeePpm::MANIFOLD_DEFAULT.value(),
            }),
        };

        let preflight = maintenance.preflight().unwrap();
        let MaintenancePreflight::GuardianFees { send_ppm, .. } = &preflight else {
            panic!("expected guardian-fee preflight")
        };
        assert_eq!(*send_ppm, GuardianFeePpm::MANIFOLD_DEFAULT);
    }

    #[tokio::test]
    async fn post_formation_operation_is_polled_only_after_successful_reconciliation() {
        // CLI liquidity and maintenance call sites use this helper after
        // opening persisted Formed state, which fi-client restores unsynced.
        let fresh = std::cell::Cell::new(false);
        let operation_polled = std::cell::Cell::new(false);
        let outcome = reconcile_then_run_post_formation(
            async {
                assert!(!fresh.get(), "the reopened formation starts unsynced");
                fresh.set(true);
                Ok::<(), &'static str>(())
            },
            async {
                operation_polled.set(true);
                if fresh.get() {
                    Ok("liquidity operation ran")
                } else {
                    Err("formation is unsynced")
                }
            },
        )
        .await;

        assert_eq!(outcome, Ok("liquidity operation ran"));
        assert!(operation_polled.get());

        operation_polled.set(false);
        let outcome = reconcile_then_run_post_formation(
            async { Err::<(), &'static str>("reconciliation failed") },
            async {
                operation_polled.set(true);
                Ok("liquidity operation ran")
            },
        )
        .await;

        assert_eq!(outcome, Err("reconciliation failed"));
        assert!(
            !operation_polled.get(),
            "a failed reconciliation must not start a post-formation operation"
        );
    }

    struct ChunkedReader {
        bytes: Vec<u8>,
        offset: usize,
        interrupted: bool,
    }

    impl Read for ChunkedReader {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let remaining = &self.bytes[self.offset..];
            let count = remaining.len().min(output.len()).min(3);
            output[..count].copy_from_slice(&remaining[..count]);
            self.offset += count;
            Ok(count)
        }
    }

    #[test]
    fn bounded_wallet_secret_read_handles_short_and_interrupted_reads() {
        for length in [128, 131] {
            let (encoded, read) = read_wallet_secret_input(ChunkedReader {
                bytes: vec![b'a'; length],
                offset: 0,
                interrupted: false,
            })
            .expect("read chunked secret");
            assert_eq!(read, length);
            assert!(encoded[..read].iter().all(|byte| *byte == b'a'));
        }
    }

    #[test]
    fn wallet_root_secret_cannot_be_formatted() {
        trait AmbiguousIfDebug<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<u8> for T {}

        trait AmbiguousIfDisplay<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfDisplay<()> for T {}
        impl<T: ?Sized + std::fmt::Display> AmbiguousIfDisplay<u8> for T {}

        <WalletRootSecret as AmbiguousIfDebug<_>>::check();
        <WalletRootSecret as AmbiguousIfDisplay<_>>::check();
        <SecretBuffer<Vec<u8>> as AmbiguousIfDebug<_>>::check();
        <SecretBuffer<Vec<u8>> as AmbiguousIfDisplay<_>>::check();
        <SecretBuffer<[u8; 64]> as AmbiguousIfDebug<_>>::check();
        <SecretBuffer<[u8; 64]> as AmbiguousIfDisplay<_>>::check();
    }

    #[test]
    fn identity_is_stable_and_restrictive() {
        let dir = tempfile::tempdir().unwrap();
        let first = CliIdentity::load_or_create(dir.path(), true).unwrap();
        let second = CliIdentity::load_or_create(dir.path(), false).unwrap();
        assert_eq!(first.public_key().unwrap(), second.public_key().unwrap());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(IDENTITY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn identity_read_is_bounded_to_its_fixed_format() {
        struct CountingReader {
            remaining: usize,
            read: std::rc::Rc<std::cell::Cell<usize>>,
        }

        impl Read for CountingReader {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let count = output.len().min(self.remaining);
                output[..count].fill(1);
                self.remaining -= count;
                self.read.set(self.read.get() + count);
                Ok(count)
            }
        }

        assert_eq!(read_identity_bytes(&[1_u8; 32][..]).unwrap().len(), 32);
        assert_eq!(read_identity_bytes(&[1_u8; 33][..]).unwrap().len(), 33);

        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let bytes = read_identity_bytes(CountingReader {
            remaining: 1024,
            read: std::rc::Rc::clone(&read),
        })
        .unwrap();
        assert_eq!(bytes.len(), 33);
        assert_eq!(read.get(), 33);
    }

    #[test]
    fn cli_overrides_toml_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intent.toml");
        std::fs::write(
            &path,
            r#"
federation_name = "file"
federation_size = 7
fedimintd_version_minimum = "0.11.1"
fedimintd_version_maximum_exclusive = "0.11.3"
max_total_msats = 1000
"#,
        )
        .unwrap();
        let intent = load_intent(&CreateArgs {
            fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
            locators: Vec::new(),
            insecure_skip_fman_trust: false,
            config: Some(path),
            federation_name: Some("override".to_owned()),
            federation_size: None,
            max_total_msats: Some(21_000),
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            poll_interval_secs: 2,
            poll_timeout_secs: 600,
            completion_callback_url_file: None,
            completion_callback_idempotency_key: None,
            payment_federation_id: None,
            wallet_data_dir: None,
            wallet_secret_file: None,
            payment_invite_code: None,
            funding_token_file: None,
        })
        .unwrap();
        assert_eq!(
            intent.federation_name(),
            Some(&FederationName("override".to_owned()))
        );
        assert_eq!(intent.federation_size(), FederationSize(7));
        assert_eq!(intent.fedimintd_versions().minimum().to_string(), "0.11.1");
        assert_eq!(
            intent.fedimintd_versions().maximum_exclusive().to_string(),
            "0.11.3"
        );
        assert_eq!(intent.plan(), PlanPreference::InfiniteBestEffort);
        assert_eq!(intent.max_total_msats(), Some(21_000));
    }

    #[test]
    fn spending_cap_rejects_zero_from_flag_and_toml() {
        let from_flag = load_intent(&CreateArgs {
            fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
            locators: Vec::new(),
            insecure_skip_fman_trust: false,
            config: None,
            federation_name: None,
            federation_size: Some(7),
            max_total_msats: Some(0),
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            poll_interval_secs: 2,
            poll_timeout_secs: 600,
            completion_callback_url_file: None,
            completion_callback_idempotency_key: None,
            payment_federation_id: None,
            wallet_data_dir: None,
            wallet_secret_file: None,
            payment_invite_code: None,
            funding_token_file: None,
        })
        .unwrap_err();
        assert!(from_flag.to_string().contains("greater than zero"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intent.toml");
        std::fs::write(
            &path,
            "federation_size = 7\nplan = \"infinite_best_effort\"\nmax_total_msats = 0\n",
        )
        .unwrap();
        let from_toml = load_intent(&CreateArgs {
            fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
            locators: Vec::new(),
            insecure_skip_fman_trust: false,
            config: Some(path),
            federation_name: None,
            federation_size: None,
            max_total_msats: None,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            poll_interval_secs: 2,
            poll_timeout_secs: 600,
            completion_callback_url_file: None,
            completion_callback_idempotency_key: None,
            payment_federation_id: None,
            wallet_data_dir: None,
            wallet_secret_file: None,
            payment_invite_code: None,
            funding_token_file: None,
        })
        .unwrap_err();
        assert!(from_toml.to_string().contains("greater than zero"));
    }

    #[test]
    fn toml_intent_rejects_the_retired_guardian_fee_field() {
        // The initial rate is compiled rather than supplied by formation intent, so
        // the creation-time TOML schema does not know the rate field. A
        // config still carrying it is rejected rather than silently ignored.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intent.toml");
        std::fs::write(&path, "federation_size = 7\nguardian_fee_ppm = 100000\n").unwrap();
        let error = load_intent(&CreateArgs {
            fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
            locators: Vec::new(),
            insecure_skip_fman_trust: false,
            config: Some(path),
            federation_name: None,
            federation_size: None,
            max_total_msats: None,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            poll_interval_secs: 2,
            poll_timeout_secs: 600,
            completion_callback_url_file: None,
            completion_callback_idempotency_key: None,
            payment_federation_id: None,
            wallet_data_dir: None,
            wallet_secret_file: None,
            payment_invite_code: None,
            funding_token_file: None,
        })
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("unknown field `guardian_fee_ppm`"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn toml_intent_rejects_unknown_fields_before_state_access() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("intent.toml");
        let state_dir = dir.path().join("state");
        let wallet_secret_file = dir.path().join("missing-wallet-secret");
        let setup_payment_event_file = dir.path().join("missing-setup-payment-event");
        let funding_token_file = dir.path().join("funding-token");
        let funding_token = "sentinel-token";
        std::fs::write(&funding_token_file, funding_token).unwrap();
        let funding_token_journal =
            funding_token_journal::journal_path(&funding_token_file).unwrap();
        std::fs::write(
            &config,
            r#"
federation_size = 7
fedimintd_version_minimum = "0.11.1"
fedimintd_version_maximum_exclusive = "0.11.2"
guardian_fee_pmm = 100
"#,
        )
        .unwrap();

        let args = AppArgs::try_parse_from([
            "fi-cli",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--setup-payment-publisher",
            "not-a-public-key",
            "--setup-payment-event-file",
            setup_payment_event_file.to_str().unwrap(),
            "create",
            "--fi-spv2-account-file",
            dir.path().join("missing-fi-account").to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
            "--wallet-secret-file",
            wallet_secret_file.to_str().unwrap(),
            "--funding-token-file",
            funding_token_file.to_str().unwrap(),
        ])
        .unwrap();

        let error = execute(args).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("unknown field `guardian_fee_pmm`"),
            "{error:#}"
        );
        assert!(!state_dir.exists());
        assert_eq!(
            std::fs::read_to_string(&funding_token_file).unwrap(),
            funding_token
        );
        assert!(!funding_token_journal.exists());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn create_validation_precedes_wallet_and_fi_state_side_effects() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("funding-token");
        std::fs::write(&token, "bearer token fixture").unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let invite = FedimintInviteCode::new(
            "https://guardian.example/".parse().unwrap(),
            fedimint_core::PeerId::from(0),
            "11".repeat(32).parse().unwrap(),
            None,
        )
        .to_string();
        let locators: Vec<String> = (0..7)
            .map(|index| {
                let endpoint_key = iroh::SecretKey::from_bytes(&[index + 1; 32]);
                let service_key =
                    SecretKey::from_byte_array(&[index + 20; 32]).expect("valid test key");
                Locator::new(
                    iroh::EndpointAddr::new(endpoint_key.public()),
                    service_key.x_only_public_key(secp256k1::SECP256K1).0,
                )
                .to_json()
            })
            .collect();
        let create_args = |poll_timeout_secs, payment_federation_id, wallet_data_dir| CreateArgs {
            fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
            locators: locators.clone(),
            insecure_skip_fman_trust: false,
            config: None,
            federation_name: None,
            federation_size: Some(7),
            max_total_msats: None,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            poll_interval_secs: 2,
            poll_timeout_secs,
            completion_callback_url_file: None,
            completion_callback_idempotency_key: None,
            payment_federation_id: Some(payment_federation_id),
            wallet_data_dir: Some(wallet_data_dir),
            wallet_secret_file: None,
            payment_invite_code: Some(invite.clone()),
            funding_token_file: Some(token.clone()),
        };
        let create = create_args(600, "22".repeat(32), dir.path().join("wallet"));
        let state_dir = dir.path().join("fi-state");
        let args = AppArgs {
            state_dir: state_dir.clone(),
            json: false,
            setup_payment_event_file: None,
            setup_payment_publisher: None,
            manifold_environment: ManifoldEnvironment::Development,
            command: Command::Create(Box::new(create)),
        };
        let intent = match &args.command {
            Command::Create(create) => load_intent(create).unwrap(),
            _ => unreachable!(),
        };

        let error = Box::pin(run(
            args,
            Some(WalletRootSecret(WalletSecret([42; 64]))),
            Some(CreatePreflight {
                intent,
                options: FormationRunOptions::default(),
                fi_fee_account_provider: CliFiFeeAccountProvider::unavailable(),
                completion_callback: None,
            }),
            None,
            None,
        ))
        .await
        .unwrap_err();

        assert!(error.to_string().contains("different federation"));
        assert_eq!(
            std::fs::read_to_string(&token).unwrap(),
            "bearer token fixture"
        );
        assert!(
            !funding_token_journal::journal_path(&token)
                .unwrap()
                .exists()
        );
        assert!(!dir.path().join("wallet").exists());
        assert!(!state_dir.exists());

        let registry_token = dir.path().join("registry-funding-token");
        std::fs::write(&registry_token, "second bearer token").unwrap();
        std::fs::set_permissions(&registry_token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let registry_state = dir.path().join("registry-fi-state");
        let registry_args = AppArgs {
            state_dir: registry_state.clone(),
            json: false,
            setup_payment_event_file: None,
            setup_payment_publisher: None,
            manifold_environment: ManifoldEnvironment::Development,
            command: Command::Create(Box::new(CreateArgs {
                fi_spv2_account_file: PathBuf::from("unused-fi-account.json"),
                locators: Vec::new(),
                insecure_skip_fman_trust: false,
                config: None,
                federation_name: None,
                federation_size: Some(7),
                max_total_msats: None,
                fedimintd_version_minimum: None,
                fedimintd_version_maximum_exclusive: None,
                poll_interval_secs: 2,
                poll_timeout_secs: 600,
                completion_callback_url_file: None,
                completion_callback_idempotency_key: None,
                payment_federation_id: Some("11".repeat(32)),
                wallet_data_dir: Some(dir.path().join("registry-wallet")),
                wallet_secret_file: None,
                payment_invite_code: Some(
                    FedimintInviteCode::new(
                        "https://guardian.example/".parse().unwrap(),
                        fedimint_core::PeerId::from(0),
                        "11".repeat(32).parse().unwrap(),
                        None,
                    )
                    .to_string(),
                ),
                funding_token_file: Some(registry_token.clone()),
            })),
        };
        let registry_intent = match &registry_args.command {
            Command::Create(create) => load_intent(create).unwrap(),
            _ => unreachable!(),
        };

        let error = Box::pin(run(
            registry_args,
            Some(WalletRootSecret(WalletSecret([43; 64]))),
            Some(CreatePreflight {
                intent: registry_intent,
                options: FormationRunOptions::default(),
                fi_fee_account_provider: CliFiFeeAccountProvider::unavailable(),
                completion_callback: None,
            }),
            None,
            None,
        ))
        .await
        .unwrap_err();

        assert!(error.to_string().contains("--max-total-msats"));
        assert_eq!(
            std::fs::read_to_string(&registry_token).unwrap(),
            "second bearer token"
        );
        assert!(
            !funding_token_journal::journal_path(&registry_token)
                .unwrap()
                .exists()
        );
        assert!(!dir.path().join("registry-wallet").exists());
        assert!(!registry_state.exists());
    }

    #[test]
    fn publisher_only_configuration_reuses_durable_policy() {
        let publisher = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let args =
            AppArgs::try_parse_from(["fi-cli", "--setup-payment-publisher", publisher, "status"])
                .expect("publisher-only CLI configuration parses");
        let setup = CliSetupPayment::load(&args).expect("publisher-only policy loads");

        assert_eq!(
            setup.publisher,
            Some(NostrPublicKey::parse(publisher).expect("test publisher parses"))
        );
        assert!(setup.event.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn funding_token_journal_is_reused_until_receive_is_confirmed() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("funding-token");
        std::fs::write(&source, "bearer token fixture").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        let journal_path = funding_token_journal::journal_path(&source).unwrap();

        let first = FundingTokenJournal::prepare(&source).unwrap();
        assert_eq!(first.token(), "bearer token fixture");
        assert!(!source.exists());
        assert_eq!(
            std::fs::read_to_string(&journal_path).unwrap(),
            "bearer token fixture"
        );
        drop(first);

        let resumed = FundingTokenJournal::prepare(&source).unwrap();
        assert_eq!(resumed.token(), "bearer token fixture");
        assert!(journal_path.exists());

        resumed.complete().unwrap();
        assert!(!journal_path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn funding_token_journal_rejects_two_possible_imports() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("funding-token");
        let journal = funding_token_journal::journal_path(&source).unwrap();
        std::fs::write(&source, "new token").unwrap();
        std::fs::write(&journal, "in-progress token").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();

        let error = FundingTokenJournal::prepare(&source)
            .err()
            .expect("two token files must be ambiguous");
        assert!(error.to_string().contains("ambiguous import"));
        assert!(source.exists());
        assert!(journal.exists());
    }

    #[test]
    #[cfg(unix)]
    fn funding_token_journal_rejects_unsafe_files() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        fn rejected(path: &Path) {
            assert!(FundingTokenJournal::prepare(path).is_err());
        }

        let dir = tempfile::tempdir().unwrap();

        let public = dir.path().join("public");
        std::fs::write(&public, "token").unwrap();
        std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o644)).unwrap();
        rejected(&public);

        let target = dir.path().join("target");
        std::fs::write(&target, "token").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        rejected(&link);

        let non_regular = dir.path().join("directory");
        std::fs::create_dir(&non_regular).unwrap();
        rejected(&non_regular);
    }

    #[test]
    #[cfg(unix)]
    fn funding_token_journal_rejects_oversized_data() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("oversized");
        std::fs::write(&source, vec![b'x'; 256 * 1024 + 1]).unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = FundingTokenJournal::prepare(&source).err().unwrap();
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    #[cfg(unix)]
    fn funding_token_journal_rejects_path_replacement_on_completion() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("funding-token");
        std::fs::write(&source, "validated token").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        let journal_path = funding_token_journal::journal_path(&source).unwrap();
        let displaced = dir.path().join("displaced");

        let journal = FundingTokenJournal::prepare(&source).unwrap();
        std::fs::rename(&journal_path, &displaced).unwrap();
        std::fs::write(&journal_path, "replacement").unwrap();
        std::fs::set_permissions(&journal_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(journal.token(), "validated token");
        let error = journal.complete().unwrap_err();
        assert!(error.to_string().contains("was replaced"));
        assert_eq!(
            std::fs::read_to_string(&journal_path).unwrap(),
            "replacement"
        );
    }

    #[tokio::test]
    async fn paid_resume_requires_complete_wallet_coordinates() {
        let payments = CliPayments::open_for_resume(
            &ResumeArgs {
                fi_spv2_account_file: None,
                payment_federation_id: None,
                wallet_data_dir: None,
                wallet_secret_file: None,
                payment_invite_code: None,
                funding_token_file: None,
            },
            None,
        )
        .await
        .unwrap();
        assert!(payments.wallet.is_none());

        let error = CliPayments::open_for_resume(
            &ResumeArgs {
                fi_spv2_account_file: None,
                payment_federation_id: Some("00".repeat(32)),
                wallet_data_dir: None,
                wallet_secret_file: None,
                payment_invite_code: None,
                funding_token_file: None,
            },
            None,
        )
        .await
        .err()
        .expect("partial paid-resume arguments must fail");
        assert!(
            error
                .to_string()
                .contains("paid resume requires --payment-federation-id")
        );

        let error = CliPayments::open_for_resume(
            &ResumeArgs {
                fi_spv2_account_file: None,
                payment_federation_id: None,
                wallet_data_dir: None,
                wallet_secret_file: None,
                payment_invite_code: Some(
                    FedimintInviteCode::new(
                        "https://guardian.example/".parse().unwrap(),
                        fedimint_core::PeerId::from(0),
                        "11".repeat(32).parse().unwrap(),
                        None,
                    )
                    .to_string(),
                ),
                funding_token_file: None,
            },
            None,
        )
        .await
        .err()
        .expect("a resume invite without wallet coordinates must fail");
        assert!(
            error
                .to_string()
                .contains("--payment-invite-code requires --payment-federation-id")
        );
    }

    #[tokio::test]
    async fn paid_resume_rejects_invite_mismatch_before_wallet_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let wallet_data_dir = dir.path().join("wallet");
        let error = CliPayments::open_for_resume(
            &ResumeArgs {
                fi_spv2_account_file: None,
                payment_federation_id: Some("22".repeat(32)),
                wallet_data_dir: Some(wallet_data_dir.clone()),
                wallet_secret_file: None,
                payment_invite_code: Some(
                    FedimintInviteCode::new(
                        "https://guardian.example/".parse().unwrap(),
                        fedimint_core::PeerId::from(0),
                        "11".repeat(32).parse().unwrap(),
                        None,
                    )
                    .to_string(),
                ),
                funding_token_file: None,
            },
            Some(WalletRootSecret(WalletSecret([42; 64]))),
        )
        .await
        .err()
        .expect("a resume invite for another federation must fail");
        assert!(error.to_string().contains("different federation"));
        assert!(!wallet_data_dir.exists());
    }

    #[test]
    fn payment_invite_parses_and_binds_to_the_selected_federation() {
        let selected: fedimint_core::config::FederationId = "11".repeat(32).parse().unwrap();
        let invite = FedimintInviteCode::new(
            "https://guardian.example/".parse().unwrap(),
            fedimint_core::PeerId::from(0),
            selected,
            None,
        )
        .to_string();

        assert!(parse_payment_invite(None, selected).unwrap().is_none());
        let accepted = parse_payment_invite(Some(&invite), selected)
            .unwrap()
            .expect("a matching invite is accepted");
        assert_eq!(accepted.federation_id(), selected);

        let other: fedimint_core::config::FederationId = "22".repeat(32).parse().unwrap();
        let error = parse_payment_invite(Some(&invite), other).unwrap_err();
        assert!(error.to_string().contains("different federation"));

        let error = parse_payment_invite(Some("not-an-invite"), selected).unwrap_err();
        assert!(format!("{error:#}").contains("parse --payment-invite-code"));
    }

    #[test]
    fn over_cap_requirements_do_not_enter_the_cli_authorization_path() {
        let requirements = fi_client::PaymentRequirements {
            authorization_id: PaymentAuthorizationId::try_from_opaque("00".repeat(32)).unwrap(),
            total_msats: 21_000,
            max_total_msats: Some(20_000),
            seats: Vec::new(),
        };
        assert!(payment_requirements_exceed_cap(&requirements));

        let exact_cap = fi_client::PaymentRequirements {
            max_total_msats: Some(21_000),
            ..requirements
        };
        assert!(!payment_requirements_exceed_cap(&exact_cap));
    }

    #[test]
    fn concrete_wallet_preflight_converts_both_mint_generations() {
        let selected = test_payment_federation(0x11);
        let v1_request = test_locked_request(128, 1);
        let v1_terms = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV1 {
                federation_id: selected.clone(),
                issuance: vec![v1_request.clone()],
            },
            RefundIssuance::MintV1 {
                refund_nonce: [1; 32],
                issuance: vec![v1_request.clone()],
            },
            128,
        );
        let (v1_amount, _) = concrete_wallet_preflight(
            &selected,
            &test_requirement(selected.clone(), 128),
            QuoteId([3; 32]),
            &v1_terms,
            None,
        )
        .unwrap();
        assert_eq!(v1_amount, 128);

        // Mint-v1 and mint-v2 use the same consensus-encoded TBS blinded
        // message, so the generated nonce is valid input to the v2 decoder.
        let v2_request = LockedIssuanceRequestV2 {
            amount_msats: 128,
            blind_nonce: v1_request.blind_nonce,
            tweak: [9; 16],
        };
        let v2_terms = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV2 {
                federation_id: selected.clone(),
                issuance: vec![v2_request.clone()],
            },
            RefundIssuance::MintV2 {
                refund_nonce: [2; 32],
                issuance: vec![v2_request],
            },
            128,
        );
        let (v2_amount, _) = concrete_wallet_preflight(
            &selected,
            &test_requirement(selected.clone(), 128),
            QuoteId([3; 32]),
            &v2_terms,
            Some(fedimint_core::core::ModuleInstanceId::from(7u16)),
        )
        .unwrap();
        assert_eq!(v2_amount, 128);
    }

    #[test]
    fn concrete_wallet_preflight_rejects_payer_refund_and_amount_mismatches() {
        let selected = test_payment_federation(0x11);
        let other = test_payment_federation(0x22);
        let request = test_locked_request(128, 2);
        let valid_terms = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV1 {
                federation_id: selected.clone(),
                issuance: vec![request.clone()],
            },
            RefundIssuance::MintV1 {
                refund_nonce: [3; 32],
                issuance: vec![request.clone()],
            },
            128,
        );

        let wrong_requirement = test_requirement(other.clone(), 128);
        assert!(
            concrete_wallet_preflight(
                &selected,
                &wrong_requirement,
                wrong_requirement.quote_id,
                &valid_terms,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("different payment federation")
        );

        let wrong_quote_payer = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV1 {
                federation_id: other,
                issuance: vec![request.clone()],
            },
            RefundIssuance::MintV1 {
                refund_nonce: [4; 32],
                issuance: vec![request.clone()],
            },
            128,
        );
        assert!(
            concrete_wallet_preflight(
                &selected,
                &test_requirement(selected.clone(), 128),
                QuoteId([3; 32]),
                &wrong_quote_payer,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("paid quote belongs")
        );

        let wrong_refund = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV1 {
                federation_id: selected.clone(),
                issuance: vec![request.clone()],
            },
            RefundIssuance::MintV2 {
                refund_nonce: [5; 32],
                issuance: Vec::new(),
            },
            128,
        );
        assert!(
            concrete_wallet_preflight(
                &selected,
                &test_requirement(selected.clone(), 128),
                QuoteId([3; 32]),
                &wrong_refund,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("refund generation mismatch")
        );

        assert!(
            concrete_wallet_preflight(
                &selected,
                &test_requirement(selected.clone(), 127),
                QuoteId([3; 32]),
                &valid_terms,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("amount differs")
        );
    }

    #[test]
    fn concrete_wallet_preflight_checks_internal_and_aggregate_overflow() {
        let selected = test_payment_federation(0x11);
        let first = test_locked_request(u64::MAX, 3);
        let second = test_locked_request(1, 4);
        let overflowing_terms = test_quote_terms(
            selected.clone(),
            PaymentTerms::MintV1 {
                federation_id: selected.clone(),
                issuance: vec![first.clone(), second.clone()],
            },
            RefundIssuance::MintV1 {
                refund_nonce: [6; 32],
                issuance: vec![first, second],
            },
            u64::MAX,
        );
        assert!(
            concrete_wallet_preflight(
                &selected,
                &test_requirement(selected.clone(), u64::MAX),
                QuoteId([3; 32]),
                &overflowing_terms,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("mint-v1 quote amount overflow")
        );
        assert!(add_payment_total(u64::MAX, 1).is_err());
        assert!(validate_aggregate_payment_total(100, 101).is_err());
        assert!(validate_aggregate_payment_total(101, 101).is_ok());
    }

    #[test]
    fn static_discovery_does_not_require_peer_badge_trust_configuration() {
        let args = RegistryQueryArgs {
            federation_size: 7,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            timeout_secs: 60,
        };
        assert!(!command_requires_peer_badge_verifier(&Command::Discover(
            args
        )));

        let preview_args = RegistryQueryArgs {
            federation_size: 7,
            fedimintd_version_minimum: None,
            fedimintd_version_maximum_exclusive: None,
            timeout_secs: 60,
        };
        assert!(command_requires_peer_badge_verifier(&Command::Preview(
            preview_args
        )));
    }

    #[test]
    fn reserve_error_mapping_only_marks_the_pre_journal_balance_proof() {
        let insufficient = anyhow::Error::new(InsufficientLockedPaymentFundsWithoutReservation)
            .context("exact aggregate payment is not ready");
        assert_eq!(
            map_reservation_error(insufficient),
            FiPaymentError::insufficient_funds_without_reservation(
                "exact aggregate payment is not ready",
            ),
        );

        let binding = anyhow::anyhow!("same reservation id belongs to another plan")
            .context("exact aggregate payment is not ready");
        assert_eq!(
            map_reservation_error(binding),
            FiPaymentError::new("exact aggregate payment is not ready"),
        );
    }
}
