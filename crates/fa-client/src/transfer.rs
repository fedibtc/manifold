//! Guardian transfer: move one guardian of the formed federation onto a spare
//! seat ([`SPEC-guardian-transfer`](../../mg/specs/SPEC-guardian-transfer.md)).
//!
//! The FA grants the move and asks the spare seat to pull the guardian. The
//! destination MG then fetches it from the source MG on its own, so the
//! FA only has to come back to finish. Finishing has two steps:
//!
//! 1. Every guardian votes the moved peer's new seat binding and fee
//!    recipient into consensus. The FA confirms the result by readback.
//! 2. The source MG deletes the old guardian data.
//!
//! The local seat list changes between the two steps, in the same transaction
//! that takes the spare out of the pool.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_domain::{
    MG_SEAT_BINDINGS_META_FIELD_KEY, MgPeerAttestation, MgSeatBindings, federation_seats,
};
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, FederationId, FormationSeatBinding,
    GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY, GetPeerAttestationRequest, GetStatusRequest, Locator,
    ManifoldGuardianError, ManifoldGuardianService, MetaConsensusBase, PullSeatRequest,
    RemoveSeatRequest, ReplaceSeatBindingRequest, SeatId, SeatTransferGrant, ServiceStatus,
    SignedRequest, Timestamp, rebind_transferred_seat,
};
use fedimint_core::runtime::Instant;
use futures::stream::{FuturesUnordered, StreamExt as _};
use nostr_sdk::PublicKey;

use crate::db::GuardianTransferWrite;
use crate::formation::{
    DriverRun, PostFormedAuthority, PostFormedSeat, SeatSession, finish_driver_run, now_secs,
    sleep_for_retry, snapshot_meta_consensus, start_driver_run,
};
use crate::maintenance::{
    MaintenanceConnection, maintenance_convergence, sleep_for_maintenance_retry,
};
use crate::mg::MgClient;
use crate::seat_pool::StoredSpareSeat;
use crate::{
    FaClient, FaError, FaId, FaIdentity, FaPayments, FaResult, FaStatus, FederationConsensusReader,
    FormationFreshness, MaintenanceRunOptions, ManifoldGuardianConnector, MgCause, MgOperation,
    SpareSeat,
};

/// How long a grant lets the destination keep asking the source.
const GRANT_LIFETIME: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// One guardian moving from its MG onto a spare seat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardianTransfer {
    /// Position of the moved guardian in the federation's seat list.
    pub seat_index: u16,
    /// MG that hosted the guardian.
    pub source_mg_id: PublicKey,
    /// Seat on the source MG.
    pub source_seat_id: SeatId,
    /// Spare seat that receives the guardian.
    pub destination: SpareSeat,
    /// Last second at which the destination asks the source.
    pub expires_at: Timestamp,
    pub phase: GuardianTransferPhase,
}

/// Where a guardian transfer stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianTransferPhase {
    /// The destination waits for the source MG to hand over the guardian.
    AwaitingSource,
    /// The guardian runs on the destination. Its new seat binding and fee
    /// recipient are not confirmed in consensus yet.
    PublishingBinding,
    /// Consensus names the destination. The source MG has not deleted the
    /// old guardian data yet.
    RemovingSource,
    /// The transfer is done.
    Completed,
    /// The grant expired before the source handed over the guardian. The
    /// spare seat is back in the pool.
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredTransferPhase {
    Pulling,
    Binding,
    RemovingSource,
}

/// The one open guardian transfer.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardianTransfer {
    pub(crate) seat_index: u16,
    #[serde(rename = "source_fman_id")]
    pub(crate) source_mg_id: PublicKey,
    pub(crate) source_locator: Locator,
    pub(crate) source_seat_id: SeatId,
    pub(crate) destination: StoredSpareSeat,
    pub(crate) grant: SignedRequest<SeatTransferGrant>,
    /// The grant's expiry, which its envelope only exposes after verification.
    pub(crate) expires_at: Timestamp,
    /// Bearer callback the destination invokes once the guardian runs.
    pub(crate) completion_callback: Option<DkgCompletionCallback>,
    /// The destination's binding for the moved peer, saved before the first
    /// vote. The destination signs a new one on every request, so a resumed
    /// transfer must vote and look for the same one.
    pub(crate) binding: Option<FormationSeatBinding>,
    pub(crate) phase: StoredTransferPhase,
}

