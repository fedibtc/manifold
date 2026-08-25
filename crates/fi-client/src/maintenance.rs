//! Typed, threshold-live post-formation metadata maintenance.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_nostr_clients::FiNostrClient;
use fedi_decentralized_service_fleet_manager::{
    FederationMetadataUpdate, FiId, FleetManagerError, FleetManagerService, GatewayApiUrl,
    MetaConsensusBase, MetaFieldKey, MetaFieldValue, RegisterGatewayRequest,
};
use fedimint_core::runtime::Instant;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::formation::{
    DriverRun, FormationRunOptions, FormationRunOptionsConfig, FormationTimingField,
    InvalidFormationRunOptions, MetaFieldSubmission, MetaFieldSubmissionError, SeatSession,
    finish_driver_run, meta_field_matches, sleep_for_retry, snapshot_meta_consensus,
    start_driver_run, validate_consensus_metadata_size,
};
use crate::{
    FederationConsensusError, FederationConsensusReader, FederationConsensusSnapshot, FiClient,
    FiError, FiIdentity, FiPayments, FiResult, FleetManagerConnector, FormationPhase,
};

const MAINTENANCE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
/// Sanitized diagnosis for a guardian that pinned the active consensus base
/// to a different admitted target. Distinguishable from a stale base so an
/// app can stop expecting same-base retries to help.
const MAINTENANCE_TARGET_CONFLICT: &str = "guardian already admitted a different metadata target for this consensus base; retrying \
     cannot help until the conflicting write converges or the guardian's operator restarts it";

/// One timing field in [`MaintenanceRunOptionsConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceTimingField {
    /// Delay between convergence/readback attempts.
    PollInterval,
    /// Maximum elapsed time for one maintenance invocation.
    RunTimeout,
    /// Maximum time for one consensus, connection, signing, or RPC call.
    RequestTimeout,
}

impl std::fmt::Display for MaintenanceTimingField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PollInterval => "poll interval",
            Self::RunTimeout => "run timeout",
            Self::RequestTimeout => "request timeout",
        })
    }
}

/// Reason maintenance timing options could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidMaintenanceRunOptions {
    /// A timer would truncate below the shared runtime quantum.
    #[error("invalid maintenance options: {field} must be at least one millisecond")]
    BelowMinimum {
        /// Invalid configuration field.
        field: MaintenanceTimingField,
    },
    /// A timer exceeds the shared native/WASM representation.
    #[error("invalid maintenance options: {field} exceeds the runtime timer range")]
    AboveMaximum {
        /// Invalid configuration field.
        field: MaintenanceTimingField,
    },
    /// A duration would be truncated by the WASM millisecond timer.
    #[error("invalid maintenance options: {field} must be an integral millisecond value")]
    NonIntegral {
        /// Invalid configuration field.
        field: MaintenanceTimingField,
    },
    /// A derived durable lease duration overflowed.
    #[error("invalid maintenance options: deadline is too large")]
    LeaseOverflow,
    /// A runtime monotonic deadline could not represent the value.
    #[error("invalid maintenance options: {field} is too large")]
    DeadlineOverflow {
        /// Invalid configuration field.
        field: MaintenanceTimingField,
    },
}

/// Named inputs for checked metadata-maintenance timing bounds.
pub struct MaintenanceRunOptionsConfig {
    /// Delay between convergence/readback attempts.
    pub poll_interval: Duration,
    /// Maximum elapsed time for one maintenance invocation.
    pub run_timeout: Duration,
    /// Maximum time for one consensus, connection, signing, or RPC call.
    pub request_timeout: Duration,
}

impl Default for MaintenanceRunOptionsConfig {
    fn default() -> Self {
        let defaults = FormationRunOptionsConfig::default();
        Self {
            poll_interval: defaults.poll_interval,
            run_timeout: defaults.run_timeout,
            request_timeout: defaults.request_timeout,
        }
    }
}

/// Timing and deadline policy for one metadata-maintenance invocation.
#[derive(Clone, Copy, Debug)]
pub struct MaintenanceRunOptions(FormationRunOptions);

