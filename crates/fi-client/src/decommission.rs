//! Testing-only FI-initiated decommission of the FI's own seats.
//!
//! Not a product capability, and deliberately the thinnest thing that works:
//! it sends one `DecommissionSeat` per seat of the recorded formation and
//! reports what each FMan said. It takes no driver lease, records nothing, and
//! leaves local FI state exactly as it found it — a decommissioned federation
//! still reads as `Formed` until the caller abandons or overwrites it. Every
//! FMan outside development and staging refuses the underlying verb, and the
//! seat is forfeited, exactly as an operator decommission would leave it.

use std::time::Duration;

use fedi_decentralized_nostr_clients::FiNostrClient;
use fedi_decentralized_service_fleet_manager::{DecommissionSeatRequest, FleetManagerService};
use fedimint_core::runtime::timeout;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::{
    FederationConsensusReader, FiClient, FiError, FiIdentity, FiPayments, FiResult,
    FleetManagerConnector, Timestamp,
};

/// Per-seat budget for the connect-sign-call round trip. Fixed rather than
/// configurable: this is a developer command, not a tuned driver run.
const DECOMMISSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What one decommission pass achieved, per seat index.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DecommissionOutcome {
    /// Seats this pass ended.
    pub decommissioned: Vec<u16>,
    /// Seats already terminal before this pass.
    pub already_decommissioned: Vec<u16>,
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
    /// Ask every FMan holding one of this formation's seats to decommission
    /// it.
    ///
    /// One pass, no retries and no convergence: a seat that could not be
    /// reached is reported, and its operator has to decommission it by hand.
    pub async fn decommission_seats(&self) -> FiResult<DecommissionOutcome> {
        let fi_id = self.fi_id()?;
        let recovery = self.active_recovery(fi_id).await?;
        let mut pending = FuturesUnordered::new();
        for seat in &recovery.seats {
            let index = seat.progress.index;
            // A seat row without an id was never accepted by its FMan, so
            // there is nothing on the other side to release.
            let Some(seat_id) = seat.progress.seat_id.clone() else {
                continue;
            };
            let locator = seat.progress.locator.clone();
            pending.push(async move {
                let result = timeout(DECOMMISSION_REQUEST_TIMEOUT, async {
                    let client = self
                        .inner
                        .ports
                        .fman_connector
                        .connect(&locator)
                        .await
                        .map_err(|error| error.to_string())?;
                    let request = DecommissionSeatRequest {
                        ts: Timestamp(
                            crate::formation::now_secs()
                                .map_err(|error: FiError| error.to_string())?,
                        ),
                        fi_id,
                        seat_id,
                    };
                    let request = self.sign(&request).map_err(|error| error.to_string())?;
                    client
                        .decommission_seat(request)
                        .await
                        .map_err(|error| error.to_string())
                })
                .await;
                (index, result)
            });
        }

        let mut outcome = DecommissionOutcome::default();
        while let Some((index, result)) = pending.next().await {
            match result {
                Ok(Ok(response)) if response.already_decommissioned => {
                    outcome.already_decommissioned.push(index);
                }
                Ok(Ok(_)) => outcome.decommissioned.push(index),
                Ok(Err(reason)) => outcome.refused.push((index, reason)),
                Err(_) => outcome
                    .refused
                    .push((index, "decommission request timed out".to_owned())),
            }
        }
        outcome.decommissioned.sort_unstable();
        outcome.already_decommissioned.sort_unstable();
        outcome.refused.sort_unstable();
        Ok(outcome)
    }
}
