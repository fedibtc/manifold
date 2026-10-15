//! Crash-safe formation of a full pool: DKG, then seat-binding publication.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_domain::{
    MG_SEAT_BINDINGS_META_FIELD_KEY, MgSeatBindings, federation_seats,
};
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::*;
use fedimint_core::config::{ClientConfig, FederationId as FedimintFederationId};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::runtime::{Instant, sleep, timeout};
use futures::stream::{FuturesUnordered, StreamExt as _};
use sha2::{Digest as _, Sha256};
use stability_pool_common::Account;

use crate::db::{FormationMetaTarget, FormationView};
use crate::meta_write::{MetaWriteError, SeatVote, write_consensus_meta};
use crate::mg::MgClient;
use crate::ports::FaIdentityExt as _;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    FederationConsensusSnapshot, FormationId, GuardianFeePpm, ManifoldGuardianConnector, MgCause,
    MgFailure, MgOperation, SeatPhase,
};

pub(crate) fn log_mg_failure(formation_id: &FormationId, error: &FaError, outcome: &'static str) {
    let failure = match error {
        FaError::Mg(failure)
        | FaError::SelectionReauthorizationRequired {
            failure: Some(failure),
            ..
        } => failure,
        _ => return,
    };
    tracing::warn!(
        safe_to_share = true,
        formation_id = %formation_id.0,
        mg_pubkey = %failure.mg,
        operation = %failure.operation,
        cause = %failure.cause,
        outcome,
        "formation request failed"
    );
}

/// Smallest duration represented without truncation by native and WASM runtime timers.
pub(crate) const MIN_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(1);
/// Largest duration represented without overflow by native and WASM runtime timers.
pub(crate) const MAX_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(i32::MAX as u64);

/// The timeouts of FA calls. No step of the run loop has a deadline: a
/// step ends when its work is done, when a call fails, or when a user action
/// wakes the loop.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    /// One ordinary MG, consensus, or wallet call.
    pub(crate) request: Duration,
    /// The delay between two probes of a wait.
    pub(crate) poll: Duration,
    /// A payment, its recovery, a refund, a backup read, and one maintenance
    /// action, which the caller waits for.
    pub(crate) long: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(30),
            poll: Duration::from_secs(2),
            long: Duration::from_secs(600),
        }
    }
}

impl Timeouts {
    /// Poll one ordinary call under the request timeout.
    pub(crate) async fn call<T, Fut>(
        self,
        operation: &'static str,
        make_future: impl FnOnce() -> FaResult<Fut>,
    ) -> FaResult<T>
    where
        Fut: Future<Output = T>,
    {
        poll_with_timeout(operation, self.request, make_future()?).await
    }

    /// Poll a payment, its recovery, a refund, or a backup read under the
    /// long timeout.
    pub(crate) async fn long_call<T, Fut>(
        self,
        operation: &'static str,
        make_future: impl FnOnce() -> FaResult<Fut>,
    ) -> FaResult<T>
    where
        Fut: Future<Output = T>,
    {
        poll_with_timeout(operation, self.long, make_future()?).await
    }
}

async fn poll_with_timeout<T>(
    operation: &'static str,
    duration: Duration,
    future: impl Future<Output = T>,
) -> FaResult<T> {
    timeout(duration, future)
        .await
        .map_err(|_| FaError::Timeout(operation.to_owned()))
}

pub(crate) struct SeatSession<C> {
    pub(crate) index: u16,
    pub(crate) client: C,
    pub(crate) seat_id: SeatId,
}

/// One guardian's reply to an explicit restart. An error does not prove that
/// its old ceremony survived: the request or acknowledgement may have been lost.
#[derive(Debug)]
pub struct DkgRestartResult {
    /// Index in the saved formation's guardian set.
    pub index: u16,
    /// Identity of the Manifold Guardian that owns this guardian.
    pub mg: secp256k1::XOnlyPublicKey,
    /// `DkgInProcess` acknowledges a new ceremony; `Running` means completion
    /// won the race. Errors may follow a partial change on the guardian.
    pub result: FaResult<ServiceStatus>,
}

#[derive(Clone)]
pub(crate) struct PostFormedSeat {
    pub(crate) index: u16,
    pub(crate) mg_id: nostr_sdk::PublicKey,
    pub(crate) locator: Locator,
    pub(crate) seat_id: SeatId,
}

pub(crate) struct PostFormedAuthority {
    pub(crate) formation_id: FormationId,
    pub(crate) invite_code: InviteCode,
    pub(crate) seats: Vec<PostFormedSeat>,
}

/// One guardian's guarded metadata-vote result.
pub(crate) enum MetaFieldSubmission {
    /// The guardian accepted and submitted this exact base-bound mutation.
    Accepted,
    /// The guardian observed another consensus base before it could submit.
    BaseChanged,
}

/// Failure from the shared guarded metadata-vote primitive.
pub(crate) enum MetaFieldSubmissionError {
    /// Local identity, storage, or request-timeout failure.
    Driver(FaError),
    /// Typed refusal returned by the MG protocol.
    ManifoldGuardian(ManifoldGuardianError),
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Propose a new guardian-fee rate after formation.
    ///
    /// Formation fixes the canonical recipient split. This operation changes
    /// only its rate through the generic metadata verb, bound to the exact live
    /// whole-object metadata base; success is confirmed by a fresh consensus
    /// read containing the requested rate.
    ///
    /// The resolved FA account must be a single-signature `BtcDepositor`
    /// account and the rate must be within the pinned payer's inclusive
    /// `0..=210_000`-ppm ceiling and at or above the published minimum
    /// carried by the admitted setup-payment publication (1,500 ppm when none
    /// is admitted).
    /// Both bounds are refused here, before any guardian is contacted, so a
    /// rate every MG would vote down reports its own reason instead of
    /// retrying to a timeout. MG remains
    /// the authoritative split validator and may reject a proposal before
    /// casting its guardian vote. Transport, stale-base, or consensus-read
    /// failures may yield a timeout; cancellation stores no partial policy
    /// record, and retrying the same call safely rereads, rebases, and confirms
    /// the live consensus value.
    pub async fn propose_guardian_fees(&self, send_ppm: GuardianFeePpm) -> FaResult<()> {
        self.propose_guardian_fees_with(send_ppm, Timeouts::default())
            .await
    }

