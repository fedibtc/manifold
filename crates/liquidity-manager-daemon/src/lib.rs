//! FLIP Liquidity Manager daemon runtime.

// Only three modules are `pub mod`, so a `pub` item anywhere else is reachable
// only from inside this crate. Saying `pub(crate)` instead keeps the crate's
// real API visible in the source, and lets `dead_code` see everything else.
#![warn(unreachable_pub)]

mod admin;
mod admin_token;
mod advertisement;
mod allocation_funding;
mod allocation_store;
mod attestation_store;
mod auth;
mod backup;
mod chain_observer;
mod config;
mod daemon;
mod database;
mod endpoint_policy;
mod federation_preview;
mod funds_admin;
mod gateway;
mod gateway_allocation;
pub mod holder_authorization;
mod identity;
mod manual_ops;
mod nostr;
#[cfg(feature = "embedded-operator-ui")]
mod operator_ui;
mod public;
mod recovery;
pub mod revocation;
mod secret_store;
mod setup_store;
mod stability_allocation;
mod stability_deposit;
mod stability_pool;
mod target_fedimint;
mod target_recovery;
// `cfg(test)` only, so it exists in the lib's own test build and nowhere an
// integration test can reach.
#[cfg(test)]
#[path = "../tests/support.rs"]
pub(crate) mod test_support;
pub mod trust_fixtures;
mod verification;
mod verification_budget;
mod wallet;

pub use config::{CliCommand, DaemonArgs, DaemonMode, DaemonPaths, parse_cli};
pub(crate) use daemon::DaemonContext;
pub use daemon::{Worker, run_daemon, run_restore_daemon};
pub use database::Database;
pub use federation_preview::{FederationPreview, PreviewPeer};
pub use nostr::{
    FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
    FLIP_PROVIDER_ADVERTISEMENT_HASHTAG, NostrRelayPublisher, RelayPublishRequest, RelayPublisher,
    RelayWithdrawRequest,
};
pub(crate) use secret_store::SecretStore;
pub use stability_pool::STABILITY_POOL_MODULE_KIND;

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::Address;
use bitcoin::address::NetworkUnchecked;
use fedi_decentralized_service_liquidity_manager::{
    BitcoinNetwork, Sats, ServiceError, ServiceErrorCode, ServiceResult, Timestamp,
};

use crate::wallet::domain_network_to_bitcoin;

