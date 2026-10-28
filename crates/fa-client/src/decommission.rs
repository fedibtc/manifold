//! Testing-only FA-initiated decommission of the FA's own seats.
//!
//! Not a product capability, and deliberately the thinnest thing that works:
//! it sends one `DecommissionSeat` per recorded seat and
//! reports what each MG said. It takes no driver lease, records nothing, and
//! leaves local FA state exactly as it found it — a decommissioned federation
//! still reads as formed until the caller overwrites it. Every
//! MG outside development and staging refuses the underlying verb, and the
//! seat is forfeited, exactly as an operator decommission would leave it.

use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    DecommissionSeatRequest, ManifoldGuardianService,
};
use fedimint_core::runtime::timeout;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::db::FaRecovery;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    ManifoldGuardianConnector, Timestamp,
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
    /// Seats whose MG refused or could not be reached, with the reason.
    pub refused: Vec<(u16, String)>,
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Ask every MG holding one of this formation's seats to decommission
    /// it.
    ///
    /// One pass, no retries and no convergence: a seat that could not be
    /// reached is reported, and its operator has to decommission it by hand.
    pub async fn decommission_seats(&self) -> FaResult<DecommissionOutcome> {
        let fa_id = self.fa_id()?;
        let seats = match self.inner.store.load_recovery(fa_id).await? {
            FaRecovery::Idle => return Err(FaError::NoActiveFormation),
            // A seat row without an id was never accepted by its MG, so
            // there is nothing on the other side to release.
            FaRecovery::Formation(recovery) => recovery
                .seats
                .into_iter()
                .filter_map(|seat| {
                    let seat_id = seat.progress.seat_id?;
                    Some((seat.progress.index, seat.progress.locator, seat_id))
                })
                .collect::<Vec<_>>(),
            FaRecovery::Formed(_) => self
                .post_formed_authority(fa_id)
                .await?
                .seats
                .into_iter()
                .map(|seat| (seat.index, seat.locator, seat.seat_id))
                .collect(),
        };
        let mut pending = FuturesUnordered::new();
        for (index, locator, seat_id) in seats {
            pending.push(async move {
                let result = timeout(DECOMMISSION_REQUEST_TIMEOUT, async {
                    let client = self
                        .inner
                        .ports
                        .mg_connector
                        .connect(&locator)
                        .await
                        .map_err(|error| error.to_string())?;
                    let request = DecommissionSeatRequest {
                        ts: Timestamp(
                            crate::formation::now_secs()
                                .map_err(|error: FaError| error.to_string())?,
                        ),
                        fa_id,
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
