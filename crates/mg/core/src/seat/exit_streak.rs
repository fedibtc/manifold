//! Consecutive exits of a seat's child with the same status.
//!
//! Each exit already has its own event; a run of them names the loop, so a
//! child that fedimintd's session timeout ends every hour reads as one
//! growing signal rather than unrelated exits.

use std::time::Duration;

use tokio::time::Instant;

use crate::seat_process::ObservedSeatExit;

/// The longest time between two exits of one run. Twice fedimintd's
/// one-hour session timeout, so an hourly loop stays one run however long
/// its respawns take, while exits a day apart do not form one.
pub(crate) const MAX_EXIT_GAP: Duration = Duration::from_secs(2 * 60 * 60);
/// Exits in a run before it counts as a loop.
pub(crate) const LOOP_EXITS: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExitStreak {
    pub(crate) status_code: Option<i32>,
    pub(crate) signal: Option<i32>,
    pub(crate) exits: u32,
    pub(crate) first_at: Instant,
    last_at: Instant,
}

impl ExitStreak {
    /// The run after `exit` at `now`: the previous one extended when the
    /// status matches and the gap is short enough, otherwise a new one.
    pub(crate) fn record(previous: Option<Self>, exit: &ObservedSeatExit, now: Instant) -> Self {
        match previous {
            Some(run)
                if run.status_code == exit.status_code
                    && run.signal == exit.signal
                    && now.saturating_duration_since(run.last_at) <= MAX_EXIT_GAP =>
            {
                Self {
                    exits: run.exits + 1,
                    last_at: now,
                    ..run
                }
            }
            _ => Self {
                status_code: exit.status_code,
                signal: exit.signal,
                exits: 1,
                first_at: now,
                last_at: now,
            },
        }
    }

    pub(crate) fn is_loop(&self) -> bool {
        self.exits >= LOOP_EXITS
    }
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_service_manifold_guardian::SeatId;

    use super::*;

    const HOUR: Duration = Duration::from_secs(60 * 60);

    fn exit(status_code: Option<i32>, signal: Option<i32>) -> ObservedSeatExit {
        ObservedSeatExit {
            seat_id: SeatId::new("cd".repeat(32)).unwrap(),
            status_code,
            signal,
        }
    }

    fn after(exits: &[(Duration, ObservedSeatExit)]) -> Option<ExitStreak> {
        let mut now = Instant::now();
        let mut run = None;
        for (gap, exit) in exits {
            now += *gap;
            run = Some(ExitStreak::record(run, exit, now));
        }
        run
    }

    /// An hourly session-timeout loop is one run whose count grows; it
    /// becomes a loop at the third exit.
    #[test]
    fn an_hourly_exit_with_one_code_is_one_growing_loop() {
        let timeout = exit(Some(255), None);
        let two = after(&[(HOUR, timeout.clone()), (HOUR, timeout.clone())]).unwrap();
        assert!(!two.is_loop());
        let run = after(&vec![(HOUR + Duration::from_secs(70), timeout); 26]).unwrap();
        assert!(run.is_loop());
        assert_eq!(run.exits, 26);
        assert_eq!(run.status_code, Some(255));
    }

    #[test]
    fn another_status_or_a_long_gap_starts_a_new_run() {
        let timeout = exit(Some(255), None);
        let run = after(&[
            (HOUR, timeout.clone()),
            (HOUR, timeout.clone()),
            (HOUR, exit(Some(1), None)),
        ])
        .unwrap();
        assert_eq!((run.exits, run.status_code), (1, Some(1)));
        let run = after(&[
            (HOUR, timeout.clone()),
            (HOUR, timeout.clone()),
            (HOUR, exit(None, Some(9))),
        ])
        .unwrap();
        assert_eq!((run.exits, run.signal), (1, Some(9)));
        let run = after(&[
            (HOUR, timeout.clone()),
            (HOUR, timeout.clone()),
            (MAX_EXIT_GAP + Duration::from_secs(1), timeout.clone()),
        ])
        .unwrap();
        assert_eq!(run.exits, 1);
        let run = after(&[(HOUR, timeout.clone()), (MAX_EXIT_GAP, timeout)]).unwrap();
        assert_eq!(
            run.exits, 2,
            "a gap of exactly the maximum continues the run"
        );
    }
}