/// Runs a periodic daemon task until shutdown. `FailedPrecondition` results
/// (setup not configured or not ready) are skipped silently so an
/// unconfigured daemon stays quiet.
///
/// Every outcome is also recorded against `worker` so a task that fails every
/// pass is visible through Admin API health. Without that, retry-forever
/// semantics make a permanently stuck worker indistinguishable from an idle
/// one, and restarting the daemon becomes the only way to probe it.
///
/// # A pass is cancellable, and callers depend on that
///
/// The loop selects `run` against the shutdown token, so **a shutdown drops
/// the pass at whatever await it is suspended on**, including one inside a
/// durable write. This is the one thing about the loop a caller has to reason
/// about, and it is stated here because the hazard is created here.
///
/// It is bounded: one event per process lifetime, resumed from durable state
/// on the next start, and not something a counterparty can aim repeatedly at a
/// chosen write. Whether a worker must do anything about it depends on what a
/// dropped write costs *that* worker, and the four callers divide cleanly:
///
/// - `Worker::GatewayObservation` and `Worker::WalletOperationSync` re-derive
///   what they were writing from a repeatable read — `gateway_info()` is a
///   plain HTTP GET, and the sync re-reads the same chain evidence. A dropped
///   write costs one re-read on the next boot.
/// - `Worker::GatewayAllocation` moves money, so it does not rely on being
///   re-derivable at all: it relies on the send fence. `ensure_wallet_operation`
///   persists the operation row, and `claim_funding_submission` moves it to
///   `in_doubt`, both before the irreversible gatewayd call. A pass dropped
///   before the fence has sent nothing; one dropped after it leaves an
///   `in_doubt` row that recovery resumes and never resubmits automatically.
///   The intermediate step writes — the deposit address among them — are a
///   cache that saves re-minting, not the thing that makes the drop safe.
/// - `Worker::StabilityPoolAllocation` cannot. Its observations come from
///   draining a Fedimint stream, so the write that *ends* an observation is
///   the only cheap record of expensive work. It routes those writes through
///   `commit_step_beyond_cancellation` and
///   `require_item_action_beyond_cancellation`
///   (`stability_allocation.rs`), which put them on a task this drop cannot
///   reach. See `STABILITY_ITEM_BUDGET` for the full argument, including why
///   the per-item budget is a consulted deadline rather than a `timeout`
///   wrapped around the item.
///
/// A new worker whose terminal write cannot be re-derived from a repeatable
/// read belongs in the second group.
/// A stable per-worker fraction of `period`, so the workers in one daemon do
/// not tick together.
///
/// Derived from the worker name and nothing else. That makes it identical
/// across a fleet, which is deliberate: this de-phases the workers *within* a
/// daemon, and does nothing for a fleet restarted together. Doing that needs a
/// per-instance value FLIP does not have at this point in startup — the data
/// directory is typically identical across containerised deployments, and the
/// provider identity is not installed yet — so it is a separate change with a
/// separate decision behind it, tracked in the FLIP open items.
///
/// Kept under one period so a worker's first scheduled pass never slips past
/// the cadence its caller asked for.
fn worker_phase_offset(worker: Worker, period: Duration) -> Duration {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    worker.to_string().hash(&mut hasher);
    // Spread over the period rather than over a fixed span: the periods here
    // differ by an order of magnitude, and a spread wider than the period
    // would delay the fast workers by whole cycles.
    //
    // The shift takes the 53 bits an f64 represents exactly, so the division
    // lands in [0, 1) without rounding to 1.0 and the offset stays under one
    // period.
    let fraction = (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64;
    period.mul_f64(fraction)
}

pub(crate) async fn run_interval_task<T, F, Fut>(
    context: DaemonContext,
    worker: Worker,
    mut period: Duration,
    failure_message: &'static str,
    run: F,
) -> anyhow::Result<()>
where
    F: Fn(DaemonContext) -> Fut,
    Fut: Future<Output = ServiceResult<T>>,
{
    // Match Fedimint's existing devimint convention: integration tests run
    // real state machines, but should not inherit production-scale polling
    // latency between each state transition.
    if std::env::var_os("DEV_DEFE_SOCKET_PATH").is_some() {
        period = period.min(Duration::from_millis(100));
    }
    let mut interval = tokio::time::interval(period);
    // The last failure this worker reported, so a condition that lasts is
    // logged once rather than once per pass. Health already carries the
    // standing state; the log carries the change to it.
    let mut last_failure: Option<String> = None;
    // `interval` fires immediately and then holds an exact fixed period, so
    // every worker in one daemon ticks together for as long as the process
    // lives. Four workers hitting SQLite, the gateway, and the relays in one
    // burst is worse than the same work spread across the period, and it stays
    // that way because nothing ever perturbs the phase.
    //
    // The first pass stays immediate — startup work should not wait — and the
    // schedule shifts once afterwards. `reset_after` is what keeps that a
    // schedule change rather than a sleep: sleeping inside the `select!` arm
    // would make shutdown unresponsive for the length of the offset.
    let mut pending_offset = Some(worker_phase_offset(worker, period));
    loop {
        tokio::select! {
            _ = context.shutdown.cancelled() => return Ok(()),
            _ = interval.tick() => {
                if let Some(offset) = pending_offset.take() {
                    interval.reset_after(period + offset);
                }
                // One pass is the unit a backup's quiescence barrier holds
                // still: a pass reads an item from SQLite, acts on a Fedimint
                // client, and writes the result back, so it is what can leave
                // the two stores at different instants. See `WorkQuiescence`.
                let _pass = context.work_quiescence.pass().await;
                match run(context.clone()).await {
                    Ok(_) => {
                        if let Some(previous) = last_failure.take() {
                            tracing::info!(
                                %worker,
                                previous_failure = %previous,
                                "worker pass succeeded again"
                            );
                        }
                        context.record_worker_success(worker).await
                    }
                    // An unconfigured or not-yet-ready deployment is neither a
                    // success nor a failure: the worker had nothing it was
                    // allowed to do, and setup status already reports that.
                    Err(error) if error.code() == ServiceErrorCode::FailedPrecondition => {}
                    Err(error) => {
                        let message = error.to_string();
                        // A dependency that stays down makes every worker fail
                        // every pass. Warning each time would bury whatever
                        // else happened; the repeats stay at debug, and a
                        // different error warns again because it is new.
                        if last_failure.as_deref() == Some(message.as_str()) {
                            tracing::debug!(?error, "{failure_message}");
                        } else {
                            tracing::warn!(?error, "{failure_message}");
                        }
                        last_failure = Some(message.clone());
                        context.record_worker_failure(worker, message).await;
                    }
                }
            }
        }
    }
}

pub(crate) fn now_timestamp() -> Timestamp {
    Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

pub(crate) fn to_i64_sats(amount: Sats) -> ServiceResult<i64> {
    i64::try_from(amount.0).map_err(|_| internal_error("sats amount exceeds SQLite i64 range"))
}

pub(crate) fn checked_sats_add(left: Sats, right: Sats) -> ServiceResult<Sats> {
    left.0
        .checked_add(right.0)
        .map(Sats)
        .ok_or_else(|| internal_error("sats addition overflow"))
}

pub(crate) fn checked_sum(mut amounts: impl Iterator<Item = Sats>) -> ServiceResult<Sats> {
    amounts
        .try_fold(0_u64, |acc, amount| acc.checked_add(amount.0))
        .map(Sats)
        .ok_or_else(|| internal_error("sats sum overflow"))
}

pub(crate) fn validate_deposit_address(
    address: &str,
    network: BitcoinNetwork,
) -> Result<(), String> {
    let address = address
        .parse::<Address<NetworkUnchecked>>()
        .map_err(|error| error.to_string())?;
    address
        .require_network(domain_network_to_bitcoin(network))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn internal_error(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::Internal, error.to_string())
}

pub(crate) fn invalid_argument(message: impl std::fmt::Display) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::InvalidArgument, message.to_string())
}

