//! The daemon's latest verdict on whether a new seat would be served.
//!
//! The daemon probes its prerequisites and hands each report to
//! [`crate::fleet::Fleet::set_seat_readiness`], which gates admission on it.
//! Only fixed codes cross this boundary: the report is shared as telemetry and
//! shown to the operator.

use fedi_decentralized_domain::MinUptimePercent;

/// One uptime slot: the daemon records one per readiness run, and runs at
/// least once per slot while it is up.
pub const UPTIME_SLOT_SECS: u64 = 10 * 60;

const DAY_SECS: f64 = 24.0 * 60.0 * 60.0;

/// One readiness check's outcome; never a remote value or error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessOutcome {
    Pass,
    /// The check does not apply to this deployment.
    NotApplicable,
    RelayDisconnected,
    DiscoveryRecordMissing,
    BitcoinUnavailable,
    BitcoinWrongNetwork,
    BitcoinSyncing,
    BitcoinNoFeeRate,
    /// The one-day uptime score is under the setup-payment policy minimum.
    #[serde(rename = "uptime_1d_low")]
    DayUptimeLow,
    /// The seven-day uptime score is under the setup-payment policy minimum.
    #[serde(rename = "uptime_7d_low")]
    WeekUptimeLow,
    /// The thirty-day uptime score is under the setup-payment policy minimum.
    #[serde(rename = "uptime_30d_low")]
    MonthUptimeLow,
}

impl ReadinessOutcome {
    pub fn passed(self) -> bool {
        matches!(self, Self::Pass | Self::NotApplicable)
    }

    /// The serialized code, for tracing fields.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::NotApplicable => "not_applicable",
            Self::RelayDisconnected => "relay_disconnected",
            Self::DiscoveryRecordMissing => "discovery_record_missing",
            Self::BitcoinUnavailable => "bitcoin_unavailable",
            Self::BitcoinWrongNetwork => "bitcoin_wrong_network",
            Self::BitcoinSyncing => "bitcoin_syncing",
            Self::BitcoinNoFeeRate => "bitcoin_no_fee_rate",
            Self::DayUptimeLow => "uptime_1d_low",
            Self::WeekUptimeLow => "uptime_7d_low",
            Self::MonthUptimeLow => "uptime_30d_low",
        }
    }
}

/// One completed readiness run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct ReadinessReport {
    /// Unix time the run completed.
    #[ts(type = "number")]
    pub checked_at_ms: u64,
    pub relay: ReadinessOutcome,
    pub discovery: ReadinessOutcome,
    pub bitcoin: ReadinessOutcome,
    pub uptime: ReadinessOutcome,
}

impl ReadinessReport {
    pub fn ready(&self) -> bool {
        self.prerequisites_passed() && self.uptime.passed()
    }

    /// Whether the probed prerequisites passed. These decide whether a
    /// slot counts as up, so the uptime gate cannot keep itself closed.
    pub fn prerequisites_passed(&self) -> bool {
        self.relay.passed() && self.discovery.passed() && self.bitcoin.passed()
    }
}

/// How much of the recent past this MG was up: exponential moving averages
/// over [`UPTIME_SLOT_SECS`] slots with 1-, 7- and 30-day time constants,
/// each between 0 and 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UptimeScores {
    pub day: f64,
    pub week: f64,
    pub month: f64,
}

impl UptimeScores {
    /// The scores history starts at, with its first up slot: a new MG has
    /// less margin than one with a good record.
    pub const START: Self = Self {
        day: 0.95,
        week: 0.95,
        month: 0.95,
    };

    /// The readiness outcome of these scores against the policy minimums,
    /// naming the shortest window that fails. Scores compare at the 0.1%
    /// telemetry reports: up slots approach 1 without reaching it, so an
    /// exact comparison would never pass a minimum of 100.
    pub fn outcome(self, min: MinUptimePercent) -> ReadinessOutcome {
        [
            (self.day, min.day, ReadinessOutcome::DayUptimeLow),
            (self.week, min.week, ReadinessOutcome::WeekUptimeLow),
            (self.month, min.month, ReadinessOutcome::MonthUptimeLow),
        ]
        .into_iter()
        .find(|(score, min, _)| (score * 1000.0).round() < f64::from(*min) * 10.0)
        .map_or(ReadinessOutcome::Pass, |(_, _, low)| low)
    }
}

/// The durable uptime history: the scores after the last recorded slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uptime {
    pub scores: UptimeScores,
    /// The last recorded slot, in Unix time divided by [`UPTIME_SLOT_SECS`].
    pub slot: u64,
}