    pub(crate) async fn propose_guardian_fees_with(
        &self,
        send_ppm: GuardianFeePpm,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let _run = self.claim_run_guard().await?;
        if send_ppm.value() > crate::MAX_GUARDIAN_FEE_PPM {
            return Err(FaError::InvalidIntent(format!(
                "guardian fee ppm exceeds the payer ceiling of {}",
                crate::MAX_GUARDIAN_FEE_PPM
            )));
        }
        let min_send_ppm = self.min_guardian_fee_ppm().await;
        if u64::from(send_ppm.value()) < min_send_ppm {
            return Err(FaError::InvalidIntent(format!(
                "guardian fee ppm is below the published minimum of {min_send_ppm}"
            )));
        }
        let fa_id = self.fa_id()?;
        let key = MetaFieldKey(
            fedi_decentralized_service_manifold_guardian::GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY
                .to_owned(),
        );
        let value = MetaFieldValue(send_ppm.value().to_string());
        self.update_meta_field_pinned(key, value, fa_id, timeouts)
            .await
    }

    /// Explicitly replace every unfinished guardian ceremony, without paying
    /// again. All status checks finish before any restart is sent. Guardians
    /// can still finish or disappear afterward, so this is a best-effort wave.
    ///
    /// No request is retried. Cancellation or a missing reply leaves the
    /// outcome unknown; another restart requires a new user decision. The
    /// run loop yields to this call, then continues the formation.
    pub async fn restart_dkg(&self) -> FaResult<Vec<DkgRestartResult>> {
        self.restart_dkg_with(Timeouts::default()).await
    }

    pub(crate) async fn restart_dkg_with(
        &self,
        timeouts: Timeouts,
    ) -> FaResult<Vec<DkgRestartResult>> {
        let _guard = self.claim_run_guard().await?;
        let fa_id = self.fa_id()?;
        async {
            let formation = self.active_formation(fa_id).await?;
            let formation_id = &formation.formation_id;
            if formation.invite_code.is_some() {
                return Err(FaError::InvalidIntent(
                    "formation cannot restart DKG in its current state".into(),
                ));
            }
            let codes = formation
                .seats
                .iter()
                .map(|seat| {
                    seat.guardian_code.clone().ok_or_else(|| {
                        FaError::InvalidIntent("restart requires every saved guardian code".into())
                    })
                })
                .collect::<FaResult<Vec<_>>>()?;
            let sessions = self.seat_sessions(&formation, timeouts).await?;
            let mut checks = FuturesUnordered::new();
            for session in &sessions {
                checks.push(async {
                    let response = self.seat_status(session, fa_id, timeouts).await?;
                    if !matches!(
                        response.status,
                        ServiceStatus::New | ServiceStatus::DkgInProcess
                    ) {
                        return Err(session
                            .client
                            .failure(
                                MgOperation::RestartDkg,
                                MgCause::WrongState(response.status),
                            )
                            .into());
                    }
                    Ok::<_, FaError>(())
                });
            }
            while let Some(check) = checks.next().await {
                check?;
            }
            drop(checks);
            let mut pending = FuturesUnordered::new();
            for session in &sessions {
                let codes = &codes;
                pending.push(async move {
                    let result = async {
                        let request = self.sign(&RestartDkgRequest {
                            ts: Timestamp(now_secs()?),
                            fa_id,
                            seat_id: session.seat_id.clone(),
                            guardian_codes: codes.clone(),
                        })?;
                        let response = session
                            .client
                            .call(timeouts, MgOperation::RestartDkg, |client| {
                                client.restart_dkg(request)
                            })
                            .await?;
                        Ok(response.status)
                    }
                    .await;
                    if let Err(error) = &result {
                        log_mg_failure(formation_id, error, "returned");
                    }
                    DkgRestartResult {
                        index: session.index,
                        mg: session.client.mg,
                        result,
                    }
                });
            }
            let mut results = Vec::new();
            while let Some(result) = pending.next().await {
                results.push(result);
            }
            results.sort_by_key(|result| result.index);
            Ok(results)
        }
        .await
    }

