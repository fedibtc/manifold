//! Operator CLI for a running Fleet Manager daemon.

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Subcommand};
use fedi_decentralized_service_fleet_manager::{FederationId, SeatId};
use fman_core::admin::{self, AdminRequest};
use tracing_subscriber::prelude::*;

#[derive(Debug, Subcommand)]
pub enum AdminVerb {
    /// Show or replace the offered plans.
    Plans {
        #[command(subcommand)]
        verb: PlansVerb,
    },
    /// Show or replace the durable seat-admission ceiling.
    Capacity {
        #[command(subcommand)]
        verb: CapacityVerb,
    },
    /// Manage setup-payment federation wallets.
    PaymentFederations {
        #[command(subcommand)]
        verb: PaymentFederationsVerb,
    },
    /// Configure the one Lightning destination used by revenue sweeps.
    Payout {
        #[command(subcommand)]
        verb: PayoutVerb,
    },
    /// Inspect seats.
    Seats {
        #[command(subcommand)]
        verb: SeatsVerb,
    },
    /// Guardian-fee revenue from the federations this fleet guards.
    GuardianFees {
        #[command(subcommand)]
        verb: GuardianFeesVerb,
    },
    /// Identity material for operator onboarding (registry listing, holder
    /// authorization).
    Onboarding,
    /// Fetch and durably retain Holder authorizations published for this FMan.
    RefreshHolderAuthorizations,
    /// Rotate the FMan-wide telemetry capability and register it immediately.
    ReenrollTelemetry,
    /// Print the root mnemonic phrase (stdout only; treat it like the
    /// fleet's master key). A full backup also needs a stopped copy of the
    /// complete data root because post-DKG guardian shares are not derived.
    /// Before starting a whole-data-root restore, remove `safe-events/` and
    /// every `seats/*/safe-events/` directory so restored cursor coordinates
    /// receive new incarnations.
    ShowMnemonic,
    /// Set this host up. A daemon started on an empty data root waits here
    /// until one of these runs (SPEC-nostr-backup-restore).
    Onboard {
        #[command(subcommand)]
        verb: OnboardVerb,
    },
}