impl Uptime {
    /// Account for `slot`, which counts as up when `up`. Every slot since the
    /// last recorded one counts as down: no daemon ran to record it. A slot at
    /// or before the last recorded one changes nothing. History starts at the
    /// first up slot, so setup before the MG first passes its prerequisites
    /// costs it nothing.
    pub fn record(previous: Option<Self>, slot: u64, up: bool) -> Option<Self> {
        let previous = match previous {
            Some(previous) => previous,
            None if up => Self {
                scores: UptimeScores::START,
                slot: slot.saturating_sub(1),
            },
            None => return None,
        };
        if slot <= previous.slot {
            return Some(previous);
        }
        let gap = slot - previous.slot;
        let update = |score: f64, days: f64| {
            let keep =
                |slots: u64| (-(slots as f64) * UPTIME_SLOT_SECS as f64 / (days * DAY_SECS)).exp();
            score * keep(gap) + if up { 1.0 - keep(1) } else { 0.0 }
        };
        let scores = previous.scores;
        Some(Self {
            scores: UptimeScores {
                day: update(scores.day, 1.0),
                week: update(scores.week, 7.0),
                month: update(scores.month, 30.0),
            },
            slot,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 6;
    const DAY: u64 = 24 * HOUR;

    /// Only the named window checked, at `percent`.
    fn only(day: u8, week: u8, month: u8) -> MinUptimePercent {
        MinUptimePercent { day, week, month }
    }

    fn up_for(mut uptime: Uptime, slots: u64) -> Uptime {
        for _ in 0..slots {
            uptime = Uptime::record(Some(uptime), uptime.slot + 1, true).unwrap();
        }
        uptime
    }

    /// A history that has been up for good.
    fn fully_up(slot: u64) -> Uptime {
        Uptime {
            scores: UptimeScores {
                day: 1.0,
                week: 1.0,
                month: 1.0,
            },
            slot,
        }
    }

    /// Slots, from `outage` after a good record and a matching gap, until
    /// `min` passes again.
    fn closed_after(outage: u64, min: MinUptimePercent) -> u64 {
        let mut uptime = Uptime::record(Some(fully_up(1000)), 1000 + outage + 1, true).unwrap();
        let mut slots = 0;
        while uptime.scores.outcome(min) != ReadinessOutcome::Pass {
            uptime = up_for(uptime, 1);
            slots += 1;
        }
        slots
    }

    /// History starts at 95% with the slot before the first up one, so a new
    /// MG has less margin than one with a good record.
    #[test]
    fn history_starts_at_95_percent_at_the_first_up_slot() {
        assert_eq!(Uptime::record(None, 1000, false), None);
        let first = Uptime::record(None, 1000, true).unwrap();
        assert_eq!(first.slot, 1000);
        let keep = (-1.0f64 / (7.0 * DAY as f64)).exp();
        assert!(
            (first.scores.week - (0.95 * keep + 1.0 - keep)).abs() < 1e-12,
            "{first:?}"
        );
        assert!(first.scores.day > first.scores.week);
        assert!(first.scores.week > first.scores.month);
    }

    /// One day missing from a 7-day average leaves e^(-1/7) of it plus the
    /// up slot after it, under the bar. Up slots shrink the 13.2% shortfall
    /// to the 10.05% that reports as 90.0% in 7 ln(13.2 / 10.05) = 1.92 days.
    #[test]
    fn a_day_offline_closes_the_week_window_for_about_two_days() {
        let after_gap = Uptime::record(Some(fully_up(1000)), 1000 + DAY, true).unwrap();
        let keep = (-1.0f64 / (7.0 * DAY as f64)).exp();
        assert!(
            (after_gap.scores.week - ((-1.0f64 / 7.0).exp() + 1.0 - keep)).abs() < 1e-12,
            "{after_gap:?}"
        );
        assert_eq!(
            after_gap.scores.outcome(only(0, 90, 0)),
            ReadinessOutcome::WeekUptimeLow
        );
        let days = closed_after(DAY - 1, only(0, 90, 0)) as f64 / DAY as f64;
        assert!((days - 1.92).abs() < 0.01, "{days} days");
    }

    /// Under the 70% one-day minimum, up to about 24 ln(1 / 0.7) = 8.6 hours
    /// down passes, and 12 hours down closes it for 24 ln(0.393 / 0.3) = 6.5
    /// hours.
    #[test]
    fn the_day_window_holds_an_mg_closed_for_hours_after_a_long_outage() {
        assert_eq!(closed_after(8 * HOUR, only(70, 0, 0)), 0);
        assert_ne!(closed_after(9 * HOUR, only(70, 0, 0)), 0);
        let hours = closed_after(12 * HOUR, only(70, 0, 0)) as f64 / HOUR as f64;
        assert!((hours - 6.5).abs() < 0.25, "{hours} hours");
    }

    /// An MG down 2 hours every day settles near 91.7% up: over the 90%
    /// seven-day minimum, under the 93% thirty-day one.
    #[test]
    fn the_month_window_closes_an_mg_that_is_often_down_a_little() {
        let mut uptime = fully_up(0);
        for day in 0..120 {
            uptime = Uptime::record(Some(uptime), day * DAY + 2 * HOUR, true).unwrap();
            uptime = up_for(uptime, DAY - 2 * HOUR - 1);
        }
        let scores = uptime.scores;
        assert!((scores.month - 22.0 / 24.0).abs() < 0.01, "{scores:?}");
        assert_eq!(
            scores.outcome(MinUptimePercent::default()),
            ReadinessOutcome::MonthUptimeLow
        );
        assert_eq!(scores.outcome(only(70, 90, 0)), ReadinessOutcome::Pass);
    }

    #[test]
    fn the_outcome_names_the_shortest_failing_window() {
        let scores = UptimeScores {
            day: 0.5,
            week: 0.5,
            month: 0.5,
        };
        assert_eq!(
            scores.outcome(MinUptimePercent::default()),
            ReadinessOutcome::DayUptimeLow
        );
        assert_eq!(
            scores.outcome(only(0, 90, 93)),
            ReadinessOutcome::WeekUptimeLow
        );
        assert_eq!(scores.outcome(only(0, 0, 0)), ReadinessOutcome::Pass);
    }

    #[test]
    fn down_slots_lower_it_and_up_slots_do_not() {
        let start = fully_up(1000);
        assert_eq!(up_for(start, 500).scores, start.scores);
        let down = Uptime::record(Some(start), 1001, false).unwrap();
        assert!(down.scores.day < 1.0 && down.scores.week < 1.0 && down.scores.month < 1.0);
        // A slot missed while the daemon was down weighs the same as one
        // recorded down.
        let missed = Uptime::record(Some(start), 1002, true).unwrap();
        let recorded = Uptime::record(Some(down), 1002, true).unwrap();
        assert!((missed.scores.month - recorded.scores.month).abs() < 1e-12);
        assert!((missed.scores.day - recorded.scores.day).abs() < 1e-12);
    }

    #[test]
    fn a_minimum_of_100_passes_after_recovery() {
        let down = Uptime::record(Some(fully_up(1000)), 1001 + HOUR, true).unwrap();
        assert_eq!(
            down.scores.outcome(only(0, 100, 0)),
            ReadinessOutcome::WeekUptimeLow
        );
        // An hour down leaves 99.41%; 99.95% rounds to 100.0% after
        // about 7 ln(0.0059 / 0.0005) = 17.3 days.
        let recovered = up_for(down, 18 * DAY);
        assert!(recovered.scores.week < 1.0, "{recovered:?}");
        assert_eq!(
            recovered.scores.outcome(only(0, 100, 0)),
            ReadinessOutcome::Pass
        );
        let at = |week| UptimeScores {
            day: 1.0,
            week,
            month: 1.0,
        };
        assert_eq!(
            at(0.8995).outcome(only(0, 90, 0)),
            ReadinessOutcome::Pass,
            "89.95% reports as 90.0%"
        );
        assert_eq!(
            at(0.8994).outcome(only(0, 90, 0)),
            ReadinessOutcome::WeekUptimeLow
        );
    }

    #[test]
    fn a_repeated_or_earlier_slot_changes_nothing() {
        let down = Uptime::record(Some(fully_up(1000)), 1001, false).unwrap();
        assert_eq!(Uptime::record(Some(down), 1001, true), Some(down));
        assert_eq!(Uptime::record(Some(down), 900, true), Some(down));
    }

    #[test]
    fn only_the_prerequisites_decide_whether_a_slot_is_up() {
        let report = ReadinessReport {
            checked_at_ms: 0,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::NotApplicable,
            bitcoin: ReadinessOutcome::Pass,
            uptime: ReadinessOutcome::WeekUptimeLow,
        };
        assert!(report.prerequisites_passed());
        assert!(!report.ready());
    }
}