    /// Collect every guardian code and start DKG on every seat.
    async fn start_dkg(
        &self,
        formation: &FormationView,
        sessions: &[SeatSession<MgClient<F::Client>>],
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let formation_id = &formation.formation_id;
        let mut pending_codes = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let existing = formation.seats[position].guardian_code.clone();
            let federation_name = (position == 0).then(|| formation.federation_name.clone());
            pending_codes.push(async move {
                let code = self
                    .wait_for_dkg_code(
                        formation_id,
                        session,
                        fa_id,
                        federation_name,
                        existing.clone(),
                        timeouts,
                    )
                    .await
                    .map_err(|error| (position, error))?;
                if let Some(recorded) = existing.as_ref()
                    && recorded != &code
                {
                    return Err((
                        position,
                        FaError::InvalidManifoldGuardians(format!(
                            "Manifold Guardian {} changed its deterministic guardian code",
                            position + 1
                        )),
                    ));
                }
                Ok((position, code, existing.is_none()))
            });
        }
        let mut guardian_codes = (0..sessions.len())
            .map(|_| None)
            .collect::<Vec<Option<GuardianCode>>>();
        let mut first_error = None;
        while let Some(result) = pending_codes.next().await {
            let (position, code, newly_generated) = match result {
                Ok(value) => value,
                Err((_, error)) => {
                    if first_error.is_some() {
                        log_mg_failure(formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            if newly_generated {
                self.inner
                    .store
                    .record_guardian_code(
                        formation_id,
                        formation.seats[position].index,
                        code.clone(),
                    )
                    .await?;
                self.publish(None).await;
            }
            guardian_codes[position] = Some(code);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        let guardian_codes = guardian_codes
            .into_iter()
            .enumerate()
            .map(|(position, code)| {
                code.ok_or_else(|| {
                    FaError::Storage(format!("missing guardian code for seat {position}"))
                })
            })
            .collect::<FaResult<Vec<_>>>()?;

        let mut pending_starts = FuturesUnordered::new();
        for session in sessions {
            let timestamp = Timestamp(now_secs()?);
            let completion_callback = formation.dkg_completion_callback.clone();
            let guardian_codes = guardian_codes.clone();
            let seat_id = session.seat_id.clone();
            pending_starts.push(async move {
                let request = StartDkgRequest {
                    ts: timestamp,
                    fa_id,
                    seat_id,
                    guardian_codes,
                    completion_callback,
                };
                let request = self.sign(&request)?;
                match session.client.call(timeouts, MgOperation::StartDkg, |client| client.start_dkg(request)).await {
                    Ok(_) | Err(FaError::Mg(MgFailure {
                        cause: MgCause::WrongState(ServiceStatus::DkgInProcess | ServiceStatus::Running),
                        ..
                    })) => Ok::<_, FaError>(()),
                    Err(error) => Err(error),
                }
            });
        }
        let mut first_error = None;
        while let Some(result) = pending_starts.next().await {
            if let Err(error) = result {
                if first_error.is_some() {
                    log_mg_failure(formation_id, &error, "discarded");
                }
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Wait until the seat's MG returns its guardian code. A seat whose
    /// `fedimintd` is not ready yet is normal progress.
    async fn wait_for_dkg_code(
        &self,
        formation_id: &FormationId,
        session: &SeatSession<MgClient<F::Client>>,
        fa_id: FaId,
        federation_name: Option<FederationName>,
        recorded_code: Option<GuardianCode>,
        timeouts: Timeouts,
    ) -> FaResult<GuardianCode> {
        wait_until(timeouts.poll, || async {
            let request = GetDkgCodeRequest {
                ts: Timestamp(now_secs()?),
                fa_id,
                seat_id: session.seat_id.clone(),
                federation_name: federation_name.clone(),
            };
            let request = self.sign(&request)?;
            match session
                .client
                .call(timeouts, MgOperation::GetDkgCode, |client| {
                    client.get_dkg_code(request)
                })
                .await
            {
                Ok(response) => Ok(Some(response.guardian_code)),
                Err(
                    error @ FaError::Mg(MgFailure {
                        cause: MgCause::SeatUnavailable,
                        ..
                    }),
                ) => {
                    log_mg_failure(formation_id, &error, "retried");
                    Ok(None)
                }
                Err(FaError::Mg(MgFailure {
                    cause: MgCause::WrongState(ServiceStatus::DkgInProcess | ServiceStatus::Running),
                    ..
                })) if recorded_code.is_some() => Ok(recorded_code.clone()),
                Err(error) => Err(error),
            }
        })
        .await
    }

    /// Read one seat's service status from its MG.
    async fn seat_status(
        &self,
        session: &SeatSession<MgClient<F::Client>>,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<GetStatusResponse> {
        let request = GetStatusRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            seat_id: session.seat_id.clone(),
        };
        let request = self.sign(&request)?;
        session
            .client
            .call(timeouts, MgOperation::GetStatus, |client| {
                client.get_status(request)
            })
            .await
    }

    /// Poll every seat until each runs healthy, and return the invite each
    /// running seat reports, in seat order.
    async fn poll_until_running(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<Vec<InviteCode>> {
        wait_until(timeouts.poll, || async {
            let mut invites = (0..sessions.len()).map(|_| None).collect::<Vec<_>>();
            let mut pending_status = FuturesUnordered::new();
            for (position, session) in sessions.iter().enumerate() {
                pending_status.push(async move {
                    let status = self.seat_status(session, fa_id, timeouts).await?;
                    Ok::<_, FaError>((position, status))
                });
            }
            while let Some(result) = pending_status.next().await {
                let (position, status) = result?;
                let failure = |cause| {
                    sessions[position]
                        .client
                        .failure(MgOperation::GetStatus, cause)
                };
                service_seat_phase(&status.status).map_err(failure)?;
                if status.status == ServiceStatus::Running
                    && status.seat_health == Some(SeatHealth::Healthy)
                {
                    let StatusDetail::Federation(FederationStatusInfo {
                        invite_code: Some(invite),
                        ..
                    }) = status.detail
                    else {
                        return Err(failure(MgCause::InvalidResponse).into());
                    };
                    invites[position] = Some(invite);
                }
            }
            Ok(invites.into_iter().collect::<Option<Vec<_>>>())
        })
        .await
    }

    /// The invite every seat agrees on: all name one federation.
    fn agreed_invite(
        sessions: &[SeatSession<MgClient<F::Client>>],
        invites: Vec<InviteCode>,
    ) -> FaResult<InviteCode> {
        let mut expected = None;
        for (session, invite) in sessions.iter().zip(&invites) {
            let federation_id = invite_federation_id(invite).map_err(|_| {
                session
                    .client
                    .failure(MgOperation::GetStatus, MgCause::InvalidResponse)
            })?;
            if *expected.get_or_insert(federation_id) != federation_id {
                return Err(FaError::InvalidManifoldGuardians(
                    "selected Manifold Guardians reported different federation identities"
                        .to_owned(),
                ));
            }
        }
        invites.into_iter().next().ok_or_else(|| {
            FaError::InvalidManifoldGuardians("no Manifold Guardians selected".to_owned())
        })
    }

    /// Form the federation from the full slots. Each stage is skipped when
    /// its durable fact exists, so an interrupted formation repeats nothing.
    async fn form(
        &self,
        formation: &FormationView,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let sessions = self.seat_sessions(formation, timeouts).await?;
        if formation.invite_code.is_none() {
            self.start_dkg(formation, &sessions, fa_id, timeouts)
                .await?;
        }
        let invites = self.poll_until_running(&sessions, fa_id, timeouts).await?;
        let invite = Self::agreed_invite(&sessions, invites)?;
        match &formation.invite_code {
            Some(stored) => {
                if invite_federation_id(stored)? != invite_federation_id(&invite)? {
                    return Err(FaError::InvalidManifoldGuardians(
                        "formed federation identity changed during reconciliation".to_owned(),
                    ));
                }
            }
            None => {
                // Save DKG before publication so an interrupted run never
                // repeats it.
                self.inner
                    .store
                    .record_dkg_complete(&formation.formation_id, invite.clone())
                    .await?;
                self.publish(None).await;
            }
        }
        self.publish_seat_bindings(&sessions, formation, fa_id, &invite, timeouts)
            .await?;
        self.finish_formation(fa_id).await
    }

    /// Publish a formed federation in place of its formation.
    async fn finish_formation(&self, fa_id: FaId) -> FaResult<()> {
        let federation = self.inner.store.load_federation(fa_id).await?;
        self.inner.progress.modify(|status| {
            status.creation = None;
            status.federation = federation;
        });
        Ok(())
    }

    /// Look up the backup of a restored mnemonic and import it.
    pub(crate) async fn recover_backup(&self, fa_id: FaId, timeouts: Timeouts) -> FaResult<()> {
        let source = self
            .inner
            .backup_source
            .as_ref()
            .expect("only a client with a backup source starts in recovery");
        let root = self.inner.ports.identity.scoped_root();
        let payload = timeouts
            .long_call("reading FA backup relays", || {
                Ok(crate::backup_worker::fetch_backup(
                    &root,
                    &source.relays,
                    source.required_reads,
                ))
            })
            .await??;
        self.import_backup(payload, &source.completed, fa_id, timeouts)
            .await
    }

    /// Import a fetched backup once consensus confirms it, and mark the
    /// lookup done. The backup stays in memory until then, so a failed check
    /// leaves nothing behind.
    pub(crate) async fn import_backup(
        &self,
        payload: Option<crate::backup::FaBackupPayload>,
        completed: &crate::db::RecoveryCompletedKey,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let Some(payload) = payload else {
            self.inner.store.complete_empty_recovery(completed).await?;
            self.inner
                .progress
                .replace(crate::state::FaStatus::default());
            return Ok(());
        };
        let seats = payload
            .seats
            .iter()
            .map(|seat| crate::liquidity::FormedMgSeat {
                mg_id: seat.mg_identity,
                locator: seat.locator.clone(),
                seat_id: seat.seat_id.clone(),
            })
            .collect();
        let federation_name = self
            .verify_federation(
                &payload.federation_invite,
                seats,
                payload.liquidity.as_ref(),
                timeouts,
            )
            .await?;
        self.inner
            .store
            .import_backup(fa_id, payload, federation_name, completed)
            .await?;
        let federation = self.inner.store.load_federation(fa_id).await?;
        self.inner.progress.replace(crate::state::FaStatus {
            federation,
            ..crate::state::FaStatus::default()
        });
        Ok(())
    }

    /// Check a federation against consensus: its identity, seat-binding
    /// directory, seats, and liquidity commitment. Return its consensus name.
    async fn verify_federation(
        &self,
        invite: &InviteCode,
        seats: Vec<crate::liquidity::FormedMgSeat>,
        liquidity: Option<
            &fedi_decentralized_service_liquidity_manager::RequestLiquidityDetailsCommitmentV1,
        >,
        timeouts: Timeouts,
    ) -> FaResult<Option<crate::FederationName>> {
        if seats.is_empty() {
            return Err(FaError::InvalidManifoldGuardians(
                "restored FA backup contains no seats".to_owned(),
            ));
        }
        let consensus = self
            .read_recovery_consensus(invite, &seats, timeouts)
            .await?
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "reading restored federation consensus failed: {error}"
                ))
            })?;
        verify_consensus_identity(&consensus, invite)?;
        let federation = federation_seats(&consensus.config).map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "restored federation config is not usable: {error}"
            ))
        })?;
        let directory = crate::liquidity::liquidity_seat_bindings_field(&consensus.meta_value)?
            .ok_or_else(|| {
                FaError::InvalidManifoldGuardians(
                    "restored federation has no MG seat-binding directory".to_owned(),
                )
            })?;
        let bindings = MgSeatBindings::parse_canonical(&directory)
            .and_then(|bindings| bindings.verify_for_federation(&federation))
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "restored federation seat-binding directory is invalid: {error}"
                ))
            })?;
        let federation_name = federation_name(&consensus)?;
        let validation_name = federation_name
            .clone()
            .or_else(|| liquidity.map(|value| value.federation_details.federation_name.clone()))
            .unwrap_or_else(|| crate::FederationName(String::new()));
        let context = crate::liquidity::FormedLiquidityContext {
            federation_name: validation_name,
            invite_code: invite.clone(),
            network: consensus.network,
            federation,
            bindings,
            seats,
        };
        context.fleet_seat_hints().map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "restored seats do not match federation consensus: {error}"
            ))
        })?;
        if let Some(commitment) = liquidity {
            context.matches_commitment(commitment)?;
        }

        self.remember_read_invite(invite, &consensus).await?;
        Ok(federation_name)
    }

    async fn seat_sessions(
        &self,
        formation: &FormationView,
        timeouts: Timeouts,
    ) -> FaResult<Vec<SeatSession<MgClient<F::Client>>>> {
        let mut sessions = Vec::with_capacity(formation.seats.len());
        for seat in &formation.seats {
            let seat_id = seat.seat_id.clone();
            let client =
                MgClient::connect(&self.inner.ports.mg_connector, &seat.locator, timeouts).await?;
            sessions.push(SeatSession {
                index: seat.index,
                client,
                seat_id,
            });
        }
        Ok(sessions)
    }

    /// Publish the directory and initial fee policy as one formation vote.
    async fn publish_seat_bindings(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        formation: &FormationView,
        fa_id: FaId,
        invite: &InviteCode,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        // A new target must be validated against the final config before it is
        // durable: recovery replays its exact bytes and cannot repair an
        // invalid directory after pinning it. This read validates the signed
        // directory only; recipient derivation no longer uses API keys.
        let formation_config = if formation.meta_target.is_none() {
            // A guardian that restarts can leave a read unanswered until its
            // request timeout. The config is not readable yet, as an error.
            let config = wait_until(timeouts.poll, || async {
                match timeouts
                    .call("reading the formation config", || {
                        Ok(self.inner.ports.consensus_reader.read_consensus(invite))
                    })
                    .await
                {
                    Ok(Ok(snapshot)) => Ok(Some(snapshot.config)),
                    Ok(Err(_)) | Err(FaError::Timeout(_)) => Ok(None),
                    Err(error) => Err(error),
                }
            })
            .await?;
            Some(config)
        } else {
            None
        };
        let target = match formation.meta_target.clone() {
            Some(target) => target,
            None => {
                let (bindings, binding_entries) = self
                    .assemble_seat_bindings(sessions, formation, fa_id, timeouts)
                    .await?;
                let federation_id = invite_federation_id(invite)?;
                let fa_account = self
                    .inner
                    .ports
                    .fa_fee_account_provider
                    .formed_federation_fee_account(&federation_id)
                    .map_err(|_| {
                        FaError::CapabilityUnavailable(crate::Capability::FeeArrangement)
                    })?;
                let guardian_verification_fee_account =
                    self.inner.guardian_verification_fee_account.clone().ok_or(
                        FaError::CapabilityUnavailable(crate::Capability::FeeArrangement),
                    )?;
                let recipients = canonical_fee_recipients(
                    formation_config
                        .as_ref()
                        .expect("a new formation target has a downloaded config"),
                    &bindings,
                    fa_account.clone(),
                    guardian_verification_fee_account.clone(),
                )?;
                let recipients =
                    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
                        FaError::InvalidManifoldGuardians(format!(
                            "guardian-fee recipient accounts cannot form canonical metadata: {error}"
                        ))
                    })?;
                let send_ppm = self
                    .min_guardian_fee_ppm()
                    .await
                    .max(u64::from(GuardianFeePpm::MANIFOLD_DEFAULT.value()));
                let fa_fee_account = GuardianFeeAccount::try_from(fa_account).map_err(|error| {
                    FaError::InvalidIntent(format!("FA guardian-fee account is invalid: {error}"))
                })?;
                let guardian_verification_fee_account = GuardianFeeAccount::try_from(
                    guardian_verification_fee_account,
                )
                .map_err(|_| FaError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
                let target = FormationMetaTarget {
                    seat_bindings: bindings,
                    binding_entries,
                    fa_fee_account,
                    guardian_verification_fee_account,
                    send_ppm,
                    recipients,
                };
                timeouts
                    .call("recording the formation metadata target", || {
                        Ok(self
                            .inner
                            .store
                            .record_formation_meta_target(&formation.formation_id, target.clone()))
                    })
                    .await??;
                self.publish(None).await;
                target
            }
        };

        let formation_id = &formation.formation_id;
        let target = &target;
        let seats = sessions.iter().map(|session| session.index).collect();
        let snapshot = write_consensus_meta(
            timeouts,
            None,
            seats,
            || {
                timeouts.call("reading the formation metadata base", || {
                    Ok(self.inner.ports.consensus_reader.read_consensus(invite))
                })
            },
            |snapshot| {
                Ok(self.seat_bindings_match(snapshot, &target.seat_bindings)?
                    && guardian_fee_recipients_match(
                        snapshot.meta_value.as_deref(),
                        &target.recipients,
                    )?
                    && guardian_fee_rate_matches(snapshot.meta_value.as_deref(), target.send_ppm)?)
            },
            |rows, expected_base| async move {
                let mut votes = Vec::with_capacity(rows.len());
                for (index, result) in self
                    .submit_formation_meta_wave(
                        sessions,
                        &rows,
                        fa_id,
                        expected_base,
                        target,
                        timeouts,
                    )
                    .await
                {
                    let vote = match result {
                        Ok(_) => SeatVote::Accepted,
                        Err(FaError::Mg(MgFailure {
                            cause: MgCause::MetaConsensusChanged,
                            ..
                        })) => SeatVote::Retry("consensus metadata base changed".to_owned()),
                        Err(
                            error @ FaError::Mg(MgFailure {
                                cause:
                                    MgCause::GuardianVerificationFeeAccountUnavailable
                                    | MgCause::GuardianVerificationFeeAccountMismatch,
                                ..
                            }),
                        ) => return Err(error),
                        Err(error) => {
                            log_mg_failure(formation_id, &error, "pending_consensus");
                            SeatVote::Rejected(error)
                        }
                    };
                    votes.push((index, vote));
                }
                Ok(votes)
            },
        )
        .await
        .map_err(|error| match error {
            MetaWriteError::Fatal(error) | MetaWriteError::Rejected { reason: error, .. } => error,
            MetaWriteError::TooLarge {
                actual_bytes,
                max_bytes,
            } => formation_metadata_too_large(ConsensusMetadataTooLarge {
                actual_bytes,
                max_bytes,
            }),
            MetaWriteError::InvalidBase(reason) => FaError::InvalidManifoldGuardians(reason),
            MetaWriteError::Unconverged { .. } => {
                FaError::Timeout("waiting for formation metadata consensus".to_owned())
            }
        })?;
        timeouts
            .call("confirming formation metadata consensus", || {
                Ok(self.inner.store.finish_formation(formation_id))
            })
            .await??;
        self.remember_read_invite(invite, &snapshot).await
    }

    /// Collect one MG attestation and endpoint proof per seat.
    async fn assemble_seat_bindings(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        formation: &FormationView,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<(String, Vec<FormationSeatBinding>)> {
        let mut pending = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let expected_guardian_fee_account =
                formation.seats[position].guardian_fee_account.clone();

            pending.push(async move {
                let request = GetPeerAttestationRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                };
                let request = self.sign(&request)?;
                let response = session
                    .client
                    .call(timeouts, MgOperation::GetPeerAttestation, |client| {
                        client.get_peer_attestation(request)
                    })
                    .await?;
                if response
                    .mg_peer_attestation
                    .attestation
                    .guardian_fee_account
                    != expected_guardian_fee_account
                {
                    return Err(session
                        .client
                        .failure(MgOperation::GetPeerAttestation, MgCause::InvalidResponse)
                        .into());
                }
                Ok::<_, FaError>(FormationSeatBinding {
                    attestation: response.mg_peer_attestation,
                    endpoint_proof: response.seat_endpoint_proof,
                })
            });
        }
        let mut entries = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            entries.push(result?);
        }

        let bindings = MgSeatBindings::new(entries.iter().map(|entry| entry.attestation.clone()))
            .and_then(|bindings| bindings.canonical_string())
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "Manifold Guardian attestations do not form a valid seat-binding directory: {error}"
                ))
            })?;
        Ok((bindings, entries))
    }

    async fn submit_formation_meta_wave(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        rows: &BTreeSet<u16>,
        fa_id: FaId,
        expected_base: MetaConsensusBase,
        target: &FormationMetaTarget,
        timeouts: Timeouts,
    ) -> Vec<(u16, FaResult<ProposeFormationMetaResponse>)> {
        let mut pending = FuturesUnordered::new();
        for session in sessions
            .iter()
            .filter(|session| rows.contains(&session.index))
        {
            let vote = async move {
                let request = ProposeFormationMetaRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                    expected_base,
                    seat_bindings: target.binding_entries.clone(),
                    fa_fee_account: target.fa_fee_account.clone(),
                    guardian_verification_fee_account: target
                        .guardian_verification_fee_account
                        .clone(),
                    send_ppm: target.send_ppm,
                };
                let request = self.sign(&request)?;
                session
                    .client
                    .call(timeouts, MgOperation::ProposeFormationMeta, |client| {
                        client.propose_formation_meta(request)
                    })
                    .await
            };
            pending.push(async move { (session.index, vote.await) });
        }
        let mut results = Vec::with_capacity(rows.len());
        while let Some(result) = pending.next().await {
            results.push(result);
        }
        results
    }

    /// Submit one guarded whole-object metadata wave through the supplied seats.
    ///
    /// The primitive deliberately returns every seat result instead of
    /// short-circuiting. Formation can require all seats while maintenance can
    /// read consensus after a threshold-live partial wave and retry missing
    /// guardians under its deadline.
    pub(crate) async fn submit_meta_field_wave(
        &self,
        sessions: &[SeatSession<F::Client>],
        fa_id: FaId,
        expected_base: MetaConsensusBase,
        key: &MetaFieldKey,
        value: &MetaFieldValue,
        timeouts: Timeouts,
    ) -> Vec<(u16, Result<MetaFieldSubmission, MetaFieldSubmissionError>)> {
        let mut pending = FuturesUnordered::new();
        for session in sessions {
            pending.push(async move {
                let result = async {
                    let request = SetMetaFieldRequest {
                        ts: Timestamp(now_secs().map_err(MetaFieldSubmissionError::Driver)?),
                        fa_id,
                        seat_id: session.seat_id.clone(),
                        expected_base,
                        key: key.clone(),
                        value: value.clone(),
                    };
                    let request = self
                        .sign(&request)
                        .map_err(MetaFieldSubmissionError::Driver)?;
                    let response = timeouts
                        .call("submitting SetMetaField proposal", || {
                            Ok(session.client.set_meta_field(request))
                        })
                        .await
                        .map_err(MetaFieldSubmissionError::Driver)?;
                    match response {
                        Ok(_) => Ok(MetaFieldSubmission::Accepted),
                        Err(ManifoldGuardianError::MetaConsensusChanged) => {
                            Ok(MetaFieldSubmission::BaseChanged)
                        }
                        Err(error) => Err(MetaFieldSubmissionError::ManifoldGuardian(error)),
                    }
                }
                .await;
                (session.index, result)
            });
        }
        let mut results = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            results.push(result);
        }
        results
    }

    /// Whether consensus carries the expected directory, fully verified.
    fn seat_bindings_match(
        &self,
        snapshot: &FederationConsensusSnapshot,
        expected: &str,
    ) -> FaResult<bool> {
        let Some(value) = seat_bindings_field(&snapshot.meta_value)? else {
            return Ok(false);
        };
        if value != expected {
            return Ok(false);
        }

        // Equality alone would accept whatever this run happened to write.
        // Re-deriving the peer set from the downloaded config and verifying
        // every attestation against it is what makes the readback a check on
        // the federation rather than on our own memory.
        let seats = federation_seats(&snapshot.config).map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "previewed federation config is not usable: {error}"
            ))
        })?;
        MgSeatBindings::parse_canonical(&value)
            .and_then(|bindings| {
                bindings.verify_for_federation(&seats)?;
                Ok(())
            })
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "consensus seat-binding directory does not match the federation: {error}"
                ))
            })?;
        Ok(true)
    }

    /// Choose remembered guardian addresses for this saved invite, if available.
    /// Otherwise return the saved invite. Only connection information is reused;
    /// callers must still read fresh consensus.
    pub(crate) async fn consensus_invite(&self, saved: &InviteCode) -> InviteCode {
        self.inner
            .store
            .read_invite(saved)
            .await
            .unwrap_or_else(|| saved.clone())
    }

    /// Remember several guardian addresses from a verified consensus snapshot.
    /// Call after verifying the directory and saved authority. This checks the
    /// federation identity again before it stores the connection hint. The
    /// saved invite, backups, and signed liquidity request stay unchanged.
    async fn remember_read_invite(
        &self,
        saved: &InviteCode,
        snapshot: &FederationConsensusSnapshot,
    ) -> FaResult<()> {
        verify_consensus_identity(snapshot, saved)?;
        let peers = snapshot
            .config
            .global
            .api_endpoints
            .iter()
            .map(|(peer, endpoint)| (*peer, endpoint.url.clone()))
            .collect();
        let parsed = FedimintInviteCode::from_str(&saved.0)
            .map_err(|error| FaError::InvalidIntent(error.to_string()))?;
        let invite =
            FedimintInviteCode::from_map(&peers, parsed.federation_id(), parsed.api_secret());
        self.inner
            .store
            .record_read_invite(saved.clone(), InviteCode(invite.to_string()))
            .await
    }

    /// Read fresh consensus, trying the invites of `seats` if the first read
    /// fails. Alternative invites and returned configurations must name the
    /// saved federation. Calls share the driver deadlines, and remaining
    /// attempts are dropped once one succeeds. The caller must still verify the
    /// directory and saved authority before marking recovery fresh.
    async fn read_recovery_consensus(
        &self,
        saved: &InviteCode,
        seats: &[crate::liquidity::FormedMgSeat],
        timeouts: Timeouts,
    ) -> FaResult<Result<FederationConsensusSnapshot, crate::FederationConsensusError>> {
        let invite = self.consensus_invite(saved).await;
        let first = timeouts
            .call("reading federation recovery consensus", || {
                Ok(self.inner.ports.consensus_reader.read_consensus(&invite))
            })
            .await;
        match &first {
            Ok(Ok(_)) => return first,
            Err(error) if !matches!(error, FaError::Timeout(_)) => return first,
            _ => {}
        }

        // A manager supplies another way to dial, never a trust verdict.
        // Race the saved managers so one unavailable manager cannot hold up the rest.
        let fa_id = self.fa_id()?;
        let expected = invite_federation_id(saved)?;
        let mut pending = FuturesUnordered::new();
        for seat in seats {
            pending.push(async move {
                let client =
                    MgClient::connect(&self.inner.ports.mg_connector, &seat.locator, timeouts)
                        .await?;
                let request = self.sign(&GetInviteCodeRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: seat.seat_id.clone(),
                })?;
                let invite = client
                    .call(timeouts, MgOperation::GetInviteCode, |client| {
                        client.get_invite_code(request)
                    })
                    .await?
                    .invite_code;
                if invite_federation_id(&invite)? != expected {
                    return Err(FaError::InvalidManifoldGuardians(
                        "recovery invite names another federation".to_owned(),
                    ));
                }
                let snapshot = timeouts
                    .call("reading alternate recovery consensus", || {
                        Ok(self.inner.ports.consensus_reader.read_consensus(&invite))
                    })
                    .await?
                    .map_err(|error| FaError::InvalidManifoldGuardians(error.to_string()))?;
                verify_consensus_identity(&snapshot, saved)?;
                Ok::<_, FaError>(snapshot)
            });
        }
        while let Some(result) = pending.next().await {
            if let Ok(snapshot) = result {
                return Ok(Ok(snapshot));
            }
        }
        first
    }

    /// The running formation: a creation whose pool is full.
    pub(crate) async fn active_formation(&self, fa_id: FaId) -> FaResult<FormationView> {
        self.inner
            .store
            .load_formation(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)
    }

    pub(crate) async fn post_formed_authority(&self, fa_id: FaId) -> FaResult<PostFormedAuthority> {
        self.inner.store.post_formed_authority(fa_id).await
    }

    /// Publish the stored creation, with the error of the step that just
    /// failed, if one did.
    pub(crate) async fn publish(&self, last_error: Option<crate::FaErrorCode>) {
        let snapshot = match self.fa_id() {
            Ok(fa_id) => self.inner.store.load_snapshot(fa_id).await,
            Err(error) => Err(error),
        };
        match snapshot {
            Ok(snapshot) => self.inner.progress.modify(|status| {
                status.creation = snapshot.map(|snapshot| crate::CreationSnapshot {
                    last_error,
                    ..snapshot
                });
            }),
            Err(error) => tracing::warn!(%error, "reading the FA creation failed"),
        }
    }

    /// Drive a formation from pool seats to a formed federation.
    pub(crate) async fn drive_formation(
        &self,
        formation: FormationView,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        self.publish(None).await;
        let result = self.form(&formation, fa_id, timeouts).await;
        if let Err(error) = &result {
            log_mg_failure(&formation.formation_id, error, "returned");
            self.publish(Some(error.code())).await;
        }
        result
    }

    pub(crate) fn fa_id(&self) -> FaResult<FaId> {
        self.inner
            .ports
            .identity
            .public_key()
            .map_err(FaError::Identity)
    }

    pub(crate) fn sign<T: FaSignedRequest>(&self, request: &T) -> FaResult<SignedRequest<T>> {
        SignedRequest::create_with_signer(request, |digest| {
            self.inner.ports.identity.sign_digest(digest)
        })
        .map_err(|error| FaError::Identity(error.to_string()))
    }
}