impl MaintenanceRunOptions {
    /// Construct timing options valid on native and WASM runtimes.
    pub fn new(config: MaintenanceRunOptionsConfig) -> Result<Self, InvalidMaintenanceRunOptions> {
        FormationRunOptions::new(FormationRunOptionsConfig {
            poll_interval: config.poll_interval,
            run_timeout: config.run_timeout,
            request_timeout: config.request_timeout,
        })
        .map(Self)
        .map_err(InvalidMaintenanceRunOptions::from)
    }

    #[cfg(test)]
    pub(crate) fn lease_duration(self) -> Duration {
        self.0.lease_duration()
    }

    #[cfg(test)]
    pub(crate) fn lease_renewal_duration(self) -> Duration {
        self.0.lease_renewal_duration()
    }
}

impl Default for MaintenanceRunOptions {
    fn default() -> Self {
        Self::new(MaintenanceRunOptionsConfig::default())
            .expect("default maintenance timings are valid")
    }
}

impl From<InvalidFormationRunOptions> for InvalidMaintenanceRunOptions {
    fn from(error: InvalidFormationRunOptions) -> Self {
        fn field(field: FormationTimingField) -> MaintenanceTimingField {
            match field {
                FormationTimingField::PollInterval => MaintenanceTimingField::PollInterval,
                FormationTimingField::RunTimeout => MaintenanceTimingField::RunTimeout,
                FormationTimingField::RequestTimeout => MaintenanceTimingField::RequestTimeout,
            }
        }

        match error {
            InvalidFormationRunOptions::BelowMinimum { field: invalid } => Self::BelowMinimum {
                field: field(invalid),
            },
            InvalidFormationRunOptions::AboveMaximum { field: invalid } => Self::AboveMaximum {
                field: field(invalid),
            },
            InvalidFormationRunOptions::NonIntegral { field: invalid } => Self::NonIntegral {
                field: field(invalid),
            },
            InvalidFormationRunOptions::LeaseOverflow => Self::LeaseOverflow,
            InvalidFormationRunOptions::DeadlineOverflow { field: invalid } => {
                Self::DeadlineOverflow {
                    field: field(invalid),
                }
            }
        }
    }
}

enum MaintenanceConnection<C> {
    Connected(SeatSession<C>),
    Retryable { index: u16, message: String },
}

