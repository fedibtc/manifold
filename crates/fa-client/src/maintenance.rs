//! Typed, threshold-live post-formation metadata maintenance.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationMetadataUpdate, GatewayApiUrl, ManifoldGuardianError, ManifoldGuardianService,
    MetaConsensusBase, MetaField, RegisterGatewayRequest,
};
use fedimint_core::runtime::Instant;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::formation::{
    DriverRun, FormationRunOptions, FormationRunOptionsConfig, FormationTimingField,
    InvalidFormationRunOptions, MetaFieldSubmission, MetaFieldSubmissionError, SeatSession,
    finish_driver_run, meta_fields_match, sleep_for_retry, snapshot_meta_consensus,
    start_driver_run, validate_consensus_metadata_size,
};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FaStatus, FederationConsensusError,
    FederationConsensusReader, FederationConsensusSnapshot, FormationFreshness, FormationPhase,
    ManifoldGuardianConnector,
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

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
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
    ) -> FaResult<()> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let options = options.0;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self.register_gateway_pinned(gateway_api, fa_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn register_gateway_pinned(
        &self,
        gateway_api: GatewayApiUrl,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let authority = self.maintenance_authority(fa_id).await?;
        let invite = self.consensus_invite(&authority.invite_code).await;
        let snapshot = self
            .read_metadata_consensus(&invite, run)
            .await?
            .map_err(|error| FaError::MaintenanceConvergence {
                unresolved: authority.seats.iter().map(|seat| seat.index).collect(),
                guardian_errors: Vec::new(),
                consensus_error: Some(error.to_string()),
            })?;
        let federation =
            fedi_decentralized_domain::federation_seats(&snapshot.config).map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "previewed federation config is not usable: {error}"
                ))
            })?;
        let threshold = usize::try_from(federation.consensus_threshold())
            .map_err(|_| FaError::Storage("federation threshold does not fit usize".to_owned()))?;
        let all = authority
            .seats
            .iter()
            .map(|seat| seat.index)
            .collect::<BTreeSet<_>>();
        let mut accepted = BTreeSet::new();
        let mut terminal: BTreeMap<u16, ManifoldGuardianError> = BTreeMap::new();
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
                    Err(FaError::Timeout(message)) => consensus_error = Some(message),
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
                return Err(FaError::MaintenanceRejected {
                    index,
                    reason: reason.clone(),
                });
            }

            let mut sessions = Vec::with_capacity(unresolved.len());
            for outcome in self
                .connect_unresolved_formed_sessions(&authority, &unresolved, run)
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
                            fa_id,
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
                    Ok(Err(ManifoldGuardianError::SeatUnavailable)) => {
                        guardian_errors
                            .insert(index, ManifoldGuardianError::SeatUnavailable.to_string());
                    }
                    Ok(Err(error)) => {
                        terminal.insert(index, error);
                    }
                    Err(FaError::Timeout(message)) => {
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
    /// Values are constructed with the shared validated protocol
    /// types before this method can acquire a lease or touch the network. The
    /// MG repeats the same validation and remains the authoritative
    /// authorization boundary. This operation first reads live consensus, so
    /// an already-adopted update succeeds even when every MG is offline.
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
    /// An MG acknowledgement is only one guardian vote. Success means a
    /// fresh consensus read contains the exact requested value. Retryable
    /// transport, temporary-seat, stale-base, and consensus-read failures are
    /// retained until the bounded run ends in [`FaError::MaintenanceConvergence`].
    /// A guardian answering `MetaTargetConflict` has pinned the active base to
    /// a different admitted target: that seat receives no further same-base
    /// submissions (retrying is refused work until the conflicting write
    /// converges or the guardian restarts) and its distinguishable diagnosis
    /// is retained in the convergence result. Typed policy/lifecycle refusals
    /// return [`FaError::MaintenanceRejected`]. A `MetaKeyRefused` response
    /// stops retries to that seat, but returns [`FaError::MaintenanceUnsupported`]
    /// only when too few other seats remain to reach the threshold. A non-formed durable
    /// record returns [`FaError::MaintenanceWrongState`] before connector
    /// work.
    pub async fn update_federation_metadata(
        &self,
        update: FederationMetadataUpdate,
        options: MaintenanceRunOptions,
    ) -> FaResult<()> {
        self.update_federation_metadata_batch(vec![update], options)
            .await
    }

    /// Propose several metadata changes as one guardian vote per seat.
    ///
    /// Same protocol, retry, and error contract as
    /// [`Self::update_federation_metadata`], with every field bound to the
    /// same consensus base. Success means a fresh consensus read contains
    /// every requested value. An empty list or two updates for the same key
    /// return [`FaError::InvalidIntent`] before any driver work.
    ///
    /// More than one update is sent as `SetMetaFields`, which MGs released
    /// before that verb cannot answer; such a seat's reply surfaces as a
    /// [`FaError::MaintenanceRejected`]. A one-item list uses the
    /// single-field verb.
    pub async fn update_federation_metadata_batch(
        &self,
        updates: Vec<FederationMetadataUpdate>,
        options: MaintenanceRunOptions,
    ) -> FaResult<()> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        if updates.is_empty() {
            return Err(FaError::InvalidIntent(
                "metadata batch must contain at least one update".to_owned(),
            ));
        }
        let fields = updates
            .into_iter()
            .map(|update| {
                let (key, value) = update.into_field();
                MetaField { key, value }
            })
            .collect::<Vec<_>>();
        let mut keys = BTreeSet::new();
        for MetaField { key, .. } in &fields {
            if !keys.insert(key) {
                return Err(FaError::InvalidIntent(format!(
                    "metadata batch updates {} more than once",
                    key.0
                )));
            }
        }
        let options = options.0;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self.update_meta_fields_pinned(&fields, fa_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn update_meta_fields_pinned(
        &self,
        fields: &[MetaField],
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let authority = self.maintenance_authority(fa_id).await?;
        let invite = self.consensus_invite(&authority.invite_code).await;
        let all_indices = authority
            .seats
            .iter()
            .map(|seat| seat.index)
            .collect::<BTreeSet<_>>();
        let mut active_base = None;
        let mut unresolved = all_indices.clone();
        let mut guardian_errors = BTreeMap::new();
        // Key support does not depend on the metadata base. Retry these seats
        // only on a later invocation, after their operators may have upgraded.
        let mut unsupported = BTreeSet::new();
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
                Err(FaError::Timeout(message)) => {
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
                FaError::MaintenanceConsensusTooLarge {
                    actual_bytes: error.actual_bytes,
                    max_bytes: error.max_bytes,
                }
            })?;
            if meta_fields_match(snapshot.meta_value.as_deref(), fields)? {
                return Ok(());
            }

            let federation = fedi_decentralized_domain::federation_seats(&snapshot.config)
                .map_err(|error| {
                    FaError::InvalidManifoldGuardians(format!(
                        "previewed federation config is not usable: {error}"
                    ))
                })?;
            let threshold = usize::try_from(federation.consensus_threshold()).map_err(|_| {
                FaError::Storage("federation threshold does not fit usize".to_owned())
            })?;

            let expected_base = MetaConsensusBase::from_consensus(
                snapshot_meta_consensus(&snapshot)
                    .map_err(|reason| FaError::MaintenanceConsensusInvalid { reason })?,
            );
            if active_base != Some(expected_base) {
                active_base = Some(expected_base);
                unresolved.clone_from(&all_indices);
                guardian_errors.retain(|index, _| unsupported.contains(index));
                pinned_conflicts.clear();
                retry_delay = run.poll_interval();
            }

            let submit_rows = unresolved
                .difference(&pinned_conflicts)
                .filter(|index| !unsupported.contains(*index))
                .copied()
                .collect::<BTreeSet<_>>();
            let mut sessions = Vec::with_capacity(submit_rows.len());
            for outcome in self
                .connect_unresolved_formed_sessions(&authority, &submit_rows, run)
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
                .submit_meta_field_wave(&sessions, fa_id, expected_base, fields, run)
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
                    // A serialized `ManifoldGuardianError` can never prove local
                    // transport failure (see `ManifoldGuardianCallError` in
                    // `ports.rs`), so everything below is a wire-typed answer.
                    Err(MetaFieldSubmissionError::Driver(FaError::Timeout(message))) => {
                        guardian_errors.insert(index, message);
                    }
                    Err(MetaFieldSubmissionError::Driver(error)) => return Err(error),
                    Err(MetaFieldSubmissionError::ManifoldGuardian(
                        ManifoldGuardianError::MetaKeyRefused,
                    )) => {
                        unsupported.insert(index);
                        guardian_errors
                            .insert(index, ManifoldGuardianError::MetaKeyRefused.to_string());
                    }
                    Err(MetaFieldSubmissionError::ManifoldGuardian(
                        ManifoldGuardianError::SeatUnavailable,
                    )) => {
                        guardian_errors
                            .insert(index, ManifoldGuardianError::SeatUnavailable.to_string());
                    }
                    // Not retryable for this base and not a policy refusal of
                    // the value either: the guardian pinned this base to a
                    // different admitted target. Stop submitting to that seat
                    // and keep polling — the conflicting write converging (a
                    // fresh base) or an operator restart is what clears it.
                    Err(MetaFieldSubmissionError::ManifoldGuardian(
                        ManifoldGuardianError::MetaTargetConflict,
                    )) => {
                        pinned_conflicts.insert(index);
                        guardian_errors.insert(index, MAINTENANCE_TARGET_CONFLICT.to_owned());
                    }
                    Err(MetaFieldSubmissionError::ManifoldGuardian(error)) => {
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
                        |error| FaError::MaintenanceConsensusTooLarge {
                            actual_bytes: error.actual_bytes,
                            max_bytes: error.max_bytes,
                        },
                    )?;
                    if meta_fields_match(snapshot.meta_value.as_deref(), fields)? {
                        return Ok(());
                    }
                    let observed_base = MetaConsensusBase::from_consensus(
                        snapshot_meta_consensus(&snapshot)
                            .map_err(|reason| FaError::MaintenanceConsensusInvalid { reason })?,
                    );
                    if observed_base != expected_base {
                        active_base = None;
                        continue;
                    }
                }
                Ok(Err(error)) => consensus_error = Some(error.to_string()),
                Err(FaError::Timeout(message)) => consensus_error = Some(message),
                Err(error) => return Err(error),
            }

            if let Some((index, reason)) = terminal_rejections.into_iter().next() {
                return Err(FaError::MaintenanceRejected { index, reason });
            }
            if all_indices.len() - unsupported.len() < threshold {
                return Err(FaError::MaintenanceUnsupported {
                    unsupported: unsupported.into_iter().collect(),
                    required: threshold,
                });
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
        invite: &fedi_decentralized_service_manifold_guardian::InviteCode,
        run: DriverRun<'_>,
    ) -> FaResult<Result<FederationConsensusSnapshot, FederationConsensusError>> {
        run.call("reading federation consensus metadata", || {
            Ok(self.inner.ports.consensus_reader.read_consensus(invite))
        })
        .await
    }

    async fn connect_unresolved_formed_sessions(
        &self,
        authority: &crate::formation::PostFormedAuthority,
        unresolved: &BTreeSet<u16>,
        run: DriverRun<'_>,
    ) -> FaResult<Vec<MaintenanceConnection<F::Client>>> {
        let mut pending = FuturesUnordered::new();
        for seat in &authority.seats {
            if !unresolved.contains(&seat.index) {
                continue;
            }
            let index = seat.index;
            let seat_id = seat.seat_id.clone();
            let locator = seat.locator.clone();
            pending.push(async move {
                let result = run
                    .call("reconnecting to formed Manifold Guardian", || {
                        Ok(self.inner.ports.mg_connector.connect(&locator))
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
                Err(FaError::Timeout(message)) => {
                    outcomes.push(MaintenanceConnection::Retryable { index, message });
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcomes)
    }

    pub(crate) async fn maintenance_authority(
        &self,
        fa_id: FaId,
    ) -> FaResult<crate::formation::PostFormedAuthority> {
        match self.inner.store.load_recovery(fa_id).await? {
            crate::db::FaRecovery::Idle => return Err(FaError::NoActiveFormation),
            crate::db::FaRecovery::Formation(recovery)
                if recovery.snapshot.phase != FormationPhase::Formed =>
            {
                return Err(FaError::MaintenanceWrongState {
                    phase: recovery.snapshot.phase,
                });
            }
            crate::db::FaRecovery::Restored(snapshot)
                if !matches!(
                    self.status(),
                    FaStatus::Restored(current)
                        if current.freshness == FormationFreshness::Fresh
                            && current.formation_id == snapshot.formation_id
                ) =>
            {
                return Err(FaError::MaintenanceWrongState {
                    phase: snapshot.phase,
                });
            }
            crate::db::FaRecovery::Formation(_) | crate::db::FaRecovery::Restored(_) => {}
        }
        self.post_formed_authority(fa_id).await
    }
}

fn maintenance_convergence(
    unresolved: &BTreeSet<u16>,
    guardian_errors: &BTreeMap<u16, String>,
    consensus_error: Option<String>,
) -> FaError {
    FaError::MaintenanceConvergence {
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
) -> FaResult<()> {
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