/// Reject a configuration whose federation id differs from the saved invite.
/// This checks identity only; the caller verifies the guardian directory separately.
fn verify_consensus_identity(
    snapshot: &FederationConsensusSnapshot,
    invite: &InviteCode,
) -> FaResult<()> {
    if snapshot.config.calculate_federation_id() != invite_federation_id(invite)? {
        return Err(FaError::InvalidManifoldGuardians(
            "consensus federation identity differs from the saved invite".to_owned(),
        ));
    }
    Ok(())
}

/// A rename overrides the original name in the client config.
pub(crate) fn federation_name(
    consensus: &FederationConsensusSnapshot,
) -> FaResult<Option<crate::FederationName>> {
    let fields = consensus
        .meta_value
        .as_deref()
        .map(serde_json::from_slice::<BTreeMap<String, serde_json::Value>>)
        .transpose()
        .map_err(|error| {
            FaError::InvalidManifoldGuardians(format!("federation metadata is invalid: {error}"))
        })?
        .unwrap_or_default();
    match fields.get(FEDERATION_NAME_META_FIELD_KEY) {
        None => Ok(consensus
            .config
            .global
            .meta
            .get(FEDERATION_NAME_META_FIELD_KEY)
            .map(|name| crate::FederationName(name.clone()))),
        Some(serde_json::Value::String(value)) => Ok(Some(crate::FederationName(value.clone()))),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "federation name metadata is not a string".to_owned(),
        )),
    }
}

