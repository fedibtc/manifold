//! Guardian transfer: move one guardian of the formed federation onto a new
//! seat ([`SPEC-guardian-transfer`](../../mg/specs/SPEC-guardian-transfer.md)).
//!
//! The transfer owns its destination seat. It first buys the seat
//! (`crate::seat_purchase`), then grants the move and asks the new seat to
//! pull the guardian. The destination MG then fetches it from the source MG
//! on its own, so the FA only has to come back to finish. Finishing has two
//! steps:
//!
//! 1. Every guardian votes the moved peer's new seat binding and fee
//!    recipient into consensus. The FA confirms the result by readback.
//! 2. The source MG deletes the old guardian data.
//!
//! The local seat list changes between the two steps.
//!
//! Once a grant was sent, the destination seat stays bound to this move. The
//! source may have stopped and handed over the guardian, and only a renewed
//! grant for the same move recovers it, so the seat is never offered to
//! another guardian.

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
use crate::seat_purchase::{StoredDestinationSeat, StoredSeatPurchase};
use crate::{
    FaClient, FaError, FaId, FaIdentity, FaPayments, FaResult, FaStatus, FederationConsensusReader,
    FormationFreshness, FormationId, MaintenanceRunOptions, ManifoldGuardianConnector, MgCause,
    MgOperation, TransferDestinationApproval,
};

/// How long a grant lets the destination keep asking the source.
const GRANT_LIFETIME: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// One guardian moving from its MG onto a new seat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardianTransfer {
    /// Position of the moved guardian in the federation's seat list.
    pub seat_index: u16,
    /// MG that hosted the guardian.
    pub source_mg_id: PublicKey,
    /// Seat on the source MG.
    pub source_seat_id: SeatId,
    /// MG that receives the guardian.
    pub destination_mg_id: PublicKey,
    /// Seat on the destination MG. Absent until the purchase completes.
    pub destination_seat_id: Option<SeatId>,
    /// Last second at which the destination asks the source under the current
    /// grant. Absent until the purchase completes.
    pub expires_at: Option<Timestamp>,
    pub phase: GuardianTransferPhase,
}