impl<I, P, N, F, C> FiClient<I, P, N, F, C>
where
    I: FiIdentity,
    P: FiPayments,
    N: FiNostrClient,
    F: FleetManagerConnector,
    C: FederationConsensusReader,
{
    /// Ask every formed-federation guardian to store an LNv2 gateway URL.
    ///
    /// Calls are non-short-circuiting and exact retries are idempotent. The
    /// operation succeeds once the federation's consensus threshold of
    /// guardians acknowledged their local insertion; unavailable or
    /// decommissioned minority seats do not make a usable gateway impossible.
    pub async fn register_gateway(
        &self,
        gateway_api: GatewayApiUrl,
        options: MaintenanceRunOptions,
    ) -> FiResult<()> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let options = options.0;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self.register_gateway_pinned(gateway_api, fi_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn register_gateway_pinned(
        &self,
        gateway_api: GatewayApiUrl,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let recovery = self.active_recovery(fi_id).await?;
        if recovery.snapshot.phase != FormationPhase::Formed {
            return Err(FiError::MaintenanceWrongState {
                phase: recovery.snapshot.phase,
            });
        }
        let invite = recovery.snapshot.invite_code.clone().ok_or_else(|| {
            FiError::Storage("formed FI record contains no persisted invite".to_owned())
        })?;
        let snapshot = self
            .read_metadata_consensus(&invite, run)
            .await?
            .map_err(|error| FiError::MaintenanceConvergence {
                unresolved: recovery
                    .seats
                    .iter()
                    .map(|seat| seat.progress.index)
                    .collect(),
                guardian_errors: Vec::new(),
                consensus_error: Some(error.to_string()),
            })?;
        let federation =
            fedi_decentralized_domain::federation_seats(&snapshot.config).map_err(|error| {
                FiError::InvalidFleetManagers(format!(
                    "previewed federation config is not usable: {error}"
                ))
            })?;
        let threshold = usize::try_from(federation.consensus_threshold())
            .map_err(|_| FiError::Storage("federation threshold does not fit usize".to_owned()))?;
        let all = recovery
            .seats
            .iter()
            .map(|seat| seat.progress.index)
            .collect::<BTreeSet<_>>();
        let mut accepted = BTreeSet::new();
        let mut terminal: BTreeMap<u16, FleetManagerError> = BTreeMap::new();
        let mut guardian_errors = BTreeMap::new();
        let mut consensus_error = None;
        let mut retry_delay = run.poll_interval();

        loop {
            if accepted.len() >= threshold {
                match run
                    .call("reading the LNv2 gateway view", || {
                        Ok(self
                            .inner
                            .ports
                            .consensus_reader
                            .read_lnv2_gateways(&invite))
                    })
                    .await
                {
                    Ok(Ok(gateways)) if gateways.contains(&gateway_api) => return Ok(()),
                    Ok(Ok(_)) => {
                        consensus_error = Some(
                            "registered gateway is absent from the fresh LNv2 view".to_owned(),
                        );
                    }
                    Ok(Err(error)) => consensus_error = Some(error.to_string()),
                    Err(FiError::Timeout(message)) => consensus_error = Some(message),
                    Err(error) => return Err(error),
                }
            }
            let unresolved = all
                .difference(&accepted)
                .filter(|index| !terminal.contains_key(*index))
                .copied()
                .collect::<BTreeSet<_>>();
            if accepted.len() + unresolved.len() < threshold {
                let (&index, reason) = terminal
                    .iter()
                    .next()
                    .expect("threshold became impossible only after a refusal");
                return Err(FiError::MaintenanceRejected {
                    index,
                    reason: reason.clone(),
                });
            }

            let mut sessions = Vec::with_capacity(unresolved.len());
            for outcome in self
                .connect_unresolved_formed_sessions(&recovery, &unresolved, run)
                .await?
            {
                match outcome {
                    MaintenanceConnection::Connected(session) => sessions.push(session),
                    MaintenanceConnection::Retryable { index, message } => {
                        guardian_errors.insert(index, message);
                    }
                }
            }

            let mut pending = FuturesUnordered::new();
            for session in &sessions {
                let gateway_api = gateway_api.clone();
                pending.push(async move {
                    let result = async {
                        let request = RegisterGatewayRequest {
                            ts: crate::Timestamp(crate::formation::now_secs()?),
                            fi_id,
                            seat_id: session.seat_id.clone(),
                            gateway_api,
                        };
                        let request = run
                            .construct("signing RegisterGateway request", || self.sign(&request))
                            .await?;
                        run.call("registering LNv2 gateway", || {
                            Ok(session.client.register_gateway(request))
                        })
                        .await
                    }
                    .await;
                    (session.index, result)
                });
            }
            while let Some((index, result)) = pending.next().await {
                match result {
                    Ok(Ok(_)) => {
                        accepted.insert(index);
                        guardian_errors.remove(&index);
                    }
                    Ok(Err(FleetManagerError::SeatUnavailable)) => {
                        guardian_errors
                            .insert(index, FleetManagerError::SeatUnavailable.to_string());
                    }
                    Ok(Err(error)) => {
                        terminal.insert(index, error);
                    }
                    Err(FiError::Timeout(message)) => {
                        guardian_errors.insert(index, message);
                    }
                    Err(error) => return Err(error),
                }
            }
            let unresolved = all
                .difference(&accepted)
                .filter(|index| !terminal.contains_key(*index))
                .copied()
                .collect::<BTreeSet<_>>();
            sleep_for_maintenance_retry(
                run,
                &mut retry_delay,
                &unresolved,
                &guardian_errors,
                consensus_error.clone(),
            )
            .await?;
        }
    }

    /// Propose one supported metadata change through threshold-live guardians.
    ///
    /// Values are constructed with the shared Guardianito-compatible protocol
    /// types before this method can acquire a lease or touch the network. The
    /// FMan repeats the same validation and remains the authoritative
    /// authorization boundary. This operation first reads live consensus, so
    /// an already-adopted update succeeds even when every FMan is offline.
    /// Otherwise it binds one best-effort, non-short-circuiting submission wave
    /// to that exact whole-object base and reads consensus back after every
    /// partial wave.
    ///
    /// Acknowledged seats are retained for that base: retries reconnect and
    /// resubmit only unresolved rows, with bounded exponential backoff. A new
    /// consensus base resets the row set because every guardian must sign the
    /// newly rebased mutation. This prevents an unavailable minority from
    /// amplifying already accepted near-limit metadata on every poll.
    ///
    /// An FMan acknowledgement is only one guardian vote. Success means a
    /// fresh consensus read contains the exact requested value. Retryable
    /// transport, temporary-seat, stale-base, and consensus-read failures are
    /// retained until the bounded run ends in [`FiError::MaintenanceConvergence`].
    /// A guardian answering `MetaTargetConflict` has pinned the active base to
    /// a different admitted target: that seat receives no further same-base
    /// submissions (retrying is refused work until the conflicting write
    /// converges or the guardian restarts) and its distinguishable diagnosis
    /// is retained in the convergence result. Typed policy/lifecycle refusals
    /// return [`FiError::MaintenanceRejected`], and a non-formed durable
    /// record returns [`FiError::MaintenanceWrongState`] before connector
    /// work.
    pub async fn update_federation_metadata(
        &self,
        update: FederationMetadataUpdate,
        options: MaintenanceRunOptions,
    ) -> FiResult<()> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let options = options.0;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let (key, value) = update.into_field();
        let result = self.update_meta_field_pinned(key, value, fi_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn update_meta_field_pinned(
        &self,
        key: MetaFieldKey,
        value: MetaFieldValue,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let recovery = self.active_recovery(fi_id).await?;
        if recovery.snapshot.phase != FormationPhase::Formed {
            return Err(FiError::MaintenanceWrongState {
                phase: recovery.snapshot.phase,
            });
        }
        let invite = recovery.snapshot.invite_code.clone().ok_or_else(|| {
            FiError::Storage("formed FI record contains no persisted invite".to_owned())
        })?;
        let all_indices = recovery
            .seats
            .iter()
            .map(|seat| seat.progress.index)
            .collect::<BTreeSet<_>>();
        let mut active_base = None;
        let mut unresolved = all_indices.clone();
        let mut guardian_errors = BTreeMap::new();
        // Seats that answered `MetaTargetConflict` for the active base: their
        // live target pin names a different value, so same-base retries
        // are refused work. They stay unresolved (the diagnosis survives into
        // the convergence result) but receive no further submissions until a
        // fresh base clears the pin.
        let mut pinned_conflicts: BTreeSet<u16> = BTreeSet::new();
        let mut consensus_error = None;
        let mut retry_delay = run.poll_interval();

        loop {
            if Instant::now() >= run.deadline() {
                return Err(maintenance_convergence(
                    &unresolved,
                    &guardian_errors,
                    consensus_error,
                ));
            }
            let snapshot = match self.read_metadata_consensus(&invite, run).await {
                Ok(Ok(snapshot)) => snapshot,
                Ok(Err(error)) => {
                    consensus_error = Some(error.to_string());
                    sleep_for_maintenance_retry(
                        run,
                        &mut retry_delay,
                        &unresolved,
                        &guardian_errors,
                        consensus_error.clone(),
                    )
                    .await?;
                    continue;
                }
                Err(FiError::Timeout(message)) => {
                    consensus_error = Some(message);
                    sleep_for_maintenance_retry(
                        run,
                        &mut retry_delay,
                        &unresolved,
                        &guardian_errors,
                        consensus_error.clone(),
                    )
                    .await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            validate_consensus_metadata_size(snapshot.meta_value.as_deref()).map_err(|error| {
                FiError::MaintenanceConsensusTooLarge {
                    actual_bytes: error.actual_bytes,
                    max_bytes: error.max_bytes,
                }
            })?;
            if meta_field_matches(snapshot.meta_value.as_deref(), &key, &value)? {
                return Ok(());
            }

            let expected_base = MetaConsensusBase::from_consensus(
                snapshot_meta_consensus(&snapshot)
                    .map_err(|reason| FiError::MaintenanceConsensusInvalid { reason })?,
            );
            if active_base != Some(expected_base) {
                active_base = Some(expected_base);
                unresolved.clone_from(&all_indices);
                guardian_errors.clear();
                pinned_conflicts.clear();
                retry_delay = run.poll_interval();
            }

            let submit_rows = unresolved
                .difference(&pinned_conflicts)
                .copied()
                .collect::<BTreeSet<_>>();
            let mut sessions = Vec::with_capacity(submit_rows.len());
            for outcome in self
                .connect_unresolved_formed_sessions(&recovery, &submit_rows, run)
                .await?
            {
                match outcome {
                    MaintenanceConnection::Connected(session) => sessions.push(session),
                    MaintenanceConnection::Retryable { index, message } => {
                        guardian_errors.insert(index, message);
                    }
                }
            }

            let mut terminal_rejections = BTreeMap::new();
            for (index, result) in self
                .submit_meta_field_wave(&sessions, fi_id, expected_base, &key, &value, run)
                .await
            {
                match result {
                    Ok(MetaFieldSubmission::Accepted) => {
                        unresolved.remove(&index);
                        guardian_errors.remove(&index);
                    }
                    Ok(MetaFieldSubmission::BaseChanged) => {
                        guardian_errors.insert(index, "consensus metadata base changed".to_owned());
                    }
                    // A lost or hung response is ambiguous, never proof of
                    // refusal: it surfaces as the request timeout here (or as
                    // a connector-layer retryable outcome before the wave).
                    // A serialized `FleetManagerError` can never prove local
                    // transport failure (see `FleetManagerCallError` in
                    // `ports.rs`), so everything below is a wire-typed answer.
                    Err(MetaFieldSubmissionError::Driver(FiError::Timeout(message))) => {
                        guardian_errors.insert(index, message);
                    }
                    Err(MetaFieldSubmissionError::Driver(error)) => return Err(error),
                    Err(MetaFieldSubmissionError::FleetManager(
                        FleetManagerError::SeatUnavailable,
                    )) => {
                        guardian_errors
                            .insert(index, FleetManagerError::SeatUnavailable.to_string());
                    }
                    // Not retryable for this base and not a policy refusal of
                    // the value either: the guardian pinned this base to a
                    // different admitted target. Stop submitting to that seat
                    // and keep polling — the conflicting write converging (a
                    // fresh base) or an operator restart is what clears it.
                    Err(MetaFieldSubmissionError::FleetManager(
                        FleetManagerError::MetaTargetConflict,
                    )) => {
                        pinned_conflicts.insert(index);
                        guardian_errors.insert(index, MAINTENANCE_TARGET_CONFLICT.to_owned());
                    }
                    Err(MetaFieldSubmissionError::FleetManager(error)) => {
                        terminal_rejections.insert(index, error);
                    }
                }
            }

            // Read back immediately even when some connects/submissions
            // failed. A threshold may already have adopted the value while a
            // late sibling returned a terminal refusal or timeout.
            match self.read_metadata_consensus(&invite, run).await {
                Ok(Ok(snapshot)) => {
                    consensus_error = None;
                    validate_consensus_metadata_size(snapshot.meta_value.as_deref()).map_err(
                        |error| FiError::MaintenanceConsensusTooLarge {
                            actual_bytes: error.actual_bytes,
                            max_bytes: error.max_bytes,
                        },
                    )?;
                    if meta_field_matches(snapshot.meta_value.as_deref(), &key, &value)? {
                        return Ok(());
                    }
                    let observed_base = MetaConsensusBase::from_consensus(
                        snapshot_meta_consensus(&snapshot)
                            .map_err(|reason| FiError::MaintenanceConsensusInvalid { reason })?,
                    );
                    if observed_base != expected_base {
                        active_base = None;
                        continue;
                    }
                }
                Ok(Err(error)) => consensus_error = Some(error.to_string()),
                Err(FiError::Timeout(message)) => consensus_error = Some(message),
                Err(error) => return Err(error),
            }

            if let Some((index, reason)) = terminal_rejections.into_iter().next() {
                return Err(FiError::MaintenanceRejected { index, reason });
            }
            sleep_for_maintenance_retry(
                run,
                &mut retry_delay,
                &unresolved,
                &guardian_errors,
                consensus_error.clone(),
            )
            .await?;
        }
    }

    pub(crate) async fn read_metadata_consensus(
        &self,
        invite: &fedi_decentralized_service_fleet_manager::InviteCode,
        run: DriverRun<'_>,
    ) -> FiResult<Result<FederationConsensusSnapshot, FederationConsensusError>> {
        run.call("reading federation consensus metadata", || {
            Ok(self.inner.ports.consensus_reader.read_consensus(invite))
        })
        .await
    }

    async fn connect_unresolved_formed_sessions(
        &self,
        recovery: &crate::db::ActiveFormationRecovery,
        unresolved: &BTreeSet<u16>,
        run: DriverRun<'_>,
    ) -> FiResult<Vec<MaintenanceConnection<F::Client>>> {
        let mut pending = FuturesUnordered::new();
        for seat in &recovery.seats {
            if !unresolved.contains(&seat.progress.index) {
                continue;
            }
            let index = seat.progress.index;
            let seat_id = seat.progress.seat_id.clone().ok_or_else(|| {
                FiError::Storage(format!("formed FI seat row {index} has no seat id"))
            })?;
            let locator = seat.progress.locator.clone();
            pending.push(async move {
                let result = run
                    .call("reconnecting to formed Fleet Manager", || {
                        Ok(self.inner.ports.fman_connector.connect(&locator))
                    })
                    .await;
                (index, seat_id, result)
            });
        }

        let mut outcomes = Vec::with_capacity(unresolved.len());
        while let Some((index, seat_id, result)) = pending.next().await {
            match result {
                Ok(Ok(client)) => outcomes.push(MaintenanceConnection::Connected(SeatSession {
                    index,
                    client,
                    seat_id,
                })),
                Ok(Err(error)) => outcomes.push(MaintenanceConnection::Retryable {
                    index,
                    message: error.to_string(),
                }),
                Err(FiError::Timeout(message)) => {
                    outcomes.push(MaintenanceConnection::Retryable { index, message });
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcomes)
    }
}

fn maintenance_convergence(
    unresolved: &BTreeSet<u16>,
    guardian_errors: &BTreeMap<u16, String>,
    consensus_error: Option<String>,
) -> FiError {
    FiError::MaintenanceConvergence {
        unresolved: unresolved.iter().copied().collect(),
        guardian_errors: guardian_errors
            .iter()
            .map(|(index, message)| (*index, message.clone()))
            .collect(),
        consensus_error,
    }
}

async fn sleep_for_maintenance_retry(
    run: DriverRun<'_>,
    delay: &mut Duration,
    unresolved: &BTreeSet<u16>,
    guardian_errors: &BTreeMap<u16, String>,
    consensus_error: Option<String>,
) -> FiResult<()> {
    if sleep_for_retry(run.deadline(), *delay).await.is_err() {
        return Err(maintenance_convergence(
            unresolved,
            guardian_errors,
            consensus_error,
        ));
    }
    // The public poll interval is a lower bound selected by the caller. The
    // private five-second value only caps exponential growth when that would
    // not silently make a larger caller-selected interval more aggressive.
    *delay = next_maintenance_retry_delay(run.poll_interval(), *delay);
    Ok(())
}

fn next_maintenance_retry_delay(configured_poll_interval: Duration, delay: Duration) -> Duration {
    let maximum_delay = configured_poll_interval.max(MAINTENANCE_RETRY_MAX_DELAY);
    delay.saturating_mul(2).min(maximum_delay)
}

#[cfg(test)]
pub(crate) fn first_three_maintenance_retry_delays(
    configured_poll_interval: Duration,
) -> [Duration; 3] {
    let first = configured_poll_interval;
    let second = next_maintenance_retry_delay(configured_poll_interval, first);
    [
        first,
        second,
        next_maintenance_retry_delay(configured_poll_interval, second),
    ]
}