fn canonical_fee_recipients(
    config: &ClientConfig,
    seat_bindings: &str,
    fa_account: Account,
    guardian_verification_fee_account: Account,
) -> FaResult<Vec<GuardianFeeRecipient>> {
    let fa_fee_account = GuardianFeeAccount::try_from(fa_account.clone()).map_err(|error| {
        FaError::InvalidIntent(format!("FA guardian-fee account is invalid: {error}"))
    })?;
    let guardian_verification_fee_account =
        GuardianFeeAccount::try_from(guardian_verification_fee_account.clone())
            .map_err(|_| FaError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
    let federation = federation_seats(config).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!("formed federation config is invalid: {error}"))
    })?;
    let bindings = MgSeatBindings::parse_canonical(seat_bindings)
        .and_then(|bindings| bindings.verify_for_federation(&federation))
        .map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "seat-binding directory does not match the formed config: {error}"
            ))
        })?;

    let mut recipients = Vec::with_capacity(bindings.len() + 2);
    for binding in bindings {
        let guardian_account =
            GuardianFeeAccount::try_from(binding.guardian_fee_account).map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "guardian {} has an invalid signed guardian-fee account: {error}",
                    binding.peer_id.0
                ))
            })?;
        recipients.push(GuardianFeeRecipient::new(
            guardian_account,
            GUARDIAN_GUARDIAN_FEE_WEIGHT,
        ));
    }
    recipients.push(GuardianFeeRecipient::new(
        fa_fee_account,
        FA_GUARDIAN_FEE_WEIGHT,
    ));
    recipients.push(GuardianFeeRecipient::new(
        guardian_verification_fee_account,
        GUARDIAN_VERIFICATION_FEE_WEIGHT,
    ));
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "guardian-fee accounts do not form a canonical recipient set: {error}"
        ))
    })?;
    Ok(recipients)
}

