//! FA Stats for the formed federation.
//!
//! Each guardian's MG reports the figures as its own guardian sees them
//! (`GetFedimintStats`). The FA asks every guardian and combines the
//! answers per figure: the median of the answers that cover the whole
//! window, once at least 2f+1 guardians gave one. Then at least f+1 of them
//! are honest, so up to f dishonest answers cannot move the median outside
//! the honest ones. With fewer, the figure is still shown but marked
//! partial. Active-wallet comparisons instead require a quorum of complete,
//! date-matched pairs, with both medians taken over the same guardians.

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
    /// The active-wallet comparison, if enough guardians reported a complete
    /// pair for the current UTC day and the day 30 dates earlier.
    pub active_wallets_comparison: Option<ActiveWalletComparison>,
    /// Lightning and on-chain volume over the last 1008 blocks (about 7 days),
    /// in millisatoshis.
    pub volume_1008_blocks_msats: CombinedFigure,
    /// Lightning and on-chain volume over the last 4320 blocks (about 30
    /// days), in millisatoshis.
    pub volume_4320_blocks_msats: CombinedFigure,
}

/// One figure combined from the guardians' answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CombinedFigure {
    /// The median of the answers it is based on.
    pub value: u64,
    /// How many guardians' answers the value is the median of.
    pub based_on: u16,
    /// Fewer than 2f+1 guardians answered for the whole window, so the value
    /// is the median of every answer, including partial ones.
    pub partial: bool,
}

/// A 30-day active-wallet comparison over the same complete guardian answers
/// on both dates. The current count can differ from the standalone figure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveWalletComparison {
    /// Date of the current count, as UTC days since the Unix epoch.
    pub utc_day: u64,
    /// Lower median of the current 30-day active-wallet counts.
    pub current_30d: u64,
    /// Lower median of the counts from 30 UTC dates earlier.
    pub previous_30d: u64,
    /// Number of guardians that contributed to both counts (at least 2f+1).
    pub based_on: u16,
}

impl ActiveWalletComparison {
    /// Signed change in active wallets between the two dates.
    pub fn delta(&self) -> i128 {
        i128::from(self.current_30d) - i128::from(self.previous_30d)
    }

    /// Percentage change relative to the earlier count, undefined at zero.
    pub fn delta_percent(&self) -> Option<f64> {
        if self.previous_30d == 0 {
            return None;
        }
        Some(self.delta() as f64 / self.previous_30d as f64 * 100.0)
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
        let guardians =
            u16::try_from(authority.seats.len()).expect("a federation's seat count fits u16");
        // One day for the whole fan-out, even if replies straddle midnight.
        let utc_day = crate::formation::now_secs()? / (24 * 60 * 60);
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
        combine_stats(guardians, &answers, utc_day).ok_or(FaError::FederationStatsUnavailable {
            unsupported,
            failed,
        })
    }
}

/// Combine the guardians' answers, or `None` when there are none.
pub(crate) fn combine_stats(
    guardians: u16,
    answers: &[FedimintStats],
    utc_day: u64,
) -> Option<FederationStats> {
    if answers.is_empty() {
        return None;
    }
    // 2f+1 answers hold at least f+1 honest ones, which bound the median.
    let needed = 2 * NumPeers::from(usize::from(guardians)).max_evil() + 1;
    let figure = |read: fn(&FedimintStats) -> WindowedFigure| {
        combine(answers.iter().map(read).collect(), needed)
    };
    Some(FederationStats {
        guardians,
        // Custody is the current balance, so every answer covers it.
        custodied_msats: figure(|stats| WindowedFigure {
            value: stats.custodied_msats,
            complete: true,
        }),
        active_wallets_30d: figure(|stats| stats.active_wallets_30d),
        active_wallets_comparison: combine_active_wallet_history(answers, utc_day, needed),
        volume_1008_blocks_msats: figure(|stats| stats.volume_1008_blocks_msats),
        volume_4320_blocks_msats: figure(|stats| stats.volume_4320_blocks_msats),
    })
}

fn combine_active_wallet_history(
    answers: &[FedimintStats],
    utc_day: u64,
    needed: usize,
) -> Option<ActiveWalletComparison> {
    let (current, previous): (Vec<_>, Vec<_>) = answers
        .iter()
        .filter_map(|stats| {
            let history = stats.active_wallets_history?;
            let previous = history.previous_30d?;
            let current = stats.active_wallets_30d;
            (history.utc_day == utc_day && current.complete && previous.complete)
                .then_some((current.value, previous.value))
        })
        .unzip();
    if current.len() < needed {
        return None;
    }
    Some(ActiveWalletComparison {
        utc_day,
        based_on: u16::try_from(current.len()).expect("one answer per guardian fits u16"),
        current_30d: lower_median(current),
        previous_30d: lower_median(previous),
    })
}

/// The median of the complete answers when at least `needed` gave one,
/// otherwise the median of all of them, marked partial.
fn combine(figures: Vec<WindowedFigure>, needed: usize) -> CombinedFigure {
    let complete = figures
        .iter()
        .filter(|figure| figure.complete)
        .map(|figure| figure.value)
        .collect::<Vec<_>>();
    let (values, partial) = if complete.len() >= needed {
        (complete, false)
    } else {
        (figures.iter().map(|figure| figure.value).collect(), true)
    };
    CombinedFigure {
        based_on: u16::try_from(values.len()).expect("one answer per guardian fits u16"),
        value: lower_median(values),
        partial,
    }
}

/// The lower of the two middle values for an even count, so the result is
/// always a value some guardian reported.
fn lower_median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values[(values.len() - 1) / 2]
}
