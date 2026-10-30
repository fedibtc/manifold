//! The daemon's latest verdict on whether a new seat would be served.
//!
//! The daemon probes its prerequisites ([`ReadinessProbe`]) and [`run`] hands
//! each report to [`crate::fleet::Fleet::set_seat_readiness`], which gates
//! admission on it.
//! Only fixed codes and this MG's own uptime figures cross this boundary: the
//! report is shown to the operator, and the same values are shared as
//! telemetry.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fedi_decentralized_domain::{AdmittedSetupPaymentFederations, MgVersion, MinUptimePercent};
use fedi_decentralized_nostr::mg::{AdvertisedUptime, Permille};
use fedi_decentralized_service_manifold_guardian::SEAT_READINESS_EVENT;
use tokio::sync::watch;

use crate::fleet::Fleet;

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
    /// This MG's release is older than the setup-payment policy's minimum.
    MgVersionTooOld,
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
            Self::MgVersionTooOld => "mg_version_too_old",
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
    /// This MG's release against the setup-payment policy's minimum.
    pub version: ReadinessOutcome,
    pub uptime: ReadinessOutcome,
    /// The uptime scores this run compared and the minimums it compared them
    /// against; `None` before the run records its slot, or when recording it
    /// failed.
    pub uptime_comparison: Option<UptimeComparison>,
}

/// One figure per uptime window, in per-mille: the 0.1% the gate compares at.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct UptimePermille {
    #[serde(rename = "1d")]
    pub day: u16,
    #[serde(rename = "7d")]
    pub week: u16,
    #[serde(rename = "30d")]
    pub month: u16,
}

impl From<UptimePermille> for AdvertisedUptime {
    fn from(scores: UptimePermille) -> Self {
        let permille =
            |value: u16| Permille::try_from(value).expect("uptime scores are clamped to 0..=1000");
        Self {
            day: permille(scores.day),
            week: permille(scores.week),
            month: permille(scores.month),
        }
    }
}

impl From<MinUptimePercent> for UptimePermille {
    fn from(min: MinUptimePercent) -> Self {
        Self {
            day: u16::from(min.day) * 10,
            week: u16::from(min.week) * 10,
            month: u16::from(min.month) * 10,
        }
    }
}

impl UptimePermille {
    /// The readiness outcome of these scores against `min`, naming the
    /// shortest window that fails.
    pub fn outcome(self, min: Self) -> ReadinessOutcome {
        [
            (self.day, min.day, ReadinessOutcome::DayUptimeLow),
            (self.week, min.week, ReadinessOutcome::WeekUptimeLow),
            (self.month, min.month, ReadinessOutcome::MonthUptimeLow),
        ]
        .into_iter()
        .find(|(score, min, _)| score < min)
        .map_or(ReadinessOutcome::Pass, |(_, _, low)| low)
    }
}

/// The uptime scores one readiness run compared, beside the setup-payment
/// policy minimums it compared them against.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct UptimeComparison {
    pub scores: UptimePermille,
    pub minimums: UptimePermille,
    /// Per window, how long this MG must stay up without a break before the
    /// score reaches its minimum; zero for a window that passes.
    pub wait_secs: UptimeWaitSecs,
}

/// One wait per uptime window, in seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct UptimeWaitSecs {
    #[serde(rename = "1d")]
    #[ts(type = "number")]
    pub day: u64,
    #[serde(rename = "7d")]
    #[ts(type = "number")]
    pub week: u64,
    #[serde(rename = "30d")]
    #[ts(type = "number")]
    pub month: u64,
}

impl ReadinessReport {
    pub fn ready(&self) -> bool {
        self.prerequisites_passed() && self.version.passed() && self.uptime.passed()
    }

    /// Whether the probed prerequisites passed. These decide whether a
    /// slot counts as up, so the uptime gate cannot keep itself closed. The
    /// version check is not one of them: an MG below the policy's minimum
    /// release still serves its seats, so it is up.
    pub fn prerequisites_passed(&self) -> bool {
        self.relay.passed() && self.discovery.passed() && self.bitcoin.passed()
    }
}

/// The version check: `current` against the policy's minimum release, as
/// SemVer. Without a minimum the check does not apply.
pub fn version_outcome(current: &MgVersion, minimum: Option<&MgVersion>) -> ReadinessOutcome {
    match minimum {
        None => ReadinessOutcome::NotApplicable,
        Some(minimum) if current.is_older_than(minimum) => ReadinessOutcome::MgVersionTooOld,
        Some(_) => ReadinessOutcome::Pass,
    }
}