/// Whether an MG reported that the seat ended for good.
fn service_seat_phase(status: &ServiceStatus) -> Result<SeatPhase, MgCause> {
    match status {
        ServiceStatus::New => Ok(SeatPhase::Created),
        ServiceStatus::DkgInProcess => Ok(SeatPhase::DkgUnderway),
        ServiceStatus::Running => Ok(SeatPhase::Running),
        ServiceStatus::DataLoss | ServiceStatus::Decommissioned => {
            Err(MgCause::TerminalStatus(status.clone()))
        }
    }
}

/// Run `check` every `interval` until it returns a value. `Ok(None)` means
/// the awaited remote state is not there yet, which is normal progress and
/// not a failure. An error ends the wait.
async fn wait_until<T, A: Future<Output = FaResult<Option<T>>>>(
    interval: Duration,
    mut check: impl FnMut() -> A,
) -> FaResult<T> {
    loop {
        if let Some(value) = check().await? {
            return Ok(value);
        }
        sleep(interval).await;
    }
}

/// Sleep before a retry. Fail when no time remains before the deadline of
/// the action.
pub(crate) async fn sleep_for_retry(deadline: Instant, interval: Duration) -> FaResult<()> {
    let timeout = || FaError::Timeout("waiting to retry Manifold Guardian".to_owned());
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(timeout());
    }
    sleep(interval.min(remaining)).await;
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    Ok(())
}

