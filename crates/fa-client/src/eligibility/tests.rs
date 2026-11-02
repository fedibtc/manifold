use fedi_decentralized_domain::GuardianSessionReport;
use fedimint_core::peer_availability::{
    PEER_AVAILABILITY_WINDOWS_SECS, PeerAvailability, WindowAvailability,
};

use super::*;

fn policy(seconds: u64, week: u8, month: u8) -> AdmittedSetupPaymentFederations {
    AdmittedSetupPaymentFederations::parse(
        serde_json::to_string(&serde_json::json!({
            "version": 1, "fman_version": "0.1.0", "federations": [],
            "telemetry_registration_url": "https://example.com/telemetry",
            "max_guardian_offline_secs": seconds, "min_uptime_percent": {"7d": week, "30d": month}
        }))
        .unwrap()
        .as_bytes(),
    )
    .unwrap()
}

fn reports() -> GuardianReports {
    GuardianReports {
        federation_id: FederationId("test".into()),
        peers: (0..4).map(PeerId::from).collect(),
        sessions: BTreeMap::new(),
        availability: BTreeMap::new(),
    }
}

fn session(reports: &mut GuardianReports, reporter: u16, gap: Option<u64>) {
    reports.sessions.insert(
        PeerId::from(reporter),
        GuardianSessionReport {
            session_count: 1000,
            disconnected: BTreeSet::new(),
            last_contribution: BTreeMap::from([(PeerId::from(0), gap.map(|gap| 999 - gap))]),
        },
    );
}

fn availability(reports: &mut GuardianReports, reporter: u16, seen: u64, week: u16, month: u16) {
    reports.availability.insert(
        PeerId::from(reporter),
        PeerAvailability {
            last_seen_secs: BTreeMap::from([(PeerId::from(0), seen)]),
            windows: PEER_AVAILABILITY_WINDOWS_SECS
                .into_iter()
                .zip([week, month])
                .map(|(window_secs, score)| WindowAvailability {
                    window_secs,
                    coverage_bps: 10_000,
                    peers: BTreeMap::from([(PeerId::from(0), score)]),
                })
                .collect(),
        },
    );
}

#[test]
fn eligibility_session_fallback_requires_other_reporters_and_uses_policy_duration() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    session(&mut reports, 0, Some(900)); // Self-report cannot help.
    session(&mut reports, 1, Some(21));
    session(&mut reports, 2, None); // Unknown is not zero.
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
    session(&mut reports, 2, Some(20)); // Exactly one hour is not over the limit.
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
    session(&mut reports, 2, Some(21));
    assert_eq!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::MissedSessions { sessions: 21 }]
    );
    assert!(
        replacement_reasons(
            &reports,
            &Tenures::default(),
            &super::tests::policy(7200, 90, 93),
            10_000
        )[&PeerId::from(0)]
            .is_empty()
    );
}

#[test]
fn eligibility_recent_timestamps_override_old_session_estimates_and_scores_use_policy() {
    let mut reports = reports();
    for peer in [1, 2] {
        session(&mut reports, peer, Some(900));
        availability(&mut reports, peer, 9999, 8500, 9100);
    }
    assert_eq!(
        replacement_reasons(&reports, &Tenures::default(), &policy(3600, 90, 93), 10_000)
            [&PeerId::from(0)],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 8500 },
            ReplacementReason::LowMonthScore { basis_points: 9100 }
        ]
    );
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy(3600, 85, 91), 10_000)
            [&PeerId::from(0)]
            .is_empty()
    );
}

#[test]
fn eligibility_ignores_outsiders_and_needs_confirmed_absence() {
    let mut reports = reports();
    availability(&mut reports, 0, 1, 0, 0);
    availability(&mut reports, 1, 1, 10_000, 10_000);
    availability(&mut reports, 99, 1, 0, 0);
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy(3600, 90, 93), 10_000)
            [&PeerId::from(0)]
            .is_empty()
    );
    availability(&mut reports, 2, 6400, 10_000, 10_000);
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy(3600, 90, 93), 10_000)
            [&PeerId::from(0)]
            .is_empty()
    );
    availability(&mut reports, 2, 6399, 10_000, 10_000);
    assert_eq!(
        replacement_reasons(&reports, &Tenures::default(), &policy(3600, 90, 93), 10_000)
            [&PeerId::from(0)],
        vec![ReplacementReason::Offline {
            last_seen_secs: 6399
        }]
    );
}

