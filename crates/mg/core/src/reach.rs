//! Operator view of reach: how long this daemon's calls to a seat's
//! federation take, and which fellow guardians each seat's `fedimintd`
//! currently reaches. Answers `ShowReach`; kept in memory, so it starts empty
//! after a restart.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use fedi_decentralized_service_manifold_guardian::SeatId;
use fedimint_core::PeerId;
use serde::Serialize;

use crate::db::now_ms;
use crate::fedimint_api::PeerLink;

/// Timing of one kind of call for one seat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CallStat {
    pub last_ms: u64,
    pub max_ms: u64,
    pub last_at_ms: u64,
    pub count: u64,
    /// Calls that failed or were abandoned before they finished.
    pub errors: u64,
    pub last_ok: bool,
}

#[derive(Debug, Serialize)]
pub struct CallRow {
    pub seat_id: SeatId,
    pub op: &'static str,
    #[serde(flatten)]
    pub stat: CallStat,
}

#[derive(Debug, Serialize)]
pub struct PeerRow {
    pub peer_id: usize,
    pub connected: bool,
    pub conn_type: Option<String>,
    pub rtt_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SeatPeers {
    pub seat_id: SeatId,
    pub peers: Vec<PeerRow>,
    /// Why the seat could not be asked, when it could not.
    pub peers_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReachReport {
    pub calls: Vec<CallRow>,
    pub seats: Vec<SeatPeers>,
}

pub fn peer_rows(links: BTreeMap<PeerId, Option<PeerLink>>) -> Vec<PeerRow> {
    links
        .into_iter()
        .map(|(peer, link)| PeerRow {
            peer_id: peer.to_usize(),
            connected: link.is_some(),
            conn_type: link.as_ref().and_then(|l| l.conn_type.clone()),
            rtt_ms: link
                .and_then(|l| l.rtt)
                .map(|rtt| u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX)),
        })
        .collect()
}

#[derive(Default)]
pub struct ReachLog {
    stats: Mutex<BTreeMap<(SeatId, &'static str), CallStat>>,
}

/// Records on drop, so a call whose caller gave up (the browser aborts at
/// 15 s) is still seen, as an unfinished one.
struct Timer<'a> {
    log: &'a ReachLog,
    seat_id: &'a SeatId,
    op: &'static str,
    started: Instant,
    ok: bool,
}

impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.log
            .record(self.seat_id, self.op, self.started.elapsed(), self.ok);
    }
}

impl ReachLog {
    /// Run `call` and note how long it took; the result passes through.
    pub async fn time<T, E>(
        &self,
        seat_id: &SeatId,
        op: &'static str,
        call: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let mut timer = Timer {
            log: self,
            seat_id,
            op,
            started: Instant::now(),
            ok: false,
        };
        let result = call.await;
        timer.ok = result.is_ok();
        result
    }

    fn record(&self, seat_id: &SeatId, op: &'static str, elapsed: Duration, ok: bool) {
        let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let at_ms = u64::try_from(now_ms()).unwrap_or(0);
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        let stat = stats.entry((seat_id.clone(), op)).or_default();
        stat.last_ms = ms;
        stat.max_ms = stat.max_ms.max(ms);
        stat.last_at_ms = at_ms;
        stat.count = stat.count.saturating_add(1);
        stat.last_ok = ok;
        if !ok {
            stat.errors = stat.errors.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> Vec<CallRow> {
        let stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats
            .iter()
            .map(|((seat_id, op), stat)| CallRow {
                seat_id: seat_id.clone(),
                op,
                stat: stat.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_service_manifold_guardian::QuoteId;

    use super::*;

    fn seat() -> SeatId {
        SeatId::from(QuoteId([7; 32]))
    }

    #[tokio::test]
    async fn records_duration_outcome_and_keeps_the_max() -> Result<(), String> {
        let log = ReachLog::default();
        let seat = seat();
        let slow = log
            .time(&seat, "op", async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                Ok::<_, String>(1)
            })
            .await;
        assert_eq!(slow, Ok(1));
        let failed = log
            .time(&seat, "op", async { Err::<(), _>("boom".to_owned()) })
            .await;
        assert_eq!(failed, Err("boom".to_owned()));

        let rows = log.snapshot();
        let [row] = rows.as_slice() else {
            return Err(format!("expected one row, got {}", rows.len()));
        };
        assert_eq!((row.stat.count, row.stat.errors), (2, 1));
        assert!(!row.stat.last_ok);
        assert!(row.stat.max_ms >= 30);
        assert!(row.stat.last_ms <= row.stat.max_ms);
        Ok(())
    }

    #[tokio::test]
    async fn a_call_dropped_before_it_finishes_counts_as_an_error() -> Result<(), String> {
        let log = ReachLog::default();
        let seat = seat();
        let never = log.time(&seat, "op", std::future::pending::<Result<(), String>>());
        let _ = tokio::time::timeout(Duration::from_millis(20), never).await;
        let rows = log.snapshot();
        let [row] = rows.as_slice() else {
            return Err(format!("expected one row, got {}", rows.len()));
        };
        assert_eq!(
            (row.stat.count, row.stat.errors, row.stat.last_ok),
            (1, 1, false)
        );
        assert!(row.stat.last_ms >= 20);
        Ok(())
    }

    #[test]
    fn peer_rows_tell_disconnected_from_connected() {
        let mut links = BTreeMap::new();
        links.insert(PeerId::from(1), None);
        links.insert(
            PeerId::from(2),
            Some(PeerLink {
                conn_type: Some("relay".to_owned()),
                rtt: Some(Duration::from_millis(250)),
            }),
        );
        let rows = peer_rows(links);
        assert_eq!(
            rows.iter()
                .map(|r| (r.peer_id, r.connected, r.rtt_ms))
                .collect::<Vec<_>>(),
            vec![(1, false, None), (2, true, Some(250))]
        );
    }
}
