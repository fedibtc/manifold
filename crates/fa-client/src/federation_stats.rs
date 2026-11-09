//! FA Stats for the formed federation.
//!
//! Each guardian's MG reports the figures as its own guardian sees them
//! (`GetFedimintStats`). The FA asks every guardian and combines the
//! answers per figure: the median of the answers that cover the whole
//! window, once at least f+1 guardians gave one. With fewer, a dishonest
//! minority could set the value, so the figure is still shown but marked
//! partial.

use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    FEDIMINT_STATS_API_VERSION, FedimintStats, GetFedimintStatsRequest, GetVersionRequest,
    ManifoldGuardianError, ManifoldGuardianService, WindowedFigure,
};
use fedimint_core::NumPeers;
use fedimint_core::runtime::timeout;
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    ManifoldGuardianConnector, Timestamp,
};

/// Per-guardian budget for the connect-sign-call round trip. Each MG reads
/// several `fedimintd` endpoints to answer.
const STATS_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// FA Stats for the formed federation, combined from its guardians' answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FederationStats {
    /// Guardians in the federation: the M of "based on N of M".
    pub guardians: u16,
    /// The bitcoin the federation holds on chain, in millisatoshis.
    pub custodied_msats: CombinedFigure,
    /// Client wallets that refreshed their ecash backup in the last 30 days.
    pub active_wallets_30d: CombinedFigure,
    /// Lightning and on-chain volume over the last 7 days, in millisatoshis.
    pub volume_7d_msats: CombinedFigure,
    /// Lightning and on-chain volume over the last 30 days, in millisatoshis.
    pub volume_30d_msats: CombinedFigure,
}

/// One figure combined from the guardians' answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CombinedFigure {
    /// The median of the answers it is based on.
    pub value: u64,
    /// How many guardians' answers the value is the median of.
    pub based_on: u16,
    /// Fewer than f+1 guardians answered for the whole window, so the value
    /// is the median of every answer, including partial ones.
    pub partial: bool,
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Ask every guardian's MG for its FA Stats and combine the answers.
    ///
    /// One pass with no retries. Each MG's API version says whether it
    /// serves stats. A guardian that cannot be reached, errors, or runs an
    /// older MG gives no answer. Fails only when no
    /// guardian answered, saying which guardians lack the verb and which
    /// failed, so a consumer can tell "update your guardians" from "retry".
    pub async fn federation_stats(&self) -> FaResult<FederationStats> {
        let fa_id = self.fa_id()?;
        let authority = self.maintenance_authority(fa_id).await?;
        let guardians = u16::try_from(authority.seats.len())
            .map_err(|_| FaError::Storage("guardian count does not fit u16".to_owned()))?;
        let mut pending = FuturesUnordered::new();
        for seat in &authority.seats {
            pending.push(async move {
                let result = timeout(STATS_REQUEST_TIMEOUT, async {
                    let client = self
                        .inner
                        .ports
                        .mg_connector
                        .connect(&seat.locator)
                        .await
                        .map_err(|error| error.to_string())?;
                    let request = GetFedimintStatsRequest {
                        ts: Timestamp(
                            crate::formation::now_secs()
                                .map_err(|error: FaError| error.to_string())?,
                        ),
                        fa_id,
                        seat_id: seat.seat_id.clone(),
                    };
                    // An MG from before `GetVersion` predates the stats too.
                    match client.get_version(GetVersionRequest).await {
                        Ok(version) if version.supports(FEDIMINT_STATS_API_VERSION) => {}
                        Ok(_) | Err(ManifoldGuardianError::UnsupportedVerb { .. }) => {
                            return Ok(None);
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                    let request = self.sign(&request).map_err(|error| error.to_string())?;
                    client
                        .get_fedimint_stats(request)
                        .await
                        .map(|response| Some(response.stats))
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|_| Err("request timed out".to_owned()));
                (seat.index, result)
            });
        }
        let mut answers = Vec::new();
        let mut unsupported = Vec::new();
        let mut failed = Vec::new();
        while let Some((index, result)) = pending.next().await {
            match result {
                Ok(Some(stats)) => answers.push(stats),
                Ok(None) => unsupported.push(index),
                Err(message) => failed.push((index, message)),
            }
        }
        unsupported.sort_unstable();
        failed.sort();
        combine_stats(guardians, &answers).ok_or(FaError::FederationStatsUnavailable {
            unsupported,
            failed,
        })
    }
}

/// Combine the guardians' answers, or `None` when there are none.
pub(crate) fn combine_stats(guardians: u16, answers: &[FedimintStats]) -> Option<FederationStats> {
    if answers.is_empty() {
        return None;
    }
    let max_evil = NumPeers::from(usize::from(guardians)).max_evil();
    let figure = |read: fn(&FedimintStats) -> WindowedFigure| {
        combine(answers.iter().map(read).collect(), max_evil)
    };
    Some(FederationStats {
        guardians,
        // Custody is the current balance, so every answer covers it.
        custodied_msats: figure(|stats| WindowedFigure {
            value: stats.custodied_msats,
            complete: true,
        }),
        active_wallets_30d: figure(|stats| stats.active_wallets_30d),
        volume_7d_msats: figure(|stats| stats.volume_7d_msats),
        volume_30d_msats: figure(|stats| stats.volume_30d_msats),
    })
}

/// The median of the complete answers when more than `max_evil` gave one,
/// otherwise the median of all of them, marked partial.
fn combine(figures: Vec<WindowedFigure>, max_evil: usize) -> CombinedFigure {
    let complete = figures
        .iter()
        .filter(|figure| figure.complete)
        .map(|figure| figure.value)
        .collect::<Vec<_>>();
    let (values, partial) = if complete.len() > max_evil {
        (complete, false)
    } else {
        (figures.iter().map(|figure| figure.value).collect(), true)
    };
    CombinedFigure {
        value: lower_median(values.clone()),
        based_on: u16::try_from(values.len()).expect("one answer per guardian fits u16"),
        partial,
    }
}

/// The lower of the two middle values for an even count, so the result is
/// always a value some guardian reported.
fn lower_median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values[(values.len() - 1) / 2]
}