#[test]
fn eligibility_disconnection_fallback_requires_f_plus_one_other_members() {
    let mut reports = reports();
    reports.peers = (0..10).map(PeerId::from).collect();
    for reporter in [0, 1, 2, 3, 99] {
        session(&mut reports, reporter, None);
        reports
            .sessions
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .disconnected
            .insert(PeerId::from(0));
    }
    let policy = policy(86_400, 90, 93);
    // Self and outsider reports cannot make three actual peers count as four.
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
    session(&mut reports, 4, None); // Unknown contribution, but not disconnected.
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
    reports
        .sessions
        .get_mut(&PeerId::from(4))
        .unwrap()
        .disconnected
        .insert(PeerId::from(0));
    assert_eq!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::Disconnected]
    );
}

#[test]
fn eligibility_recent_history_prevents_disconnection_fallback() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    for reporter in [1, 2] {
        session(&mut reports, reporter, None);
        reports
            .sessions
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .disconnected
            .insert(PeerId::from(0));
    }
    assert_eq!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::Disconnected]
    );
    for report in reports.sessions.values_mut() {
        report
            .last_contribution
            .insert(PeerId::from(0), Some(report.session_count));
    }
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
    for report in reports.sessions.values_mut() {
        report.last_contribution.insert(PeerId::from(0), None);
    }
    for reporter in [1, 2] {
        availability(&mut reports, reporter, 9999, 10_000, 10_000);
    }
    assert!(
        replacement_reasons(&reports, &Tenures::default(), &policy, 10_000)[&PeerId::from(0)]
            .is_empty()
    );
}

const DAY: u64 = 86_400;
/// When the moved peer changed host, in the tests below.
const MOVED_AT: u64 = 1_000_000_000;

/// What fedimintd reports per window, as (score, coverage), at `now` for a
/// peer of a federation formed at `formed_at` that was up `before` of the
/// time until `moved_at` and `after` of the time since, all of it observed.
fn reported(formed_at: u64, moved_at: u64, before: f64, after: f64, now: u64) -> Vec<(u16, u16)> {
    PEER_AVAILABILITY_WINDOWS_SECS
        .into_iter()
        .map(|window_secs| {
            let weight = |since: u64| (-((now - since) as f64) / window_secs as f64).exp();
            let coverage = 1.0 - weight(formed_at);
            let up =
                before * (weight(moved_at) - weight(formed_at)) + after * (1.0 - weight(moved_at));
            (
                (up / coverage * 10_000.0).round() as u16,
                (coverage * 10_000.0).round() as u16,
            )
        })
        .collect()
}

/// A federation formed a year before the move.
fn long_running(before: f64, after: f64, now: u64) -> Vec<(u16, u16)> {
    reported(MOVED_AT - 365 * DAY, MOVED_AT, before, after, now)
}

/// Every other peer reports peer 0 with these windows, and saw it at `seen`.
fn peer_zero_reports(windows: &[(u16, u16)], seen: u64) -> GuardianReports {
    let mut reports = reports();
    for reporter in 1..4 {
        reports.availability.insert(
            PeerId::from(reporter),
            PeerAvailability {
                last_seen_secs: BTreeMap::from([(PeerId::from(0), seen)]),
                windows: PEER_AVAILABILITY_WINDOWS_SECS
                    .into_iter()
                    .zip(windows)
                    .map(|(window_secs, (score, coverage))| WindowAvailability {
                        window_secs,
                        coverage_bps: *coverage,
                        peers: BTreeMap::from([(PeerId::from(0), *score)]),
                    })
                    .collect(),
            },
        );
    }
    reports
}

/// Peers 1 to 3 bound at formation, peer 0 at `bound_at`.
fn tenures(formed_at: u64, bound_at: u64) -> Tenures {
    Tenures {
        formed_at,
        bound_at: (0..4)
            .map(|peer| {
                (
                    PeerId::from(peer),
                    if peer == 0 { bound_at } else { formed_at },
                )
            })
            .collect(),
    }
}