/// Extract the `fedi:fman_seat_bindings` string from a raw meta object.
///
/// The `meta` module holds one JSON object of fields under its default key,
/// and the directory is one string field within it. `Ok(None)` means the
/// federation carries no directory yet, which is the normal state while
/// consensus is still forming.
fn seat_bindings_field(meta_value: &Option<Vec<u8>>) -> FaResult<Option<String>> {
    let Some(fields) = parse_metadata_object(meta_value.as_deref()).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(None);
    };

    match fields.get(MG_SEAT_BINDINGS_META_FIELD_KEY) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(FaError::InvalidManifoldGuardians(format!(
            "{MG_SEAT_BINDINGS_META_FIELD_KEY} consensus metadata is not a string"
        ))),
    }
}

pub(crate) fn meta_field_matches(
    meta_value: Option<&[u8]>,
    key: &MetaFieldKey,
    expected: &MetaFieldValue,
) -> FaResult<bool> {
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusInvalid {
            reason: format!("metadata is not a JSON object: {error}"),
        }
    })?
    else {
        return Ok(false);
    };
    match fields.get(&key.0) {
        None => Ok(false),
        Some(serde_json::Value::String(value)) => Ok(value == &expected.0),
        Some(_) => Err(FaError::MaintenanceConsensusInvalid {
            reason: format!("{} consensus metadata is not a string", key.0),
        }),
    }
}