impl StoredGuardianTransfer {
    fn view(&self, phase: GuardianTransferPhase) -> GuardianTransfer {
        GuardianTransfer {
            seat_index: self.seat_index,
            source_mg_id: self.source_mg_id,
            source_seat_id: self.source_seat_id.clone(),
            destination: self.destination.clone().into(),
            expires_at: self.expires_at,
            phase,
        }
    }

    fn public_phase(&self) -> GuardianTransferPhase {
        match self.phase {
            StoredTransferPhase::Pulling => GuardianTransferPhase::AwaitingSource,
            StoredTransferPhase::Binding => GuardianTransferPhase::PublishingBinding,
            StoredTransferPhase::RemovingSource => GuardianTransferPhase::RemovingSource,
        }
    }
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Start to move the guardian at `seat_index` onto a spare seat.
    ///
    /// The FA signs a grant that is valid for 14 days and asks the spare seat
    /// to pull the guardian from its MG. The call then advances the
    /// transfer like [`Self::resume_guardian_transfer`]. The destination keeps
    /// asking the source on its own, so the caller can return later, for
    /// example when `completion_callback` reports that the guardian runs.
    /// Only one transfer is open at a time.
    pub async fn start_guardian_transfer(
        &self,
        seat_index: u16,
        spare_seat_id: SeatId,
        completion_callback: Option<DkgCompletionCallback>,
        options: MaintenanceRunOptions,
    ) -> FaResult<GuardianTransfer> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let options = options.run_options();
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let authority = self.maintenance_authority(fa_id).await?;
        // A liquidity request commits to the current seats. Moving one now
        // would strand a request that the provider may already have accepted.
        if let Some(operation) = self.find_live_liquidity_operation(|_| true).await? {
            return Err(FaError::LiquidityOperationExists {
                operation_id: operation.operation_id,
            });
        }
        let source = authority
            .seats
            .iter()
            .find(|seat| seat.index == seat_index)
            .ok_or_else(|| FaError::InvalidIntent("no guardian has this seat index".to_owned()))?;
        let source_mg_id = source.mg_id.ok_or_else(|| {
            FaError::InvalidIntent("the guardian's MG has no authenticated identity".to_owned())
        })?;
        let destination = self
            .inner
            .store
            .list_spare_seats()
            .await?
            .into_iter()
            .find(|spare| spare.seat_id == spare_seat_id)
            .ok_or_else(|| FaError::InvalidIntent("no spare seat has this id".to_owned()))?;
        let now = now_secs()?;
        let expires_at = Timestamp(now + GRANT_LIFETIME.as_secs());
        let grant = self.sign(&SeatTransferGrant {
            ts: Timestamp(now),
            fa_id,
            federation_id: FederationId(
                crate::formation::invite_federation_id(&authority.invite_code)?.to_string(),
            ),
            source: source.locator.clone(),
            source_seat_id: source.seat_id.clone(),
            destination_service_pubkey: destination.locator.service_pubkey,
            destination_seat_id: destination.seat_id.clone(),
            expires_at,
        })?;
        let transfer = StoredGuardianTransfer {
            seat_index,
            source_mg_id,
            source_locator: source.locator.clone(),
            source_seat_id: source.seat_id.clone(),
            destination,
            grant,
            expires_at,
            completion_callback,
            binding: None,
            phase: StoredTransferPhase::Pulling,
        };
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            lease
                .write_guardian_transfer(fa_id, GuardianTransferWrite::Begin(transfer.clone()))
                .await?;
            self.drive_guardian_transfer(transfer, fa_id, false, run)
                .await
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Advance the open transfer as far as it goes within `options`.
    ///
    /// Waiting for the source MG or for its removal is not an error: the
    /// result reports the phase, and a later call continues from it. A
    /// guardian that cannot confirm the new binding in time returns
    /// [`FaError::MaintenanceConvergence`]. `None` means no transfer is open.
    pub async fn resume_guardian_transfer(
        &self,
        options: MaintenanceRunOptions,
    ) -> FaResult<Option<GuardianTransfer>> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let options = options.run_options();
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let Some(transfer) = self.inner.store.load_guardian_transfer().await? else {
            return Ok(None);
        };
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self
            .drive_guardian_transfer(transfer, fa_id, true, run)
            .await
            .map(Some);
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// The open transfer, if any, as last recorded.
    pub async fn guardian_transfer(&self) -> FaResult<Option<GuardianTransfer>> {
        Ok(self
            .inner
            .store
            .load_guardian_transfer()
            .await?
            .map(|transfer| transfer.view(transfer.public_phase())))
    }