fn moved_tenures() -> Tenures {
    tenures(MOVED_AT - 365 * DAY, MOVED_AT)
}

fn scores(reasons: &[ReplacementReason]) -> (Option<u16>, Option<u16>) {
    let find = |week: bool| {
        reasons.iter().find_map(|reason| match (reason, week) {
            (ReplacementReason::LowWeekScore { basis_points }, true)
            | (ReplacementReason::LowMonthScore { basis_points }, false) => Some(*basis_points),
            _ => None,
        })
    };
    (find(true), find(false))
}

#[test]
fn a_new_host_is_not_blamed_for_its_predecessors_downtime() {
    // Up 25% of the time on the old host, then all the time on the new one.
    let policy = policy(3600, 90, 93);
    for tenure in [DAY, 3 * DAY, 20 * DAY] {
        let now = MOVED_AT + tenure;
        let reports = peer_zero_reports(&long_running(0.25, 1.0, now), now);
        // The raw scores still carry the old host's downtime.
        assert!(
            !replacement_reasons(&reports, &Tenures::default(), &policy, now)[&PeerId::from(0)]
                .is_empty()
        );
        assert_eq!(
            replacement_reasons(&reports, &moved_tenures(), &policy, now)[&PeerId::from(0)],
            vec![],
            "{tenure} s after the move"
        );
    }
}

#[test]
fn a_new_host_that_is_down_is_flagged_as_offline() {
    let policy = policy(3600, 90, 93);
    let now = MOVED_AT + DAY;
    // Up all the time on the old host, last seen at the move.
    let reports = peer_zero_reports(&long_running(1.0, 0.0, now), MOVED_AT);
    assert_eq!(
        replacement_reasons(&reports, &moved_tenures(), &policy, now)[&PeerId::from(0)],
        vec![ReplacementReason::Offline {
            last_seen_secs: MOVED_AT
        }]
    );
}

#[test]
fn a_flaky_new_host_is_flagged_once_its_bound_falls_below_the_floor() {
    // Up all the time on the old host, then 85% of the time. Assuming the
    // old host was always down, the weekly bound (w + 0.85(1 - w)) / (1 - w)
    // falls below 90% once w < 1/21, after 7 ln 21 = 21.3 days.
    let policy = policy(3600, 90, 93);
    for (days, flagged) in [(1, false), (20, false), (21, false), (22, true), (40, true)] {
        let now = MOVED_AT + days * DAY;
        let reports = peer_zero_reports(&long_running(1.0, 0.85, now), now);
        let reasons =
            &replacement_reasons(&reports, &moved_tenures(), &policy, now)[&PeerId::from(0)];
        let (week, month) = scores(reasons);
        assert_eq!(week.is_some(), flagged, "day {days}: {reasons:?}");
        // The monthly bound stays above 93% until day 78.
        assert_eq!(month, None, "day {days}");
    }
    // The bound at day 22 is (w + 0.85 (1 - w)) / (1 - w), w = exp(-22/7).
    let now = MOVED_AT + 22 * DAY;
    let w = (-22.0_f64 / 7.0).exp();
    let expected = ((w + 0.85 * (1.0 - w)) / (1.0 - w) * 10_000.0) as i32;
    let reports = peer_zero_reports(&long_running(1.0, 0.85, now), now);
    let week =
        scores(&replacement_reasons(&reports, &moved_tenures(), &policy, now)[&PeerId::from(0)]).0;
    assert!((i32::from(week.unwrap()) - expected).abs() <= 2);
}