fn parse_metadata_object(
    meta_value: Option<&[u8]>,
) -> Result<Option<BTreeMap<String, serde_json::Value>>, serde_json::Error> {
    meta_value.map(serde_json::from_slice).transpose()
}

/// Pair the snapshot's raw metadata bytes with their consensus revision.
///
/// The port contract requires the reader to report both halves of one meta
/// consensus read together, so a metadata base always commits to one
/// occurrence of the board state. An unpaired combination is a broken
/// adapter; the driver refuses to guess which half to trust.
pub(crate) fn snapshot_meta_consensus(
    snapshot: &FederationConsensusSnapshot,
) -> Result<Option<(u64, &[u8])>, String> {
    match (snapshot.meta_value.as_deref(), snapshot.meta_revision) {
        (Some(value), Some(revision)) => Ok(Some((revision, value))),
        (None, None) => Ok(None),
        (Some(_), None) => Err(
            "consensus read returned metadata bytes without their consensus revision".to_owned(),
        ),
        (None, Some(_)) => {
            Err("consensus read returned a consensus revision without metadata bytes".to_owned())
        }
    }
}

pub(crate) struct ConsensusMetadataTooLarge {
    pub(crate) actual_bytes: usize,
    pub(crate) max_bytes: usize,
}

fn formation_metadata_too_large(error: ConsensusMetadataTooLarge) -> FaError {
    FaError::InvalidManifoldGuardians(format!(
        "consensus metadata is {} bytes; formation permits at most {} bytes",
        error.actual_bytes, error.max_bytes
    ))
}

pub(crate) fn validate_consensus_metadata_size(
    value: Option<&[u8]>,
) -> Result<(), ConsensusMetadataTooLarge> {
    let actual_bytes = value.map_or(0, <[u8]>::len);
    if actual_bytes > FEDERATION_METADATA_OBJECT_MAX_BYTES {
        return Err(ConsensusMetadataTooLarge {
            actual_bytes,
            max_bytes: FEDERATION_METADATA_OBJECT_MAX_BYTES,
        });
    }
    Ok(())
}

fn guardian_fee_rate_matches(meta_value: Option<&[u8]>, send_ppm: u64) -> FaResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_send_ppm") {
        Some(serde_json::Value::String(rate)) => Ok(rate == &send_ppm.to_string()),
        None => Ok(false),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "guardian-fee consensus rate metadata field is not a string".to_owned(),
        )),
    }
}

fn guardian_fee_recipients_match(meta_value: Option<&[u8]>, recipients: &str) -> FaResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_remittance_account") {
        Some(serde_json::Value::String(value)) => Ok(value == recipients),
        None => Ok(false),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "guardian-fee consensus recipients metadata field is not a string".to_owned(),
        )),
    }
}

pub(crate) fn now_secs() -> FaResult<u64> {
    Ok(fedimint_core::time::duration_since_epoch().as_secs())
}

pub(crate) fn restored_formation_id(fa_id: FaId, invite: &InviteCode) -> FaResult<FormationId> {
    let federation_id = invite_federation_id(invite)?;
    let mut digest = Sha256::new();
    digest.update(b"fedi-fi-restored-formation-id/v1\0");
    digest.update(fa_id.0.serialize());
    digest.update(federation_id.to_string().as_bytes());
    Ok(FormationId(format!(
        "restored-{}",
        hex::encode(digest.finalize())
    )))
}

fn invite_federation_id(invite: &InviteCode) -> FaResult<FedimintFederationId> {
    let parsed = FedimintInviteCode::from_str(&invite.0).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "Manifold Guardian returned an invalid federation invite: {error}"
        ))
    })?;
    if parsed.api_secret().is_some() {
        return Err(FaError::InvalidManifoldGuardians(
            "Manifold Guardian returned a bearer-secret federation invite".to_owned(),
        ));
    }
    Ok(parsed.federation_id())
}