    async fn drive_guardian_transfer(
        &self,
        mut transfer: StoredGuardianTransfer,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<GuardianTransfer> {
        let lease = run.lease();
        if transfer.phase == StoredTransferPhase::Pulling {
            match self
                .await_pulled_guardian(&transfer, fa_id, wait, run)
                .await?
            {
                Pulled::Running => {
                    transfer.phase = StoredTransferPhase::Binding;
                    lease
                        .write_guardian_transfer(
                            fa_id,
                            GuardianTransferWrite::Update(transfer.clone()),
                        )
                        .await?;
                }
                Pulled::Waiting => return Ok(transfer.view(GuardianTransferPhase::AwaitingSource)),
                Pulled::Expired => {
                    lease
                        .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                        .await?;
                    return Ok(transfer.view(GuardianTransferPhase::Expired));
                }
            }
        }
        if transfer.phase == StoredTransferPhase::Binding {
            // The seat list may be stale after a reopen: consensus can name
            // the destination already. The readback below confirms the moved
            // seat itself, so the transfer does not wait for a full
            // reconciliation, which would refuse the destination.
            let authority = self.post_formed_authority(fa_id).await?;
            let binding = match transfer.binding.clone() {
                Some(binding) => binding,
                None => {
                    let binding = self.destination_binding(&transfer, fa_id, run).await?;
                    transfer.binding = Some(binding.clone());
                    lease
                        .write_guardian_transfer(
                            fa_id,
                            GuardianTransferWrite::Update(transfer.clone()),
                        )
                        .await?;
                    binding
                }
            };
            self.publish_moved_binding(&transfer, &binding, &authority, fa_id, run)
                .await?;
            let was_fresh = !matches!(
                self.status(),
                FaStatus::Restored(snapshot) if snapshot.freshness != FormationFreshness::Fresh
            );
            transfer.phase = StoredTransferPhase::RemovingSource;
            lease
                .write_guardian_transfer(fa_id, GuardianTransferWrite::Rebind(transfer.clone()))
                .await?;
            let mut status = self.inner.store.load_status(fa_id).await?;
            if was_fresh && let FaStatus::Restored(snapshot) = &mut status {
                snapshot.freshness = FormationFreshness::Fresh;
            }
            self.inner.progress.send_replace(status);
        }
        match self.remove_source_seat(&transfer, fa_id, run).await {
            Ok(()) => {
                lease
                    .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                    .await?;
                Ok(transfer.view(GuardianTransferPhase::Completed))
            }
            Err(FaError::Mg(failure))
                if matches!(failure.cause, MgCause::Timeout | MgCause::Transport) =>
            {
                Ok(transfer.view(GuardianTransferPhase::RemovingSource))
            }
            Err(error) => Err(error),
        }
    }

    /// Ask the destination to pull. With `wait`, poll it until the guardian
    /// runs, the grant expires, or the run ends. An unreachable destination
    /// counts as still waiting.
    async fn await_pulled_guardian(
        &self,
        transfer: &StoredGuardianTransfer,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<Pulled> {
        match self.poll_pulled_guardian(transfer, fa_id, wait, run).await {
            Err(FaError::Mg(failure))
                if matches!(failure.cause, MgCause::Timeout | MgCause::Transport) =>
            {
                Ok(Pulled::Waiting)
            }
            result => result,
        }
    }

    async fn poll_pulled_guardian(
        &self,
        transfer: &StoredGuardianTransfer,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<Pulled> {
        let destination = &transfer.destination;
        let client = match MgClient::connect(
            &self.inner.ports.mg_connector,
            &destination.locator,
            run,
        )
        .await
        {
            Ok(client) => client,
            Err(FaError::Mg(_)) => return Ok(Pulled::Waiting),
            Err(error) => return Err(error),
        };
        loop {
            let request = self.sign(&GetStatusRequest {
                ts: Timestamp(now_secs()?),
                fa_id,
                seat_id: destination.seat_id.clone(),
            })?;
            let status = client
                .call(run, MgOperation::GetStatus, |client| {
                    client.get_status(request)
                })
                .await?
                .status;
            match status {
                ServiceStatus::Running => return Ok(Pulled::Running),
                ServiceStatus::AwaitingTransfer => {}
                ServiceStatus::New if transfer.expires_at.0 < now_secs()? => {
                    return Ok(Pulled::Expired);
                }
                ServiceStatus::New => {
                    let request = self.sign(&PullSeatRequest {
                        ts: Timestamp(now_secs()?),
                        fa_id,
                        seat_id: destination.seat_id.clone(),
                        grant: transfer.grant.clone(),
                        completion_callback: transfer.completion_callback.clone(),
                    })?;
                    client
                        .call(run, MgOperation::PullSeat, |client| {
                            client.pull_seat(request)
                        })
                        .await?;
                    continue;
                }
                status => {
                    return Err(client
                        .failure(MgOperation::GetStatus, MgCause::WrongState(status))
                        .into());
                }
            }
            if !wait
                || sleep_for_retry(run.deadline(), run.poll_interval())
                    .await
                    .is_err()
            {
                return Ok(Pulled::Waiting);
            }
        }
    }

    /// Fetch and check the destination's binding for the moved peer.
    async fn destination_binding(
        &self,
        transfer: &StoredGuardianTransfer,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<FormationSeatBinding> {
        let destination = &transfer.destination;
        let client =
            MgClient::connect(&self.inner.ports.mg_connector, &destination.locator, run).await?;
        let request = self.sign(&GetPeerAttestationRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            seat_id: destination.seat_id.clone(),
        })?;
        let response = client
            .call(run, MgOperation::GetPeerAttestation, |client| {
                client.get_peer_attestation(request)
            })
            .await?;
        let binding = FormationSeatBinding {
            attestation: response.mg_peer_attestation,
            endpoint_proof: response.seat_endpoint_proof,
        };
        let statement = binding.attestation.verify().map_err(|_| {
            client.failure(MgOperation::GetPeerAttestation, MgCause::InvalidResponse)
        })?;
        if statement.mg_pubkey.0 != destination.mg_id.to_string()
            || destination
                .guardian_fee_account
                .as_ref()
                .is_some_and(|account| account != &statement.guardian_fee_account)
        {
            return Err(client
                .failure(MgOperation::GetPeerAttestation, MgCause::InvalidResponse)
                .into());
        }
        Ok(binding)
    }

    /// Vote the destination's binding for the moved peer into consensus and
    /// confirm it by readback.
    async fn publish_moved_binding(
        &self,
        transfer: &StoredGuardianTransfer,
        binding: &FormationSeatBinding,
        authority: &PostFormedAuthority,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let destination = &transfer.destination;
        // Every current guardian votes, with the destination in place of the
        // stopped source.
        let mut voters = authority.clone();
        for seat in &mut voters.seats {
            if seat.index == transfer.seat_index {
                *seat = PostFormedSeat {
                    index: seat.index,
                    mg_id: Some(destination.mg_id),
                    locator: destination.locator.clone(),
                    seat_id: destination.seat_id.clone(),
                };
            }
        }
        let all = voters
            .seats
            .iter()
            .map(|seat| seat.index)
            .collect::<BTreeSet<_>>();
        let invite = self.consensus_invite(&authority.invite_code).await;
        let mut active_base = None;
        let mut unresolved = all.clone();
        let mut guardian_errors = BTreeMap::new();
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
            if moved_binding_adopted(&snapshot, &binding.attestation, transfer)? {
                return Ok(());
            }
            let expected_base = MetaConsensusBase::from_consensus(
                snapshot_meta_consensus(&snapshot)
                    .map_err(|reason| FaError::MaintenanceConsensusInvalid { reason })?,
            );
            if active_base != Some(expected_base) {
                active_base = Some(expected_base);
                unresolved.clone_from(&all);
                guardian_errors.clear();
                retry_delay = run.poll_interval();
            }

            let mut sessions = Vec::with_capacity(unresolved.len());
            for outcome in self
                .connect_unresolved_formed_sessions(&voters, &unresolved, run)
                .await?
            {
                match outcome {
                    MaintenanceConnection::Connected(session) => sessions.push(session),
                    MaintenanceConnection::Retryable { index, message } => {
                        guardian_errors.insert(index, message);
                    }
                }
            }
            let mut terminal = BTreeMap::new();
            for (index, result) in self
                .submit_binding_wave(&sessions, fa_id, expected_base, binding, run)
                .await
            {
                match result {
                    Ok(Ok(_)) => {
                        unresolved.remove(&index);
                        guardian_errors.remove(&index);
                    }
                    Ok(Err(
                        error @ (ManifoldGuardianError::MetaConsensusChanged
                        | ManifoldGuardianError::SeatUnavailable),
                    )) => {
                        guardian_errors.insert(index, error.to_string());
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

            match self.read_metadata_consensus(&invite, run).await {
                Ok(Ok(snapshot)) => {
                    consensus_error = None;
                    if moved_binding_adopted(&snapshot, &binding.attestation, transfer)? {
                        return Ok(());
                    }
                }
                Ok(Err(error)) => consensus_error = Some(error.to_string()),
                Err(FaError::Timeout(message)) => consensus_error = Some(message),
                Err(error) => return Err(error),
            }
            if let Some((index, reason)) = terminal.into_iter().next() {
                return Err(FaError::MaintenanceRejected { index, reason });
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

    async fn submit_binding_wave(
        &self,
        sessions: &[SeatSession<F::Client>],
        fa_id: FaId,
        expected_base: MetaConsensusBase,
        binding: &FormationSeatBinding,
        run: DriverRun<'_>,
    ) -> Vec<(u16, FaResult<Result<(), ManifoldGuardianError>>)> {
        let mut pending = sessions
            .iter()
            .map(|session| async move {
                let result = async {
                    let request = ReplaceSeatBindingRequest {
                        ts: Timestamp(now_secs()?),
                        fa_id,
                        seat_id: session.seat_id.clone(),
                        expected_base,
                        seat_binding: binding.clone(),
                    };
                    let request = run
                        .construct("signing ReplaceSeatBinding request", || self.sign(&request))
                        .await?;
                    run.call("submitting ReplaceSeatBinding vote", || {
                        Ok(session.client.replace_seat_binding(request))
                    })
                    .await
                    .map(|result| result.map(|_| ()))
                }
                .await;
                (session.index, result)
            })
            .collect::<FuturesUnordered<_>>();
        let mut results = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            results.push(result);
        }
        results
    }

    async fn remove_source_seat(
        &self,
        transfer: &StoredGuardianTransfer,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let client = MgClient::connect(
            &self.inner.ports.mg_connector,
            &transfer.source_locator,
            run,
        )
        .await?;
        let request = self.sign(&RemoveSeatRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            seat_id: transfer.source_seat_id.clone(),
        })?;
        client
            .call(run, MgOperation::RemoveSeat, |client| {
                client.remove_seat(request)
            })
            .await?;
        Ok(())
    }
}

enum Pulled {
    Running,
    Waiting,
    Expired,
}

/// Whether consensus names the destination for the moved peer, in both the
/// seat-binding directory and the fee recipients. Another binding of the
/// destination for the same peer counts too: an earlier run may have voted
/// one before this device saved it.
fn moved_binding_adopted(
    snapshot: &crate::FederationConsensusSnapshot,
    attestation: &MgPeerAttestation,
    transfer: &StoredGuardianTransfer,
) -> FaResult<bool> {
    let invalid = |reason: String| FaError::MaintenanceConsensusInvalid { reason };
    let fields: BTreeMap<String, serde_json::Value> = match snapshot.meta_value.as_deref() {
        Some(value) => serde_json::from_slice(value)
            .map_err(|error| invalid(format!("consensus metadata is not an object: {error}")))?,
        None => BTreeMap::new(),
    };
    let field = |key: &str| match fields.get(key) {
        Some(serde_json::Value::String(value)) => Ok(value.as_str()),
        _ => Err(invalid(format!("consensus metadata has no string {key}"))),
    };
    let directory = field(MG_SEAT_BINDINGS_META_FIELD_KEY)?;
    let recipients = field(GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY)?;
    let bindings = MgSeatBindings::parse_canonical(directory)
        .map_err(|error| invalid(format!("seat-binding directory is invalid: {error}")))?;
    let current = bindings
        .seat_bindings()
        .iter()
        .find(|entry| entry.attestation.peer_id == attestation.attestation.peer_id)
        .ok_or_else(|| invalid("the moved peer has no seat binding".to_owned()))?;
    if current.attestation.mg_pubkey != attestation.attestation.mg_pubkey {
        // Still the source's entry, or a different one. Refuse a peer that
        // the source does not hold: the attestation names the wrong guardian.
        if current.attestation.mg_pubkey.0 != transfer.source_mg_id.to_string() {
            return Err(invalid(
                "the destination attests a guardian that the source does not host".to_owned(),
            ));
        }
        return Ok(false);
    }
    // Adopted values must be exactly what the shared rule derives, and the
    // full directory must still verify against the federation.
    let (expected_directory, expected_recipients) =
        rebind_transferred_seat(directory, recipients, current)
            .map_err(|error| invalid(error.to_string()))?;
    if expected_directory != directory || expected_recipients != recipients {
        return Ok(false);
    }
    let federation = federation_seats(&snapshot.config)
        .map_err(|error| invalid(format!("federation config is not usable: {error}")))?;
    bindings
        .verify_for_federation(&federation)
        .map_err(|error| invalid(format!("seat-binding directory is invalid: {error}")))?;
    Ok(true)
}
