//! FI-initiated replacement of a stuck DKG ceremony.
//!
//! Resume never sends `RestartDkg`, because it discards every guardian's
//! in-flight ceremony. This takes no driver lease and records nothing, so a
//! resume that is already polling keeps polling and observes the new ceremony.

use std::time::Duration;

use fedi_decentralized_nostr_clients::FiNostrClient;
use fedi_decentralized_service_fleet_manager::{
    FleetManagerError, FleetManagerService, RestartDkgRequest, ServiceStatus,
};
use fedimint_core::runtime::timeout;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::{
    FederationConsensusReader, FiClient, FiError, FiIdentity, FiPayments, FiResult,
    FleetManagerConnector, FormationPhase, Timestamp,
};

/// The FMan replaces the guardian and starts the ceremony before it answers,
/// so each seat needs longer than a decommission.
const RESTART_DKG_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// What one DKG restart pass achieved, per seat index.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestartDkgOutcome {
    /// Seats now running a fresh ceremony.
    pub restarted: Vec<u16>,
    /// Seats whose previous ceremony finished before the restart reached them.
    pub already_running: Vec<u16>,
    /// Seats whose FMan refused or could not be reached, with the reason.
    pub refused: Vec<(u16, String)>,
}

impl<I, P, N, F, C> FiClient<I, P, N, F, C>
where
    I: FiIdentity,
    P: FiPayments,
    N: FiNostrClient,
    F: FleetManagerConnector,
    C: FederationConsensusReader,
{
    /// Ask every FMan holding one of this formation's seats to replace its
    /// DKG ceremony with a fresh one.
    ///
    /// Only a formation waiting on DKG qualifies: every seat holds a guardian
    /// code and no invite is saved yet. Anything else returns
    /// [`FiError::NoActiveFormation`] before any FMan is contacted.
    ///
    /// One pass, no retries: a seat that could not be reached is reported,
    /// and the caller decides whether to try again.
    pub async fn restart_dkg(&self) -> FiResult<RestartDkgOutcome> {
        let fi_id = self.fi_id()?;
        let recovery = self.active_recovery(fi_id).await?;
        if !matches!(
            recovery.snapshot.phase,
            FormationPhase::PreparingDkg | FormationPhase::DkgUnderway
        ) {
            return Err(FiError::NoActiveFormation);
        }
        let seats = recovery
            .seats
            .iter()
            .map(|seat| {
                Some((
                    seat.progress.index,
                    seat.progress.locator.clone(),
                    seat.progress.seat_id.clone()?,
                    seat.progress.guardian_code.clone()?,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(FiError::NoActiveFormation)?;
        let guardian_codes = seats
            .iter()
            .map(|(_, _, _, code)| code.clone())
            .collect::<Vec<_>>();

        let mut pending = FuturesUnordered::new();
        for (index, locator, seat_id, _) in seats {
            let guardian_codes = guardian_codes.clone();
            pending.push(async move {
                let result = timeout(RESTART_DKG_REQUEST_TIMEOUT, async {
                    let client = self
                        .inner
                        .ports
                        .fman_connector
                        .connect(&locator)
                        .await
                        .map_err(|error| error.to_string())?;
                    let request = RestartDkgRequest {
                        ts: Timestamp(
                            crate::formation::now_secs()
                                .map_err(|error: FiError| error.to_string())?,
                        ),
                        fi_id,
                        seat_id,
                        guardian_codes,
                    };
                    let request = self.sign(&request).map_err(|error| error.to_string())?;
                    match client.restart_dkg(request).await {
                        Ok(response) => Ok(response.status),
                        Err(FleetManagerError::WrongState {
                            status: ServiceStatus::Running,
                        }) => Ok(ServiceStatus::Running),
                        Err(error) => Err(error.to_string()),
                    }
                })
                .await;
                (index, result)
            });
        }

        let mut outcome = RestartDkgOutcome::default();
        while let Some((index, result)) = pending.next().await {
            match result {
                Ok(Ok(ServiceStatus::DkgInProcess)) => outcome.restarted.push(index),
                Ok(Ok(ServiceStatus::Running)) => outcome.already_running.push(index),
                Ok(Ok(status)) => outcome
                    .refused
                    .push((index, format!("seat reported {status} after restart"))),
                Ok(Err(reason)) => outcome.refused.push((index, reason)),
                Err(_) => outcome
                    .refused
                    .push((index, "DKG restart request timed out".to_owned())),
            }
        }
        outcome.restarted.sort_unstable();
        outcome.already_running.sort_unstable();
        outcome.refused.sort_unstable();
        Ok(outcome)
    }
}
