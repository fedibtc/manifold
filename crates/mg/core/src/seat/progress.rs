//! Whether a formed seat keeps up with its federation.
//!
//! A seat that falls behind can catch up only by fetching session outcomes
//! from its peers, and its local API keeps answering while it cannot, so the
//! watchdog's health says nothing about it. [`SessionProgress`] watches the
//! session count the watchdog's status probe already returns. Only after that
//! count has not advanced for [`STALL_THRESHOLD`] of observed up time does the
//! seat ask its peers for theirs, and it repeats that query every
//! [`PEER_QUERY_INTERVAL`] only while the stall lasts. Up time accumulates
//! across child respawns, so a child that exits on fedimintd's session timeout
//! before reaching the threshold does not hide the stall; time with no child
//! answering does not count, so downtime before a restart catch-up does not
//! either.

use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::SeatId;
use tokio::time::Instant;

use crate::fedimint_api::{PeerQueryFailure, PeerSessionCountUnknown};

/// Observed up time without a new session before the seat asks its peers.
/// Sessions normally take 12 to 17 minutes and about 30 on a slow
/// federation; fedimintd exits a session it cannot finish after 60.
pub(crate) const STALL_THRESHOLD: Duration = Duration::from_secs(45 * 60);
/// How often a stalled seat asks its peers again.
pub(crate) const PEER_QUERY_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Peers this many sessions ahead of a stalled seat make it behind. A seat
/// that has not yet closed the session its peers just closed is one behind.
pub(crate) const BEHIND_LAG_SESSIONS: u64 = 3;

/// What a seat's session progress says about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeatSync {
    /// Advancing, or not stalled long enough to ask the peers.
    Progressing,
    /// Stalled, and the peers have not been asked yet or are fewer than
    /// [`BEHIND_LAG_SESSIONS`] ahead.
    Stalled,
    /// Stalled while the federation is ahead: this MG's fault.
    Behind { lag: u64 },
    /// Stalled while the peers are not ahead: the federation itself is not
    /// advancing, which is not this MG's fault.
    FederationNotAdvancing,
    /// Stalled, and too few peers answered to know.
    FederationUnknown(PeerSessionCountUnknown),
}

impl SeatSync {
    fn code(self) -> &'static str {
        match self {
            Self::Progressing => "progressing",
            Self::Stalled => "stalled",
            Self::Behind { .. } => "behind",
            Self::FederationNotAdvancing => "federation_not_advancing",
            Self::FederationUnknown(_) => "federation_unknown",
        }
    }
}

/// One seat's session progress, owned by its seat loop. In memory only: a
/// daemon restart starts it over.
pub(crate) struct SessionProgress {
    seat_id: SeatId,
    session_count: Option<u64>,
    /// Observed up time since the session count last changed.
    stalled: Duration,
    /// The last observation of an answering child; `None` after a gap, so
    /// the gap does not count as stalled time.
    observed_at: Option<Instant>,
    sync: SeatSync,
    next_query_at: Option<Instant>,
}

impl SessionProgress {
    pub(crate) fn new(seat_id: SeatId) -> Self {
        Self {
            seat_id,
            session_count: None,
            stalled: Duration::ZERO,
            observed_at: None,
            sync: SeatSync::Progressing,
            next_query_at: None,
        }
    }

    #[cfg(test)]
    fn sync(&self) -> SeatSync {
        self.sync
    }