#[test]
fn an_original_seat_in_a_young_federation_gets_its_raw_score() {
    // Two days old: coverage is well below 1. A 100% floor shows the scores.
    let policy = policy(3600, 100, 100);
    let formed_at = MOVED_AT;
    let now = formed_at + 2 * DAY;
    let windows = reported(formed_at, formed_at, 0.8, 0.8, now);
    assert!(windows[0].1 < 3000);
    let reports = peer_zero_reports(&windows, now);
    let raw = replacement_reasons(&reports, &Tenures::default(), &policy, now);
    assert_eq!(scores(&raw[&PeerId::from(0)]), (Some(8000), Some(8000)));
    assert_eq!(
        replacement_reasons(&reports, &tenures(formed_at, formed_at), &policy, now),
        raw
    );
    // Bound a minute after the earliest binding: that minute barely moves
    // the bound, 8000 * c / (c - 0.75 bp).
    let late = replacement_reasons(&reports, &tenures(formed_at, formed_at + 60), &policy, now);
    let (week, month) = scores(&late[&PeerId::from(0)]);
    assert!((8000..=8003).contains(&week.unwrap()));
    assert!((8000..=8003).contains(&month.unwrap()));
}

#[test]
fn a_moved_peer_in_a_young_federation_is_judged_by_its_bound() {
    let policy = policy(3600, 90, 93);
    let formed_at = MOVED_AT - 2 * DAY;
    // Up 25% of the time for the first two days, then 100% for one: not
    // flagged.
    let now = MOVED_AT + DAY;
    let reports = peer_zero_reports(&reported(formed_at, MOVED_AT, 0.25, 1.0, now), now);
    assert_eq!(
        replacement_reasons(&reports, &tenures(formed_at, MOVED_AT), &policy, now)
            [&PeerId::from(0)],
        vec![]
    );
    // Then 60% for five days. Coverage before the move was at most
    // 1 - exp(-2/7), so the weekly bound is s c / (c - (w - wf)), about
    // 66%. Assuming full coverage before the move would subtract w instead
    // and leave a bound above 100%.
    let now = MOVED_AT + 5 * DAY;
    let reports = peer_zero_reports(&reported(formed_at, MOVED_AT, 0.25, 0.6, now), now);
    let reasons = &replacement_reasons(&reports, &tenures(formed_at, MOVED_AT), &policy, now)
        [&PeerId::from(0)];
    let week = scores(reasons).0.unwrap();
    assert!((6000..7000).contains(&week), "{week}");
}

#[test]
fn a_binding_issued_in_the_future_counts_as_issued_now() {
    let policy = policy(3600, 90, 93);
    let now = MOVED_AT + 3 * DAY;
    let reports = peer_zero_reports(&long_running(0.25, 1.0, now), now);
    let formed_at = MOVED_AT - 365 * DAY;
    let at_now = replacement_reasons(&reports, &tenures(formed_at, now), &policy, now);
    assert_eq!(
        replacement_reasons(&reports, &tenures(formed_at, now + 30 * DAY), &policy, now),
        at_now
    );
    // A binding from now leaves no tenure to judge.
    assert_eq!(at_now[&PeerId::from(0)], vec![]);
    let tenures = tenures(formed_at, now + 30 * DAY);
    assert_eq!(
        tenures.uptime_bound(
            PeerId::from(0),
            PEER_AVAILABILITY_WINDOWS_SECS[0],
            2500,
            10_000,
            now
        ),
        None
    );
}

#[test]
fn a_short_tenure_gives_no_score_until_enough_is_observed() {
    let policy = policy(3600, 90, 93);
    // The new host holds 5% of a full 7-day coverage after 31,022 s.
    for (tenure, scored) in [(3_600, false), (30_000, false), (32_000, true)] {
        let now = MOVED_AT + tenure;
        // Down since the move but seen just now: only scores can qualify.
        let reports = peer_zero_reports(&long_running(0.0, 0.0, now), now);
        let reasons =
            &replacement_reasons(&reports, &moved_tenures(), &policy, now)[&PeerId::from(0)];
        assert_eq!(
            scores(reasons).0.is_some(),
            scored,
            "{tenure} s after the move"
        );
    }
}

#[test]
fn a_peer_without_a_binding_time_uses_the_raw_score() {
    let policy = policy(3600, 90, 93);
    let now = MOVED_AT + DAY;
    let reports = peer_zero_reports(&long_running(0.25, 1.0, now), now);
    let mut tenures = moved_tenures();
    tenures.bound_at.remove(&PeerId::from(0));
    assert_eq!(
        replacement_reasons(&reports, &tenures, &policy, now),
        replacement_reasons(&reports, &Tenures::default(), &policy, now)
    );
}
