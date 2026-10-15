//! Typed, threshold-live post-formation metadata maintenance.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationMetadataUpdate, GatewayApiUrl, ManifoldGuardianError, ManifoldGuardianService,
    MetaFieldKey, MetaFieldValue, RegisterGatewayRequest,
};
use fedimint_core::runtime::Instant;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::formation::{
    MetaFieldSubmission, MetaFieldSubmissionError, SeatSession, Timeouts, meta_field_matches,
    sleep_for_retry,
};
use crate::meta_write::{MetaWriteError, SeatVote, write_consensus_meta};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusError,
    FederationConsensusReader, FederationConsensusSnapshot, ManifoldGuardianConnector,
};

const MAINTENANCE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
/// Sanitized diagnosis for a guardian that pinned the active consensus base
/// to a different admitted target. Distinguishable from a stale base so an
/// app can stop expecting same-base retries to help.
const MAINTENANCE_TARGET_CONFLICT: &str = "guardian already admitted a different metadata target for this consensus base; retrying \
     cannot help until the conflicting write converges or the guardian's operator restarts it";

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
    pub async fn register_gateway(&self, gateway_api: GatewayApiUrl) -> FaResult<()> {
        self.register_gateway_with(gateway_api, Timeouts::default())
            .await
    }

    pub(crate) async fn register_gateway_with(
        &self,
        gateway_api: GatewayApiUrl,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let _guard = self.claim_run_guard().await?;
        let fa_id = self.fa_id()?;
        self.register_gateway_pinned(gateway_api, fa_id, timeouts)
            .await
    }

    pub(crate) async fn register_gateway_pinned(
        &self,
        gateway_api: GatewayApiUrl,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let deadline = Instant::now() + timeouts.long;
        let authority = self.maintenance_authority(fa_id).await?;
        let invite = self.consensus_invite(&authority.invite_code).await;
        let snapshot = self
            .read_metadata_consensus(&invite, timeouts)
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
        let mut retry_delay = timeouts.poll;

        loop {
            if accepted.len() >= threshold {
                match timeouts
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
                .connect_unresolved_formed_sessions(&authority, &unresolved, timeouts)
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
                        let request = self.sign(&request)?;
                        timeouts
                            .call("registering LNv2 gateway", || {
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
                timeouts,
                deadline,
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
    /// types before this method can touch the network. The
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
    /// return [`FaError::MaintenanceRejected`], and a non-formed durable
    /// record returns [`FaError::MaintenanceWrongState`] before connector
    /// work.
    pub async fn update_federation_metadata(
        &self,
        update: FederationMetadataUpdate,
    ) -> FaResult<()> {
        self.update_federation_metadata_with(update, Timeouts::default())
            .await
    }

    pub(crate) async fn update_federation_metadata_with(
        &self,
        update: FederationMetadataUpdate,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let _guard = self.claim_run_guard().await?;
        let fa_id = self.fa_id()?;
        let (key, value) = update.into_field();
        self.update_meta_field_pinned(key, value, fa_id, timeouts)
            .await
    }

    pub(crate) async fn update_meta_field_pinned(
        &self,
        key: MetaFieldKey,
        value: MetaFieldValue,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let deadline = Instant::now() + timeouts.long;
        let authority = self.maintenance_authority(fa_id).await?;
        let invite = self.consensus_invite(&authority.invite_code).await;
        let (authority, invite, key, value) = (&authority, &invite, &key, &value);
        let seats = authority.seats.iter().map(|seat| seat.index).collect();
        let written = write_consensus_meta(
            timeouts,
            Some(deadline),
            seats,
            || self.read_metadata_consensus(invite, timeouts),
            |snapshot| meta_field_matches(snapshot.meta_value.as_deref(), key, value),
            |rows, expected_base| async move {
                let mut votes = Vec::with_capacity(rows.len());
                let mut sessions = Vec::with_capacity(rows.len());
                for outcome in self
                    .connect_unresolved_formed_sessions(authority, &rows, timeouts)
                    .await?
                {
                    match outcome {
                        MaintenanceConnection::Connected(session) => sessions.push(session),
                        MaintenanceConnection::Retryable { index, message } => {
                            votes.push((index, SeatVote::Retry(message)));
                        }
                    }
                }
                for (index, result) in self
                    .submit_meta_field_wave(&sessions, fa_id, expected_base, key, value, timeouts)
                    .await
                {
                    let vote = match result {
                        Ok(MetaFieldSubmission::Accepted) => SeatVote::Accepted,
                        Ok(MetaFieldSubmission::BaseChanged) => {
                            SeatVote::Retry("consensus metadata base changed".to_owned())
                        }
                        // A lost or hung response is ambiguous, never proof of
                        // refusal: it surfaces as the request timeout here (or
                        // as a connector-layer retryable outcome before the
                        // wave). A serialized `ManifoldGuardianError` can never
                        // prove local transport failure (see
                        // `ManifoldGuardianCallError` in `ports.rs`), so
                        // everything below is a wire-typed answer.
                        Err(MetaFieldSubmissionError::Driver(FaError::Timeout(message))) => {
                            SeatVote::Retry(message)
                        }
                        Err(MetaFieldSubmissionError::Driver(error)) => return Err(error),
                        Err(MetaFieldSubmissionError::ManifoldGuardian(
                            ManifoldGuardianError::SeatUnavailable,
                        )) => SeatVote::Retry(ManifoldGuardianError::SeatUnavailable.to_string()),
                        // Not retryable for this base and not a policy refusal
                        // of the value either: the guardian pinned this base
                        // to a different admitted target. The conflicting
                        // write converging (a fresh base) or an operator
                        // restart is what clears it.
                        Err(MetaFieldSubmissionError::ManifoldGuardian(
                            ManifoldGuardianError::MetaTargetConflict,
                        )) => SeatVote::Pinned(MAINTENANCE_TARGET_CONFLICT.to_owned()),
                        Err(MetaFieldSubmissionError::ManifoldGuardian(error)) => {
                            SeatVote::Rejected(error)
                        }
                    };
                    votes.push((index, vote));
                }
                Ok(votes)
            },
        )
        .await;
        written.map(drop).map_err(|error| match error {
            MetaWriteError::Fatal(error) => error,
            MetaWriteError::Rejected { index, reason } => {
                FaError::MaintenanceRejected { index, reason }
            }
            MetaWriteError::TooLarge {
                actual_bytes,
                max_bytes,
            } => FaError::MaintenanceConsensusTooLarge {
                actual_bytes,
                max_bytes,
            },
            MetaWriteError::InvalidBase(reason) => FaError::MaintenanceConsensusInvalid { reason },
            MetaWriteError::Unconverged {
                unresolved,
                guardian_errors,
                consensus_error,
            } => FaError::MaintenanceConvergence {
                unresolved,
                guardian_errors,
                consensus_error,
            },
        })
    }

    pub(crate) async fn read_metadata_consensus(
        &self,
        invite: &fedi_decentralized_service_manifold_guardian::InviteCode,
        timeouts: Timeouts,
    ) -> FaResult<Result<FederationConsensusSnapshot, FederationConsensusError>> {
        timeouts
            .call("reading federation consensus metadata", || {
                Ok(self.inner.ports.consensus_reader.read_consensus(invite))
            })
            .await
    }

    async fn connect_unresolved_formed_sessions(
        &self,
        authority: &crate::formation::PostFormedAuthority,
        unresolved: &BTreeSet<u16>,
        timeouts: Timeouts,
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
                let result = timeouts
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

    async fn maintenance_authority(
        &self,
        fa_id: FaId,
    ) -> FaResult<crate::formation::PostFormedAuthority> {
        if let Some(crate::CreationStage::Forming { phase, .. }) = self
            .inner
            .store
            .load_snapshot(fa_id)
            .await?
            .map(|creation| creation.stage)
        {
            return Err(FaError::MaintenanceWrongState { phase: Some(phase) });
        } else if self.inner.store.load_federation(fa_id).await?.is_none() {
            return Err(FaError::NoActiveFormation);
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
    timeouts: Timeouts,
    deadline: Instant,
    delay: &mut Duration,
    unresolved: &BTreeSet<u16>,
    guardian_errors: &BTreeMap<u16, String>,
    consensus_error: Option<String>,
) -> FaResult<()> {
    if sleep_for_retry(deadline, *delay).await.is_err() {
        return Err(maintenance_convergence(
            unresolved,
            guardian_errors,
            consensus_error,
        ));
    }
    *delay = next_maintenance_retry_delay(timeouts.poll, *delay);
    Ok(())
}

pub(crate) fn next_maintenance_retry_delay(
    configured_poll_interval: Duration,
    delay: Duration,
) -> Duration {
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