/// This daemon's own release.
pub fn this_mg_version() -> MgVersion {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("the workspace version is SemVer")
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
        self.permille().outcome(min.into())
    }

    /// These scores rounded to the 0.1% the gate compares at, so what the
    /// operator sees is exactly what the gate decided on.
    pub fn comparison(self, min: MinUptimePercent) -> UptimeComparison {
        UptimeComparison {
            scores: self.permille(),
            minimums: min.into(),
            wait_secs: UptimeWaitSecs {
                day: wait_secs(self.day, min.day, WINDOW_DAYS[0]),
                week: wait_secs(self.week, min.week, WINDOW_DAYS[1]),
                month: wait_secs(self.month, min.month, WINDOW_DAYS[2]),
            },
        }
    }

    pub fn permille(self) -> UptimePermille {
        // A score is a weighted average of 0s and 1s, so it stays in 0..=1.
        let permille = |score: f64| (score.clamp(0.0, 1.0) * 1000.0).round() as u16;
        UptimePermille {
            day: permille(self.day),
            week: permille(self.week),
            month: permille(self.month),
        }
    }
}

/// Time constants of the day, week and month scores, in days.
const WINDOW_DAYS: [f64; 3] = [1.0, 7.0, 30.0];

/// How long a score must see only up slots before the gate passes it at
/// `min_percent`, rounded up to whole slots. Each up slot multiplies the
/// shortfall `1 - score` by `keep`, and the gate passes a score that rounds to
/// the minimum at 0.1%, which is half a step under it.
fn wait_secs(score: f64, min_percent: u8, days: f64) -> u64 {
    let target = (f64::from(min_percent) * 10.0 - 0.5) / 1000.0;
    if score >= target {
        return 0;
    }
    let keep = (-(UPTIME_SLOT_SECS as f64) / (days * DAY_SECS)).exp();
    let slots = ((1.0 - target) / (1.0 - score)).ln() / keep.ln();
    slots.ceil() as u64 * UPTIME_SLOT_SECS
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
                day: update(scores.day, WINDOW_DAYS[0]),
                week: update(scores.week, WINDOW_DAYS[1]),
                month: update(scores.month, WINDOW_DAYS[2]),
            },
            slot,
        })
    }
}

/// This MG's own prerequisites for a new seat: relay, discovery and Bitcoin.
/// The daemon probes them against its endpoint and backend; the run sets the
/// version and uptime outcomes.
#[async_trait::async_trait]
pub trait ReadinessProbe: Send {
    async fn probe(&mut self) -> ReadinessReport;
}

const NOT_READY_INTERVAL: Duration = Duration::from_secs(60);

/// Run until aborted: probe, record this uptime slot, apply the verdict to
/// `fleet`, journal it, and wait for the next slot while ready or a minute
/// while failing, or until the setup-payment policy changes. `clock` is the
/// wall clock slots are counted in.
pub async fn run(
    mut probe: impl ReadinessProbe,
    fleet: Arc<Fleet>,
    mut setup_payment_policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
    clock: impl Fn() -> SystemTime + Send,
) {
    let this_release = this_mg_version();
    loop {
        let mut report = probe.probe().await;
        // Recording this slot also accounts for every slot since the last
        // record, such as a restart's downtime.
        let recorded_at = clock();
        let recorded = match fleet
            .record_uptime_slot(recorded_at, report.prerequisites_passed())
            .await
        {
            Ok(scores) => Some(scores),
            Err(error) => {
                tracing::warn!(%error, "failed to record an uptime slot");
                None
            }
        };
        let scores = recorded.unwrap_or(UptimeScores {
            day: 0.0,
            week: 0.0,
            month: 0.0,
        });
        let (min, version) = {
            let policy = setup_payment_policy.borrow_and_update();
            let policy = policy.as_ref();
            (
                policy.map_or_else(MinUptimePercent::default, |policy| {
                    policy.min_uptime_percent()
                }),
                version_outcome(
                    &this_release,
                    policy.and_then(AdmittedSetupPaymentFederations::min_mg_version),
                ),
            )
        };
        report.version = version;
        report.uptime = scores.outcome(min);
        // The operator sees no figures rather than zeros it cannot trust.
        report.uptime_comparison = recorded.map(|scores| scores.comparison(min));
        let ready = report.ready();
        journal_readiness(&report, scores, min);
        if let Err(error) = fleet.set_seat_readiness(report).await {
            tracing::warn!(%error, "failed to apply the seat readiness verdict");
        }
        let wait = if ready {
            until_slot_after(recorded_at, clock())
        } else {
            NOT_READY_INTERVAL
        };
        wait_for_next_run(wait, &mut setup_payment_policy).await;
    }
}