#[derive(Debug, Subcommand)]
pub enum OnboardVerb {
    /// Become a Fleet Manager that has never existed before: generate the root
    /// mnemonic and start with no seats.
    New {
        /// Succeed instead of refusing when this host is already onboarded.
        /// For orchestrators whose want is "ensure onboarded"; an operator
        /// setting a host up should leave it off and see the refusal.
        #[arg(long)]
        if_needed: bool,
    },
    /// Become the recovery of a Fleet Manager that did exist, from its root
    /// mnemonic. Its seats are rebuilt from the encrypted documents that
    /// phrase can find and read.
    Restore {
        /// File holding the phrase and nothing else. Read once, never written
        /// back: the phrase's home from then on is this install's database.
        #[arg(long)]
        mnemonic_file: PathBuf,
        /// Acknowledge that the guardians being restored are permanently
        /// offline. Two hosts running one guardian identity equivocate, and
        /// mnemonic-only recovery makes standing up a second copy easy. No
        /// state the daemon can observe answers this, so the operator asserts
        /// it.
        #[arg(long)]
        acknowledge_original_host_is_gone: bool,
    },
    /// Finish setup after Holder authorization is observed.
    Offer {
        #[arg(long)]
        max_seats: u32,
        #[arg(long = "price-msats")]
        price_msats: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CapacityVerb {
    Show,
    Set { max_seats: u32 },
}

#[derive(Debug, Subcommand)]
pub enum PlansVerb {
    Show,
    /// Replace the offer with the given plans. With no `--price`, the fleet
    /// offers nothing.
    Set {
        /// Offer paid InfiniteBestEffort at this price, in millisatoshis.
        #[arg(long = "price-msats")]
        price_msats: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum PaymentFederationsVerb {
    /// List payment federations with wallet health and balance: the
    /// accepted common set plus wallet-only leftovers of removed members.
    List,
    /// Sweep the wallet to the configured destination.
    Sweep {
        federation_id: String,
        /// Caller-generated idempotency identity for this payout.
        #[arg(long)]
        request_id: fman_core::wallet::PayoutRequestId,
    },
}

#[derive(Debug, Subcommand)]
pub enum PayoutVerb {
    Show,
    /// Replace the destination, or clear it when omitted.
    Set {
        destination: Option<String>,
    },
    /// Read a durable payout job without starting another payout.
    Status {
        request_id: fman_core::wallet::PayoutRequestId,
    },
    /// Await the exact native operation committed for a durable payout job.
    Await {
        request_id: fman_core::wallet::PayoutRequestId,
    },
}

#[derive(Debug, Subcommand)]
pub enum SeatsVerb {
    List,
    Status {
        seat_id: SeatId,
    },
    /// Stop the seat's fedimintd and free its capacity (terminal).
    Decommission {
        seat_id: SeatId,
    },
}

#[derive(Debug, Subcommand)]
pub enum GuardianFeesVerb {
    /// The remittance account, balances, and recent remittances for one
    /// seat's federation.
    Show {
        #[command(flatten)]
        seat_id: GuardianFeeSeatIdArgs,
        /// How many recent remittances to report.
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Move everything remitted so far out of the pool. Locked deposits leave
    /// at the next cycle turnover, so re-check `show` afterwards.
    Collect {
        #[command(flatten)]
        seat_id: GuardianFeeSeatIdArgs,
    },
    /// Sweep collected ecash to the configured destination.
    Sweep {
        #[command(flatten)]
        seat_id: GuardianFeeSeatIdArgs,
        /// Caller-generated idempotency identity for this payout.
        #[arg(long)]
        request_id: fman_core::wallet::PayoutRequestId,
    },
}

/// One required seat-id input, in either the established positional form or
/// the compatibility option form.
#[derive(Debug, ClapArgs)]
#[group(required = true, multiple = false)]
pub struct GuardianFeeSeatIdArgs {
    /// Seat id, supplied positionally.
    #[arg(value_name = "SEAT_ID")]
    seat_id: Option<SeatId>,
    /// Compatibility spelling for the seat id.
    #[arg(long = "seat-id", value_name = "SEAT_ID")]
    seat_id_option: Option<SeatId>,
}

impl GuardianFeeSeatIdArgs {
    /// Returns the one seat id required by the generated Clap argument group.
    fn into_seat_id(self) -> SeatId {
        self.seat_id
            .or(self.seat_id_option)
            .expect("Clap requires exactly one guardian-fee seat ID")
    }
}

/// Initialize ordinary human-facing logs for a one-shot Admin CLI process.
fn init_admin_logging() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter);
    tracing_subscriber::registry().with(stderr).try_init()?;
    Ok(())
}

pub async fn run(data_dir: &Path, verb: AdminVerb) -> anyhow::Result<()> {
    init_admin_logging()?;
    let request = match verb {
        AdminVerb::Plans {
            verb: PlansVerb::Show,
        } => AdminRequest::ShowPlans,
        AdminVerb::Plans {
            verb: PlansVerb::Set { price_msats },
        } => AdminRequest::SetPrice { price_msats },
        AdminVerb::Capacity {
            verb: CapacityVerb::Show,
        } => AdminRequest::ShowCapacity,
        AdminVerb::Capacity {
            verb: CapacityVerb::Set { max_seats },
        } => AdminRequest::SetCapacity { max_seats },
        AdminVerb::PaymentFederations {
            verb: PaymentFederationsVerb::List,
        } => AdminRequest::ListPaymentFederations,
        AdminVerb::PaymentFederations {
            verb:
                PaymentFederationsVerb::Sweep {
                    federation_id,
                    request_id,
                },
        } => AdminRequest::SweepPaymentFees {
            federation_id: FederationId(federation_id),
            request_id,
        },
        AdminVerb::Payout {
            verb: PayoutVerb::Show,
        } => AdminRequest::PayoutDestination,
        AdminVerb::Payout {
            verb: PayoutVerb::Set { destination },
        } => AdminRequest::SetPayoutDestination { destination },
        AdminVerb::Payout {
            verb: PayoutVerb::Status { request_id },
        } => AdminRequest::PayoutStatus { request_id },
        AdminVerb::Payout {
            verb: PayoutVerb::Await { request_id },
        } => AdminRequest::AwaitPayout { request_id },
        AdminVerb::Seats {
            verb: SeatsVerb::List,
        } => AdminRequest::ListSeats,
        AdminVerb::Seats {
            verb: SeatsVerb::Status { seat_id },
        } => AdminRequest::SeatStatus { seat_id },
        AdminVerb::Seats {
            verb: SeatsVerb::Decommission { seat_id },
        } => AdminRequest::DecommissionSeat { seat_id },
        AdminVerb::GuardianFees {
            verb: GuardianFeesVerb::Show { seat_id, limit },
        } => AdminRequest::GuardianFees {
            seat_id: seat_id.into_seat_id(),
            limit,
        },
        AdminVerb::GuardianFees {
            verb: GuardianFeesVerb::Collect { seat_id },
        } => AdminRequest::CollectGuardianFees {
            seat_id: seat_id.into_seat_id(),
        },
        AdminVerb::GuardianFees {
            verb:
                GuardianFeesVerb::Sweep {
                    seat_id,
                    request_id,
                },
        } => AdminRequest::SweepGuardianFees {
            seat_id: seat_id.into_seat_id(),
            request_id,
        },
        AdminVerb::Onboarding => AdminRequest::Onboarding,
        AdminVerb::RefreshHolderAuthorizations => AdminRequest::RefreshHolderAuthorizations,
        AdminVerb::ReenrollTelemetry => AdminRequest::ReenrollTelemetry,
        AdminVerb::ShowMnemonic => AdminRequest::ShowMnemonic,
        AdminVerb::Onboard {
            verb: OnboardVerb::New { if_needed },
        } => AdminRequest::OnboardAsNew { if_needed },
        AdminVerb::Onboard {
            verb:
                OnboardVerb::Restore {
                    mnemonic_file,
                    acknowledge_original_host_is_gone,
                },
        } => AdminRequest::OnboardFromBackup {
            // Read here rather than daemon-side: the phrase is the operator's,
            // and the socket is already the channel `ShowMnemonic` returns one
            // over.
            mnemonic: tokio::fs::read_to_string(&mnemonic_file)
                .await
                .map_err(|err| anyhow::anyhow!("read the mnemonic file: {err}"))?,
            acknowledge_original_host_is_gone,
        },
        AdminVerb::Onboard {
            verb:
                OnboardVerb::Offer {
                    max_seats,
                    price_msats,
                },
        } => AdminRequest::ConfigureInitialOffer {
            max_seats,
            price_msats,
        },
    };

    match admin::request(&admin::socket_path(data_dir), &request).await? {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            if let Some(incomplete) = value.get("incomplete") {
                tracing::warn!(
                    claimed_msat = %value["claimed_msat"].as_str().unwrap_or("unknown"),
                    phase = incomplete["phase"].as_str().unwrap_or("unknown"),
                    error = incomplete["error"]
                        .as_str()
                        .unwrap_or("refresh status before retrying"),
                    "guardian-fee collection incomplete",
                );
            }
            Ok(())
        }
        // The operator reads the sentence; the discriminant beside it is for
        // the browser wizard, which has to pick a recovery action.
        Err(error) => {
            eprintln!("error: {}", error.message);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests;