pub(crate) fn failed_precondition(message: impl Into<String>) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::FailedPrecondition, message)
}

pub(crate) fn not_found(message: impl Into<String>) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::NotFound, message)
}

pub(crate) fn permission_denied(message: impl Into<String>) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::PermissionDenied, message)
}

pub(crate) fn unavailable(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::with_code(ServiceErrorCode::Unavailable, error.to_string())
}

#[cfg(test)]
mod phase_offset_tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// The offset stays inside one period.
    ///
    /// It is added to a whole period before the next tick, so an offset that
    /// could reach or exceed the period would push a worker's cadence past what
    /// its caller asked for — a 10 s worker running every 20 s.
    #[test]
    fn an_offset_is_under_one_period() {
        for worker in Worker::iter() {
            for period in [
                Duration::from_millis(100),
                Duration::from_secs(10),
                Duration::from_secs(30),
                Duration::from_secs(60),
            ] {
                let offset = worker_phase_offset(worker, period);
                assert!(
                    offset < period,
                    "{worker} offset {offset:?} is not under period {period:?}"
                );
            }
        }
    }

    /// Workers sharing a period land on different phases.
    ///
    /// This is the whole point: `GatewayAllocation` and `StabilityPoolAllocation`
    /// both run every 10 s, and both hitting SQLite and their dependencies in
    /// the same instant is the burst being spread.
    #[test]
    fn workers_sharing_a_period_do_not_share_a_phase() {
        let period = Duration::from_secs(10);
        let offsets: Vec<Duration> = Worker::iter()
            .map(|worker| worker_phase_offset(worker, period))
            .collect();
        let mut distinct = offsets.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            offsets.len(),
            "two workers share a phase: {offsets:?}"
        );
    }

    /// The offset is a function of the worker name, so a restart puts a worker
    /// back on the phase it had. Nothing here reads a clock or a random source.
    #[test]
    fn an_offset_is_stable_across_calls() {
        let period = Duration::from_secs(30);
        for worker in Worker::iter() {
            assert_eq!(
                worker_phase_offset(worker, period),
                worker_phase_offset(worker, period)
            );
        }
    }
}