/// Wait `wait`, or less if the setup-payment policy changes first, since a
/// new policy can change the verdict (its minimum release or uptime). A
/// policy that can no longer change only leaves the timer.
async fn wait_for_next_run(
    wait: Duration,
    policy: &mut watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) {
    tokio::select! {
        () = tokio::time::sleep(wait) => {}
        Ok(()) = policy.changed() => {}
    }
}

/// The time from now to just past the start of the slot after the one
/// `recorded_at` falls in, so a ready MG runs once in every slot and misses
/// none, even when recording crossed a boundary.
fn until_slot_after(recorded_at: SystemTime, now: SystemTime) -> Duration {
    let recorded = recorded_at
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    let next =
        UNIX_EPOCH + Duration::from_secs((recorded / UPTIME_SLOT_SECS + 1) * UPTIME_SLOT_SECS + 1);
    // A clock set back while recording must not stretch the wait past one
    // slot, which would leave the last verdict standing unchecked.
    next.duration_since(now)
        .unwrap_or(Duration::ZERO)
        .min(Duration::from_secs(UPTIME_SLOT_SECS + 1))
}

/// Journal one completed run for telemetry, with each score to the 0.1% the
/// gate compares at.
fn journal_readiness(report: &ReadinessReport, scores: UptimeScores, min: MinUptimePercent) {
    let pct = |score: f64| (score * 1000.0).round() / 10.0;
    tracing::info!(
        safe_to_share = true,
        ready = report.ready(),
        relay = report.relay.as_str(),
        discovery = report.discovery.as_str(),
        bitcoin = report.bitcoin.as_str(),
        version = report.version.as_str(),
        uptime = report.uptime.as_str(),
        uptime_1d_pct = pct(scores.day),
        uptime_7d_pct = pct(scores.week),
        uptime_30d_pct = pct(scores.month),
        min_uptime_1d_pct = min.day,
        min_uptime_7d_pct = min.week,
        min_uptime_30d_pct = min.month,
        "{SEAT_READINESS_EVENT}"
    );
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
    fn the_operator_sees_the_figures_the_gate_compared() {
        let scores = UptimeScores {
            day: 0.8869,
            week: 0.8995,
            month: 0.93549,
        };
        assert_eq!(
            scores.permille(),
            UptimePermille {
                day: 887,
                week: 900,
                month: 935,
            }
        );
        assert_eq!(
            UptimePermille::from(only(70, 90, 93)),
            UptimePermille {
                day: 700,
                week: 900,
                month: 930,
            }
        );
        // The per-mille comparison decides exactly like the old float one.
        for step in 0..=10_000_u32 {
            let score = f64::from(step) / 10_000.0;
            let scores = UptimeScores {
                day: 1.0,
                week: score,
                month: 1.0,
            };
            let float_low = (score * 1000.0).round() < 900.0;
            assert_eq!(
                scores.outcome(only(0, 90, 0)) == ReadinessOutcome::WeekUptimeLow,
                float_low,
                "{score}"
            );
        }
    }

    /// The wait is checked against the recorder itself: after the reported
    /// number of up slots the gate passes the window, and one slot earlier it
    /// does not. The cases include each window and a score one 0.1% step short.
    #[test]
    fn the_wait_is_exactly_the_up_slots_the_gate_needs() {
        let min = only(70, 90, 93);
        for scores in [
            UptimeScores {
                day: 0.784,
                week: 0.832,
                month: 0.914,
            },
            UptimeScores {
                day: 0.274,
                week: 0.794,
                month: 0.911,
            },
            UptimeScores {
                day: 0.69,
                week: 0.8994,
                month: 0.9294,
            },
            UptimeScores {
                day: 0.0,
                week: 0.0,
                month: 0.0,
            },
        ] {
            let wait = scores.comparison(min).wait_secs;
            for (secs, low) in [
                (wait.day, ReadinessOutcome::DayUptimeLow),
                (wait.week, ReadinessOutcome::WeekUptimeLow),
                (wait.month, ReadinessOutcome::MonthUptimeLow),
            ] {
                assert_eq!(secs % UPTIME_SLOT_SECS, 0);
                let after = |slots: u64| {
                    let mut uptime = Uptime { scores, slot: 0 };
                    for slot in 1..=slots {
                        uptime = Uptime::record(Some(uptime), slot, true).unwrap();
                    }
                    let only_this = |s: UptimeScores| match low {
                        ReadinessOutcome::DayUptimeLow => s.outcome(only(min.day, 0, 0)),
                        ReadinessOutcome::WeekUptimeLow => s.outcome(only(0, min.week, 0)),
                        _ => s.outcome(only(0, 0, min.month)),
                    };
                    only_this(uptime.scores)
                };
                let slots = secs / UPTIME_SLOT_SECS;
                assert_eq!(after(slots), ReadinessOutcome::Pass, "{scores:?} {low:?}");
                if slots > 0 {
                    assert_eq!(after(slots - 1), low, "{scores:?} {low:?}");
                }
            }
        }
        // A passing window needs no wait.
        let passing = UptimeScores {
            day: 0.975,
            week: 0.958,
            month: 0.941,
        };
        assert_eq!(
            passing.comparison(min).wait_secs,
            UptimeWaitSecs {
                day: 0,
                week: 0,
                month: 0
            }
        );
    }

    #[test]
    fn the_report_carries_the_uptime_comparison() {
        let report = ReadinessReport {
            checked_at_ms: 1,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::Pass,
            bitcoin: ReadinessOutcome::Pass,
            version: ReadinessOutcome::NotApplicable,
            uptime: ReadinessOutcome::WeekUptimeLow,
            uptime_comparison: Some(UptimeComparison {
                scores: UptimePermille {
                    day: 887,
                    week: 899,
                    month: 935,
                },
                minimums: UptimePermille::from(only(70, 90, 93)),
                wait_secs: UptimeWaitSecs {
                    day: 0,
                    week: 3000,
                    month: 0,
                },
            }),
        };
        assert_eq!(
            serde_json::to_value(report).ok(),
            Some(serde_json::json!({
                "checked_at_ms": 1,
                "relay": "pass",
                "discovery": "pass",
                "bitcoin": "pass",
                "version": "not_applicable",
                "uptime": "uptime_7d_low",
                "uptime_comparison": {
                    "scores": { "1d": 887, "7d": 899, "30d": 935 },
                    "minimums": { "1d": 700, "7d": 900, "30d": 930 },
                    "wait_secs": { "1d": 0, "7d": 3000, "30d": 0 },
                },
            }))
        );
    }

    #[test]
    fn only_the_prerequisites_decide_whether_a_slot_is_up() {
        let report = ReadinessReport {
            checked_at_ms: 0,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::NotApplicable,
            bitcoin: ReadinessOutcome::Pass,
            version: ReadinessOutcome::Pass,
            uptime: ReadinessOutcome::WeekUptimeLow,
            uptime_comparison: None,
        };
        assert!(report.prerequisites_passed());
        assert!(!report.ready());
        // A release below the policy minimum closes the gate, but the MG
        // still serves its seats, so its slots stay up.
        let too_old = ReadinessReport {
            version: ReadinessOutcome::MgVersionTooOld,
            uptime: ReadinessOutcome::Pass,
            ..report
        };
        assert!(too_old.prerequisites_passed());
        assert!(!too_old.ready());
        assert!(
            ReadinessReport {
                version: ReadinessOutcome::Pass,
                ..too_old
            }
            .ready()
        );
    }

    /// Releases compare by SemVer precedence: 0.1.10 is newer than 0.1.9
    /// although it sorts before it as text, a pre-release is older than its
    /// release, and build metadata does not count.
    #[test]
    fn the_version_check_compares_releases_against_the_minimum() {
        let version = |v: &str| v.parse::<MgVersion>().unwrap();
        let minimum = version("0.1.9");
        for (current, expected) in [
            ("0.1.8", ReadinessOutcome::MgVersionTooOld),
            ("0.1.9-rc.1", ReadinessOutcome::MgVersionTooOld),
            ("0.1.9", ReadinessOutcome::Pass),
            ("0.1.9+build.1", ReadinessOutcome::Pass),
            ("0.1.10", ReadinessOutcome::Pass),
            ("1.0.0", ReadinessOutcome::Pass),
        ] {
            assert_eq!(
                version_outcome(&version(current), Some(&minimum)),
                expected,
                "{current}"
            );
        }
        assert_eq!(
            version_outcome(&version("0.1.9"), Some(&version("0.1.9+build.1"))),
            ReadinessOutcome::Pass
        );
        assert_eq!(
            version_outcome(&version("0.0.1"), None),
            ReadinessOutcome::NotApplicable
        );
        assert_eq!(this_mg_version().to_string(), env!("CARGO_PKG_VERSION"));
    }

    /// The journaled run decodes as the telemetry collector reads it.
    #[test]
    fn a_journaled_run_decodes_as_the_shared_readiness_event() {
        use fedi_decentralized_service_manifold_guardian::SeatReadinessEvent;
        use tracing_subscriber::layer::SubscriberExt as _;

        let directory = tempfile::tempdir().unwrap();
        let subscriber =
            tracing_subscriber::registry().with(safe_tracing::layer(directory.path()).unwrap());
        let report = ReadinessReport {
            checked_at_ms: 1,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::NotApplicable,
            bitcoin: ReadinessOutcome::Pass,
            version: ReadinessOutcome::MgVersionTooOld,
            uptime: ReadinessOutcome::WeekUptimeLow,
            uptime_comparison: None,
        };
        let scores = UptimeScores {
            day: 0.9944,
            week: 0.881,
            month: 0.95,
        };
        tracing::subscriber::with_default(subscriber, || {
            journal_readiness(&report, scores, MinUptimePercent::default());
        });

        let mut records = Vec::new();
        for entry in std::fs::read_dir(directory.path()).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                for line in std::fs::read_to_string(path).unwrap().lines() {
                    records.push(serde_json::from_str::<serde_json::Value>(line).unwrap());
                }
            }
        }
        let [record] = records.as_slice() else {
            panic!("expected one journaled run, got {records:?}");
        };
        assert_eq!(record["fields"]["message"], SEAT_READINESS_EVENT);
        // The collector keeps only records whose timestamp parses as RFC 3339.
        humantime::parse_rfc3339(record["timestamp"].as_str().unwrap()).unwrap();
        let min = MinUptimePercent::default();
        assert_eq!(
            serde_json::from_value::<SeatReadinessEvent>(record["fields"].clone()).unwrap(),
            SeatReadinessEvent {
                ready: false,
                relay: "pass".into(),
                discovery: "not_applicable".into(),
                bitcoin: "pass".into(),
                version: Some("mg_version_too_old".into()),
                uptime: Some("uptime_7d_low".into()),
                uptime_1d_pct: Some(99.4),
                uptime_7d_pct: Some(88.1),
                uptime_30d_pct: Some(95.0),
                min_uptime_1d_pct: Some(f64::from(min.day)),
                min_uptime_7d_pct: Some(f64::from(min.week)),
                min_uptime_30d_pct: Some(f64::from(min.month)),
            }
        );
    }

    /// A new policy ends the wait at once, so a changed minimum release takes
    /// effect without waiting out the slot; a policy that can no longer
    /// change leaves the timer running instead of spinning.
    #[tokio::test]
    async fn a_policy_change_ends_the_wait_for_the_next_run() {
        let (sender, mut policy) = watch::channel(None);
        let waiting = tokio::spawn(async move {
            wait_for_next_run(Duration::from_secs(3600), &mut policy).await;
            policy
        });
        tokio::task::yield_now().await;
        sender.send_replace(None);
        let mut policy = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("a policy change ends the wait")
            .unwrap();

        drop(sender);
        let started = std::time::Instant::now();
        wait_for_next_run(Duration::from_millis(200), &mut policy).await;
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    /// The wait ends just past the start of the slot after the recorded one:
    /// at once when recording crossed into it, and never more than one slot
    /// when the clock was set back meanwhile.
    #[test]
    fn the_wait_ends_in_the_slot_after_the_recorded_one() {
        let slot = Duration::from_secs(UPTIME_SLOT_SECS);
        let second = Duration::from_secs(1);
        let start = UNIX_EPOCH + slot * 3_000_000;
        let recorded = start + Duration::from_secs(100);
        assert_eq!(
            until_slot_after(recorded, recorded),
            slot - Duration::from_secs(100) + second
        );
        assert_eq!(
            until_slot_after(recorded, start + slot + second * 5),
            Duration::ZERO
        );
        assert_eq!(
            until_slot_after(recorded, start + slot - second * 2),
            second * 3
        );
        let set_back_an_hour = recorded - Duration::from_secs(3600);
        assert_eq!(until_slot_after(recorded, set_back_an_hour), slot + second);
    }
}