    /// The child answered with `session_count` at `now`.
    pub(crate) fn observe(&mut self, session_count: u64, now: Instant) {
        if self.session_count != Some(session_count) {
            if !matches!(self.sync, SeatSync::Progressing | SeatSync::Stalled) {
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.seat_id,
                    previous_state = self.sync.code(),
                    stalled_secs = self.stalled.as_secs(),
                    session_count,
                    "seat session progress resumed"
                );
            }
            self.session_count = Some(session_count);
            self.stalled = Duration::ZERO;
            self.sync = SeatSync::Progressing;
            self.next_query_at = None;
        } else if let Some(observed_at) = self.observed_at {
            self.stalled += now.saturating_duration_since(observed_at);
        }
        self.observed_at = Some(now);
        if self.stalled >= STALL_THRESHOLD && self.sync == SeatSync::Progressing {
            self.sync = SeatSync::Stalled;
            self.next_query_at = Some(now);
        }
    }

    /// The child did not answer, or is gone: the time until it answers again
    /// is not stalled time.
    pub(crate) fn interrupt(&mut self) {
        self.observed_at = None;
    }

    /// The session count to ask the peers about, when a query is due at
    /// `now`; the caller asks and reports through [`Self::peers_answered`].
    pub(crate) fn start_query(&mut self, now: Instant) -> Option<u64> {
        let due = self.next_query_at.is_some_and(|at| at <= now);
        if !due {
            return None;
        }
        self.next_query_at = Some(now + PEER_QUERY_INTERVAL);
        self.session_count
    }

    /// Classify a stalled seat whose count was `asked_at_count` by its peers'
    /// answer. An answer about a count the seat has since left is stale.
    pub(crate) fn peers_answered(
        &mut self,
        asked_at_count: u64,
        federation: Result<u64, PeerSessionCountUnknown>,
    ) {
        if self.sync == SeatSync::Progressing || self.session_count != Some(asked_at_count) {
            return;
        }
        let seat = asked_at_count;
        let previous = self.sync;
        // The seat's own API not answering, as while a respawned child
        // starts, says nothing about its peers: it does not replace what
        // they last said.
        if let Err(unknown) = &federation
            && unknown.failure == PeerQueryFailure::LocalConfig
            && previous != SeatSync::Stalled
        {
            return;
        }
        self.sync = match federation {
            Ok(federation) if federation >= seat + BEHIND_LAG_SESSIONS => SeatSync::Behind {
                lag: federation - seat,
            },
            Ok(federation) if federation <= seat => SeatSync::FederationNotAdvancing,
            Ok(_) => SeatSync::Stalled,
            Err(unknown) => SeatSync::FederationUnknown(unknown),
        };
        let changed = match (previous, self.sync) {
            (SeatSync::FederationUnknown(a), SeatSync::FederationUnknown(b)) => {
                a.failure != b.failure
            }
            (a, b) => a != b,
        };
        if !changed {
            return;
        }
        let stalled_secs = self.stalled.as_secs();
        match (self.sync, federation) {
            (SeatSync::Behind { lag }, Ok(federation)) => tracing::warn!(
                safe_to_share = true,
                seat_id = %self.seat_id,
                lag_sessions = lag,
                seat_session_count = seat,
                federation_session_count = federation,
                stalled_secs,
                "seat is behind its federation"
            ),
            (SeatSync::FederationNotAdvancing, _) => tracing::info!(
                safe_to_share = true,
                seat_id = %self.seat_id,
                session_count = seat,
                stalled_secs,
                "seat federation is not advancing"
            ),
            (SeatSync::FederationUnknown(unknown), _) => tracing::warn!(
                safe_to_share = true,
                seat_id = %self.seat_id,
                failure_kind = unknown.failure.as_str(),
                answered_peers = unknown.answered,
                required_peers = unknown.required,
                stalled_secs,
                "seat federation session count unavailable"
            ),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn progress() -> SessionProgress {
        SessionProgress::new(SeatId::new("ab".repeat(32)).unwrap())
    }

    /// Feed one watchdog observation per minute for `minutes`, the count
    /// given per minute by `count`, and ask the peers whenever due, answering
    /// with `federation`.
    fn run(
        progress: &mut SessionProgress,
        start: Instant,
        minutes: u32,
        count: impl Fn(u32) -> u64,
        federation: impl Fn(u32) -> Result<u64, PeerSessionCountUnknown>,
    ) -> Instant {
        let mut now = start;
        for minute in 0..minutes {
            progress.observe(count(minute), now);
            if let Some(asked) = progress.start_query(now) {
                progress.peers_answered(asked, federation(minute));
            }
            now += MINUTE;
        }
        now
    }

    /// A seat one session behind its peers on a slow federation stalls and
    /// asks, but is not behind.
    #[test]
    fn one_session_behind_is_not_behind() {
        let mut seat = progress();
        run(&mut seat, Instant::now(), 120, |_| 50, |_| Ok(51));
        assert_eq!(seat.sync(), SeatSync::Stalled);
    }

    /// A restarted seat starts several sessions back and catches up at its
    /// own pace: it never stalls long enough to ask.
    #[test]
    fn catching_up_after_a_restart_is_not_behind() {
        let mut seat = progress();
        // Up and current, then down for two hours (no observations).
        let now = run(&mut seat, Instant::now(), 30, |_| 100, |_| Ok(100));
        seat.interrupt();
        let later = now + 120 * MINUTE;
        // Back at the old count, eight sessions behind a federation that
        // closes one every 15 minutes; the seat closes one every four
        // minutes until it has caught up, then keeps pace.
        let head = |minute: u32| 108 + u64::from(minute / 15);
        let asked = std::cell::Cell::new(false);
        run(
            &mut seat,
            later,
            180,
            |minute| (100 + u64::from(minute / 4)).min(head(minute)),
            |minute| {
                asked.set(true);
                Ok(head(minute))
            },
        );
        assert_eq!(seat.sync(), SeatSync::Progressing);
        assert!(!asked.get(), "a catching-up seat never asks its peers");
    }

    /// The downtime before a restart is not stalled time, but the up time of
    /// every child stuck at the same count is.
    #[test]
    fn stalled_time_adds_up_across_children_but_not_across_gaps() {
        let mut seat = progress();
        let mut now = Instant::now();
        // Three children, each up 20 minutes at the same count, with long
        // gaps between them.
        for _ in 0..2 {
            now = run(&mut seat, now, 21, |_| 7, |_| Ok(20));
            seat.interrupt();
            now += 6 * 60 * MINUTE;
        }
        assert_eq!(seat.sync(), SeatSync::Progressing, "40 minutes up so far");
        run(&mut seat, now, 6, |_| 7, |_| Ok(20));
        assert_eq!(seat.sync(), SeatSync::Behind { lag: 13 });
    }

    /// A seat stuck in one session while its peers move on is behind once it
    /// has been stuck for the threshold, not before; and it stops being
    /// behind as soon as its count moves.
    #[test]
    fn staying_behind_is_behind_after_the_threshold() {
        let mut seat = progress();
        let start = Instant::now();
        // The federation closes a session every 15 minutes.
        let federation = |minute: u32| Ok(40 + u64::from(minute / 15));
        let threshold = (STALL_THRESHOLD.as_secs() / 60) as u32;
        run(&mut seat, start, threshold, |_| 40, federation);
        assert_eq!(seat.sync(), SeatSync::Progressing);
        let now = run(
            &mut seat,
            start + MINUTE * threshold,
            1,
            |_| 40,
            |_| federation(threshold),
        );
        assert_eq!(seat.sync(), SeatSync::Behind { lag: 3 });
        // Still stuck: the lag grows with each later query.
        let now = run(
            &mut seat,
            now,
            30,
            |_| 40,
            |minute| federation(threshold + 1 + minute),
        );
        assert!(
            matches!(seat.sync(), SeatSync::Behind { lag } if lag >= 4),
            "{:?}",
            seat.sync()
        );
        seat.observe(41, now);
        assert_eq!(seat.sync(), SeatSync::Progressing);
    }

    /// A respawned child whose own API is not up yet makes the query fail
    /// locally; the seat stays behind rather than reporting that it caught up.
    #[test]
    fn a_local_query_failure_does_not_clear_behind() {
        let mut seat = progress();
        let now = run(&mut seat, Instant::now(), 46, |_| 7, |_| Ok(20));
        assert_eq!(seat.sync(), SeatSync::Behind { lag: 13 });
        let local = PeerSessionCountUnknown {
            failure: PeerQueryFailure::LocalConfig,
            answered: 0,
            required: 1,
        };
        run(&mut seat, now, 11, |_| 7, |_| Err(local));
        assert_eq!(seat.sync(), SeatSync::Behind { lag: 13 });
    }

    #[test]
    fn level_peers_mean_the_federation_is_not_advancing() {
        let mut seat = progress();
        run(&mut seat, Instant::now(), 60, |_| 9, |_| Ok(9));
        assert_eq!(seat.sync(), SeatSync::FederationNotAdvancing);
    }

    #[test]
    fn too_few_answers_leave_the_federation_unknown() {
        let unknown = PeerSessionCountUnknown {
            failure: PeerQueryFailure::Unreachable,
            answered: 1,
            required: 2,
        };
        let mut seat = progress();
        run(&mut seat, Instant::now(), 60, |_| 9, |_| Err(unknown));
        assert_eq!(seat.sync(), SeatSync::FederationUnknown(unknown));
    }

    /// The peers are asked once at the threshold and then once per interval,
    /// and an answer about a count the seat has left is dropped.
    #[test]
    fn peers_are_asked_only_while_stalled_and_at_the_interval() {
        let mut seat = progress();
        let start = Instant::now();
        let mut asks = Vec::new();
        let mut now = start;
        for minute in 0..80_u32 {
            seat.observe(5, now);
            if seat.start_query(now).is_some() {
                asks.push(minute);
            }
            now += MINUTE;
        }
        assert_eq!(asks, [45, 55, 65, 75]);
        // The seat moves on while a query is in flight.
        seat.observe(6, now);
        seat.peers_answered(5, Ok(50));
        assert_eq!(seat.sync(), SeatSync::Progressing);
    }
}