/// Where a guardian transfer stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianTransferPhase {
    /// The FA buys the destination seat. A resume finishes the purchase.
    BuyingSeat,
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
    /// Identity of the transfer, and the scope of its seat payment.
    pub(crate) transfer_id: FormationId,
    pub(crate) seat_index: u16,
    #[serde(rename = "source_fman_id")]
    pub(crate) source_mg_id: PublicKey,
    pub(crate) source_locator: Locator,
    pub(crate) source_seat_id: SeatId,
    pub(crate) federation_id: FederationId,
    /// Bearer callback the destination invokes once the guardian runs.
    pub(crate) completion_callback: Option<DkgCompletionCallback>,
    pub(crate) stage: TransferStage,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum TransferStage {
    /// The destination seat is being bought. No grant exists yet.
    Buying(StoredSeatPurchase),
    /// The destination seat exists and the guardian moves onto it.
    Moving(StoredMove),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMove {
    pub(crate) destination: StoredDestinationSeat,
    /// The latest grant. The FA replaces it when it expires.
    pub(crate) grant: SignedRequest<SeatTransferGrant>,
    /// The grant's expiry, which its envelope only exposes after verification.
    pub(crate) expires_at: Timestamp,
    /// The destination's binding for the moved peer, saved before the first
    /// vote. The destination signs a new one on every request, so a resumed
    /// transfer must vote and look for the same one.
    pub(crate) binding: Option<FormationSeatBinding>,
    pub(crate) phase: StoredTransferPhase,
}

impl StoredGuardianTransfer {
    /// This transfer at the given move state.
    fn moved(&self, moving: &StoredMove) -> Self {
        Self {
            stage: TransferStage::Moving(moving.clone()),
            ..self.clone()
        }
    }

    /// What a backup keeps of this transfer: its move, while the destination
    /// exists but is not in the seat list yet. A restored FA then continues
    /// the same move instead of losing the paid seat.
    pub(crate) fn backup(&self) -> Option<crate::backup::FaBackupTransfer> {
        match &self.stage {
            TransferStage::Moving(moving)
                if moving.phase != StoredTransferPhase::RemovingSource =>
            {
                Some(crate::backup::FaBackupTransfer {
                    seat_index: self.seat_index,
                    destination: crate::backup::FaBackupSeat {
                        mg_identity: moving.destination.mg_id,
                        seat_id: moving.destination.seat_id.clone(),
                        locator: moving.destination.locator.clone(),
                    },
                })
            }
            _ => None,
        }
    }

    fn view(&self, phase: GuardianTransferPhase) -> GuardianTransfer {
        let (destination_mg_id, destination_seat_id, expires_at) = match &self.stage {
            TransferStage::Buying(purchase) => (purchase.mg_id, None, None),
            TransferStage::Moving(moving) => (
                moving.destination.mg_id,
                Some(moving.destination.seat_id.clone()),
                Some(moving.expires_at),
            ),
        };
        GuardianTransfer {
            seat_index: self.seat_index,
            source_mg_id: self.source_mg_id,
            source_seat_id: self.source_seat_id.clone(),
            destination_mg_id,
            destination_seat_id,
            expires_at,
            phase,
        }
    }

    fn public_phase(&self) -> GuardianTransferPhase {
        match &self.stage {
            TransferStage::Buying(_) => GuardianTransferPhase::BuyingSeat,
            TransferStage::Moving(moving) => match moving.phase {
                StoredTransferPhase::Pulling => GuardianTransferPhase::AwaitingSource,
                StoredTransferPhase::Binding => GuardianTransferPhase::PublishingBinding,
                StoredTransferPhase::RemovingSource => GuardianTransferPhase::RemovingSource,
            },
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
    /// Start to move the guardian at `seat_index` onto a new seat on the
    /// approved MG.
    ///
    /// The FA buys the seat, paid from `payment_federation_id` unless it is
    /// free. It then signs a grant that is valid for 14 days and asks the new
    /// seat to pull the guardian from its MG. The call advances the transfer
    /// like [`Self::resume_guardian_transfer`], but does not wait for the
    /// source. The destination keeps asking the source on its own, so the
    /// caller can return later, for example when `completion_callback`
    /// reports that the guardian runs. Only one transfer is open at a time.
    pub async fn start_guardian_transfer(
        &self,
        seat_index: u16,
        approval: TransferDestinationApproval,
        payment_federation_id: Option<FederationId>,
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
        let federation = self.transfer_federation(fa_id).await?;
        if federation.has_custody(approval.seat.mg_id, Some(approval.seat.holder)) {
            return Err(FaError::InvalidIntent(
                "the destination MG or holder hosts or hosted a peer of this federation".to_owned(),
            ));
        }
        let purchase = self.seat_purchase(approval, &federation, payment_federation_id)?;
        let transfer = StoredGuardianTransfer {
            transfer_id: FormationId(format!(
                "{}:guardian-transfer:{}",
                federation.formation_id.0,
                now_secs()?
            )),
            seat_index,
            source_mg_id: source.mg_id,
            source_locator: source.locator.clone(),
            source_seat_id: source.seat_id.clone(),
            federation_id: FederationId(
                crate::formation::invite_federation_id(&authority.invite_code)?.to_string(),
            ),
            completion_callback,
            stage: TransferStage::Buying(purchase),
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
    /// purchase or a vote that fails returns its error and stays open for a
    /// later call, unless the purchase ended without value at risk; then the
    /// transfer closes. `None` means no transfer is open.
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
        transfer: StoredGuardianTransfer,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<GuardianTransfer> {
        let lease = run.lease();
        let mut moving = match &transfer.stage {
            TransferStage::Moving(moving) => moving.clone(),
            TransferStage::Buying(purchase) => {
                let destination = self
                    .drive_seat_purchase(&transfer, purchase.clone(), fa_id, run)
                    .await?;
                let (grant, expires_at) = self.sign_grant(
                    fa_id,
                    &transfer.federation_id,
                    &transfer.source_locator,
                    &transfer.source_seat_id,
                    &destination,
                )?;
                let moving = StoredMove {
                    destination,
                    grant,
                    expires_at,
                    binding: None,
                    phase: StoredTransferPhase::Pulling,
                };
                lease
                    .write_guardian_transfer(
                        fa_id,
                        GuardianTransferWrite::Update(transfer.moved(&moving)),
                    )
                    .await?;
                moving
            }
        };
        if moving.phase == StoredTransferPhase::Pulling {
            match self
                .await_pulled_guardian(&transfer, &mut moving, fa_id, wait, run)
                .await?
            {
                Pulled::Running => {
                    moving.phase = StoredTransferPhase::Binding;
                    lease
                        .write_guardian_transfer(
                            fa_id,
                            GuardianTransferWrite::Update(transfer.moved(&moving)),
                        )
                        .await?;
                }
                Pulled::Waiting => {
                    return Ok(transfer
                        .moved(&moving)
                        .view(GuardianTransferPhase::AwaitingSource));
                }
            }
        }
        if moving.phase == StoredTransferPhase::Binding {
            // The seat list may be stale after a reopen: consensus can name
            // the destination already. The readback below confirms the moved
            // seat itself, so the transfer does not wait for a full
            // reconciliation, which would refuse the destination.
            let authority = self.post_formed_authority(fa_id).await?;
            let binding = match moving.binding.clone() {
                Some(binding) => binding,
                None => {
                    let binding = self
                        .destination_binding(&moving.destination, fa_id, run)
                        .await?;
                    moving.binding = Some(binding.clone());
                    lease
                        .write_guardian_transfer(
                            fa_id,
                            GuardianTransferWrite::Update(transfer.moved(&moving)),
                        )
                        .await?;
                    binding
                }
            };
            self.publish_moved_binding(
                &transfer,
                &moving.destination,
                &binding,
                &authority,
                fa_id,
                run,
            )
            .await?;
            let was_fresh = !matches!(
                self.status(),
                FaStatus::Restored(snapshot) if snapshot.freshness != FormationFreshness::Fresh
            );
            moving.phase = StoredTransferPhase::RemovingSource;
            lease
                .write_guardian_transfer(
                    fa_id,
                    GuardianTransferWrite::Rebind(transfer.moved(&moving)),
                )
                .await?;
            let mut status = self.inner.store.load_status(fa_id).await?;
            if was_fresh && let FaStatus::Restored(snapshot) = &mut status {
                snapshot.freshness = FormationFreshness::Fresh;
            }
            self.inner.progress.send_replace(status);
        }
        let transfer = transfer.moved(&moving);
        // The source seat is the last other copy of the guardian. Remove it
        // only right after the destination says again that it runs it, on
        // every attempt: a binding saved or published earlier says nothing
        // about the destination now.
        if !self
            .destination_runs(&moving.destination, fa_id, run)
            .await?
        {
            return Ok(transfer.view(GuardianTransferPhase::RemovingSource));
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

    /// Ask the destination to pull, with a new grant once the last one
    /// expired. With `wait`, poll it until the guardian runs or the run ends.
    /// An unreachable destination counts as still waiting.
    async fn await_pulled_guardian(
        &self,
        transfer: &StoredGuardianTransfer,
        moving: &mut StoredMove,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<Pulled> {
        match self
            .poll_pulled_guardian(transfer, moving, fa_id, wait, run)
            .await
        {
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
        moving: &mut StoredMove,
        fa_id: FaId,
        wait: bool,
        run: DriverRun<'_>,
    ) -> FaResult<Pulled> {
        let destination = moving.destination.clone();
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
                // An expired grant does not mean that the source kept the
                // guardian: its answer may have been lost. Only a new grant
                // for the same move can finish it.
                ServiceStatus::New | ServiceStatus::TransferGrantExpired => {
                    if moving.expires_at.0 <= now_secs()? {
                        (moving.grant, moving.expires_at) = self.sign_grant(
                            fa_id,
                            &transfer.federation_id,
                            &transfer.source_locator,
                            &transfer.source_seat_id,
                            &destination,
                        )?;
                        run.lease()
                            .write_guardian_transfer(
                                fa_id,
                                GuardianTransferWrite::Update(transfer.moved(moving)),
                            )
                            .await?;
                    }
                    let request = self.sign(&PullSeatRequest {
                        ts: Timestamp(now_secs()?),
                        fa_id,
                        seat_id: destination.seat_id.clone(),
                        grant: moving.grant.clone(),
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

    /// Put a transfer's destination in place of its source when consensus
    /// shows the move done but the seat list still names the source. That
    /// happens when this device lost the run before it recorded the move, or
    /// restored a backup taken before then.
    pub(crate) async fn adopt_transferred_seat(
        &self,
        fa_id: FaId,
        payload: crate::backup::FaBackupPayload,
        bindings: &[fedi_decentralized_domain::VerifiedSeatBinding],
        run: DriverRun<'_>,
    ) -> FaResult<crate::backup::FaBackupPayload> {
        let open = self.inner.store.load_guardian_transfer().await?;
        let (seat_index, destination, open) = match (open, &payload.transfer) {
            (Some(transfer), _) => match &transfer.stage {
                TransferStage::Moving(moving) => (
                    transfer.seat_index,
                    moving.destination.clone(),
                    Some(transfer.clone()),
                ),
                TransferStage::Buying(_) => return Ok(payload),
            },
            (None, None) => return Ok(payload),
            (None, Some(backed_up)) => (
                backed_up.seat_index,
                restored_destination(&backed_up.destination),
                None,
            ),
        };
        let source = payload.seats.get(usize::from(seat_index)).ok_or_else(|| {
            FaError::Storage("the transferred seat index is out of range".to_owned())
        })?;
        let named = |mg_id: PublicKey| {
            bindings
                .iter()
                .any(|binding| binding.mg_pubkey.0 == mg_id.to_string())
        };
        if named(source.mg_identity) || !named(destination.mg_id) {
            return Ok(payload);
        }
        match open {
            // This device still drives the move: advance it with the seat, so
            // its row no longer backs up a destination the seat list names,
            // and it still removes the source.
            Some(transfer) => {
                let TransferStage::Moving(mut moving) = transfer.stage.clone() else {
                    unreachable!("matched a moving transfer above");
                };
                moving.phase = StoredTransferPhase::RemovingSource;
                run.lease()
                    .write_guardian_transfer(
                        fa_id,
                        GuardianTransferWrite::Rebind(transfer.moved(&moving)),
                    )
                    .await?;
            }
            None => {
                self.inner
                    .store
                    .adopt_transferred_seat(fa_id, seat_index, &source.seat_id, &destination)
                    .await?;
            }
        }
        self.inner.store.restored_backup_payload(fa_id).await
    }

    /// Open the backed-up transfer again on a restored FA, with a new grant
    /// for the same move. The old grant may have reached the source, so only
    /// this move can bring the guardian back.
    pub(crate) async fn reopen_backed_up_transfer(
        &self,
        fa_id: FaId,
        snapshot: &crate::RestoredFormationSnapshot,
        payload: &crate::backup::FaBackupPayload,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let Some(backed_up) = &payload.transfer else {
            return Ok(());
        };
        if self.inner.store.load_guardian_transfer().await?.is_some() {
            return Ok(());
        }
        let source = payload
            .seats
            .get(usize::from(backed_up.seat_index))
            .ok_or_else(|| {
                FaError::Storage("the transferred seat index is out of range".to_owned())
            })?;
        let federation_id = FederationId(
            crate::formation::invite_federation_id(&snapshot.federation_invite)?.to_string(),
        );
        let destination = restored_destination(&backed_up.destination);
        let (grant, expires_at) = self.sign_grant(
            fa_id,
            &federation_id,
            &source.locator,
            &source.seat_id,
            &destination,
        )?;
        let transfer = StoredGuardianTransfer {
            transfer_id: FormationId(format!(
                "{}:guardian-transfer:{}",
                snapshot.formation_id.0,
                now_secs()?
            )),
            seat_index: backed_up.seat_index,
            source_mg_id: source.mg_identity,
            source_locator: source.locator.clone(),
            source_seat_id: source.seat_id.clone(),
            federation_id,
            completion_callback: None,
            stage: TransferStage::Moving(StoredMove {
                destination,
                grant,
                expires_at,
                binding: None,
                phase: StoredTransferPhase::Pulling,
            }),
        };
        run.lease()
            .write_guardian_transfer(fa_id, GuardianTransferWrite::Begin(transfer))
            .await
    }

    /// Fetch and check the destination's binding for the moved peer.
    async fn destination_binding(
        &self,
        destination: &StoredDestinationSeat,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<FormationSeatBinding> {
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
        destination: &StoredDestinationSeat,
        binding: &FormationSeatBinding,
        authority: &PostFormedAuthority,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        // Every current guardian votes, with the destination in place of the
        // stopped source.
        let mut voters = authority.clone();
        for seat in &mut voters.seats {
            if seat.index == transfer.seat_index {
                *seat = PostFormedSeat {
                    index: seat.index,
                    mg_id: destination.mg_id,
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

    /// Sign a grant for the move, valid for [`GRANT_LIFETIME`] from now.
    pub(crate) fn sign_grant(
        &self,
        fa_id: FaId,
        federation_id: &FederationId,
        source: &Locator,
        source_seat_id: &SeatId,
        destination: &StoredDestinationSeat,
    ) -> FaResult<(SignedRequest<SeatTransferGrant>, Timestamp)> {
        let now = now_secs()?;
        let expires_at = Timestamp(now + GRANT_LIFETIME.as_secs());
        let grant = self.sign(&SeatTransferGrant {
            ts: Timestamp(now),
            fa_id,
            federation_id: federation_id.clone(),
            source: source.clone(),
            source_seat_id: source_seat_id.clone(),
            destination_service_pubkey: destination.locator.service_pubkey,
            destination_seat_id: destination.seat_id.clone(),
            expires_at,
        })?;
        Ok((grant, expires_at))
    }

    /// Whether the destination reports the moved guardian running now. An
    /// unreachable destination does not.
    async fn destination_runs(
        &self,
        destination: &StoredDestinationSeat,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<bool> {
        let client = match MgClient::connect(
            &self.inner.ports.mg_connector,
            &destination.locator,
            run,
        )
        .await
        {
            Ok(client) => client,
            Err(FaError::Mg(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        let request = self.sign(&GetStatusRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            seat_id: destination.seat_id.clone(),
        })?;
        match client
            .call(run, MgOperation::GetStatus, |client| {
                client.get_status(request)
            })
            .await
        {
            Ok(response) => Ok(response.status == ServiceStatus::Running),
            Err(FaError::Mg(failure))
                if matches!(failure.cause, MgCause::Timeout | MgCause::Transport) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
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

/// A backed-up destination, without the acceptance and holder a backup omits.
fn restored_destination(seat: &crate::backup::FaBackupSeat) -> StoredDestinationSeat {
    StoredDestinationSeat {
        mg_id: seat.mg_identity,
        locator: seat.locator.clone(),
        seat_id: seat.seat_id.clone(),
        guardian_fee_account: None,
        holder: None,
    }
}

enum Pulled {
    Running,
    Waiting,
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
