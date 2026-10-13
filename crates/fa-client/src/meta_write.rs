//! One guarded consensus-metadata write, shared by formation and maintenance.
//!
//! The write reads the consensus base, votes through the seats that have not
//! accepted that base, reads back, and retries until consensus carries the
//! target or the run ends. Callers supply the target predicate and the vote;
//! the base, row, readback, and backoff bookkeeping lives here once.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use fedi_decentralized_service_manifold_guardian::MetaConsensusBase;
use fedimint_core::runtime::Instant;

use crate::formation::{
    DriverRun, sleep_for_retry, snapshot_meta_consensus, validate_consensus_metadata_size,
};
use crate::maintenance::next_maintenance_retry_delay;
use crate::{FaError, FaResult, FederationConsensusError, FederationConsensusSnapshot};

/// One guardian's answer to a base-bound metadata vote.
pub(crate) enum SeatVote<R> {
    /// The guardian submitted this exact base-bound mutation.
    Accepted,
    /// A retryable failure: vote through this seat again on the same base.
    Retry(String),
    /// The guardian pinned this base to a different target. Same-base votes
    /// are refused work, so the seat waits for a new base.
    Pinned(String),
    /// A refusal returned unless the following readback shows adoption.
    Rejected(R),
}

/// Why a metadata write stopped without consensus adopting its target.
pub(crate) enum MetaWriteError<R> {
    /// Local driver, storage, or predicate failure.
    Fatal(FaError),
    /// A guardian refused and the readback did not show adoption.
    Rejected { index: u16, reason: R },
    /// Consensus metadata is larger than the shared limit.
    TooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
    /// Consensus metadata does not form a usable base.
    InvalidBase(String),
    /// The run ended before consensus adopted the target.
    Unconverged {
        unresolved: Vec<u16>,
        guardian_errors: Vec<(u16, String)>,
        consensus_error: Option<String>,
    },
}

impl<R> From<FaError> for MetaWriteError<R> {
    fn from(error: FaError) -> Self {
        Self::Fatal(error)
    }
}

/// Write one metadata target through `seats` and return the consensus
/// snapshot that carries it.
///
/// An already adopted target succeeds without a vote. A seat that accepted
/// the active base is not asked again until the base changes, because every
/// guardian must sign a rebased mutation. A readback follows every wave, so a
/// threshold can adopt the target while a late sibling fails.
pub(crate) async fn write_consensus_meta<R, ReadFut, SubmitFut>(
    run: DriverRun<'_>,
    seats: BTreeSet<u16>,
    read: impl Fn() -> ReadFut,
    adopted: impl Fn(&FederationConsensusSnapshot) -> FaResult<bool>,
    submit: impl Fn(BTreeSet<u16>, MetaConsensusBase) -> SubmitFut,
) -> Result<FederationConsensusSnapshot, MetaWriteError<R>>
where
    ReadFut:
        Future<Output = FaResult<Result<FederationConsensusSnapshot, FederationConsensusError>>>,
    SubmitFut: Future<Output = FaResult<Vec<(u16, SeatVote<R>)>>>,
{
    let mut active_base = None;
    let mut unresolved = seats.clone();
    let mut guardian_errors = BTreeMap::new();
    let mut pinned = BTreeSet::new();
    let mut consensus_error = None;
    let mut retry_delay = run.poll_interval();
    let unconverged = |unresolved: &BTreeSet<u16>,
                       guardian_errors: &BTreeMap<u16, String>,
                       consensus_error: Option<String>| {
        MetaWriteError::Unconverged {
            unresolved: unresolved.iter().copied().collect(),
            guardian_errors: guardian_errors
                .iter()
                .map(|(index, message)| (*index, message.clone()))
                .collect(),
            consensus_error,
        }
    };

    loop {
        if Instant::now() >= run.deadline() {
            return Err(unconverged(&unresolved, &guardian_errors, consensus_error));
        }
        let snapshot = match read().await {
            Ok(Ok(snapshot)) => Some(snapshot),
            Ok(Err(error)) => {
                consensus_error = Some(error.to_string());
                None
            }
            Err(FaError::Timeout(message)) => {
                consensus_error = Some(message);
                None
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(snapshot) = snapshot {
            consensus_error = None;
            if check_adopted(&snapshot, &adopted)? {
                return Ok(snapshot);
            }
            let expected_base = consensus_base(&snapshot)?;
            if active_base != Some(expected_base) {
                active_base = Some(expected_base);
                unresolved.clone_from(&seats);
                guardian_errors.clear();
                pinned.clear();
                retry_delay = run.poll_interval();
            }

            let rows = unresolved.difference(&pinned).copied().collect();
            let mut rejections = BTreeMap::new();
            for (index, vote) in submit(rows, expected_base).await? {
                match vote {
                    SeatVote::Accepted => {
                        unresolved.remove(&index);
                        guardian_errors.remove(&index);
                    }
                    SeatVote::Retry(message) => {
                        guardian_errors.insert(index, message);
                    }
                    SeatVote::Pinned(message) => {
                        pinned.insert(index);
                        guardian_errors.insert(index, message);
                    }
                    SeatVote::Rejected(reason) => {
                        rejections.insert(index, reason);
                    }
                }
            }

            match read().await {
                Ok(Ok(snapshot)) => {
                    if check_adopted(&snapshot, &adopted)? {
                        return Ok(snapshot);
                    }
                    if consensus_base(&snapshot)? != expected_base {
                        active_base = None;
                        continue;
                    }
                }
                Ok(Err(error)) => consensus_error = Some(error.to_string()),
                Err(FaError::Timeout(message)) => consensus_error = Some(message),
                Err(error) => return Err(error.into()),
            }
            if let Some((index, reason)) = rejections.into_iter().next() {
                return Err(MetaWriteError::Rejected { index, reason });
            }
        }

        if sleep_for_retry(run.deadline(), retry_delay).await.is_err() {
            return Err(unconverged(&unresolved, &guardian_errors, consensus_error));
        }
        retry_delay = next_maintenance_retry_delay(run.poll_interval(), retry_delay);
    }
}

fn check_adopted<R>(
    snapshot: &FederationConsensusSnapshot,
    adopted: &impl Fn(&FederationConsensusSnapshot) -> FaResult<bool>,
) -> Result<bool, MetaWriteError<R>> {
    validate_consensus_metadata_size(snapshot.meta_value.as_deref()).map_err(|error| {
        MetaWriteError::TooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    Ok(adopted(snapshot)?)
}

fn consensus_base<R>(
    snapshot: &FederationConsensusSnapshot,
) -> Result<MetaConsensusBase, MetaWriteError<R>> {
    snapshot_meta_consensus(snapshot)
        .map(MetaConsensusBase::from_consensus)
        .map_err(MetaWriteError::InvalidBase)
}
