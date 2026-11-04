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
                    ..Default::default()
                })
                .collect(),
        },
    );
}

/// What a reporter serves after resetting peer 0: no last-seen time, shared
/// coverage kept, and only what it saw since the reset for peer 0.
fn reset_availability(
    reports: &mut GuardianReports,
    reporter: u16,
    coverage: u16,
    score: Option<u16>,
) {
    let moved = PeerId::from(0);
    reports.availability.insert(
        PeerId::from(reporter),
        PeerAvailability {
            last_seen_secs: BTreeMap::new(),
            windows: PEER_AVAILABILITY_WINDOWS_SECS
                .into_iter()
                .map(|window_secs| WindowAvailability {
                    window_secs,
                    coverage_bps: 10_000,
                    peer_coverage_bps: BTreeMap::from([(moved, coverage)]),
                    peers: score.map(|score| (moved, score)).into_iter().collect(),
                })
                .collect(),
        },
    );
}

#[test]
fn eligibility_reset_drops_old_host_contributions_but_keeps_new_host_evidence() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    let moved = PeerId::from(0);
    let other = PeerId::from(3);
    for reporter in [1, 2, 3] {
        session(&mut reports, reporter, Some(900));
        reports
            .sessions
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .last_contribution
            .insert(other, Some(99));
    }
    let stale = vec![ReplacementReason::MissedSessions { sessions: 900 }];
    let before = replacement_reasons(&reports, &policy, 10_000);
    assert_eq!(before[&moved], stale);
    assert_eq!(before[&other], stale);

    // Up to f reporters missed the vote and still show the old host's gap.
    for reporter in [1, 2] {
        reset_availability(&mut reports, reporter, 0, None);
    }
    let after = replacement_reasons(&reports, &policy, 10_000);
    assert!(after[&moved].is_empty());
    assert_eq!(after[&other], before[&other]);

    // Every reporter reset and nobody has observed the new host yet.
    reset_availability(&mut reports, 3, 0, None);
    assert!(replacement_reasons(&reports, &policy, 10_000)[&moved].is_empty());

    // A new host that is down now shows at once, without a session gap.
    for reporter in [1, 2] {
        reports
            .sessions
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .disconnected
            .insert(moved);
    }
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![ReplacementReason::Disconnected]
    );

    // Its first fresh observations then score it, however few they are.
    for reporter in [1, 2, 3] {
        reset_availability(&mut reports, reporter, 10, Some(0));
    }
    let down = replacement_reasons(&reports, &policy, 10_000);
    assert!(down[&moved].contains(&ReplacementReason::LowWeekScore { basis_points: 0 }));
    assert!(down[&moved].contains(&ReplacementReason::LowMonthScore { basis_points: 0 }));
}

#[test]
fn eligibility_up_to_f_reset_claims_cannot_hide_a_session_gap() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    let moved = PeerId::from(0);
    session(&mut reports, 1, Some(900));
    session(&mut reports, 2, Some(800));
    session(&mut reports, 3, Some(0));
    reset_availability(&mut reports, 3, 0, None);
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![ReplacementReason::MissedSessions { sessions: 800 }]
    );
    // The peer's own report cannot exempt it from the others' observations.
    reset_availability(&mut reports, 0, 0, None);
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![ReplacementReason::MissedSessions { sessions: 800 }]
    );
}

#[test]
fn eligibility_healthy_replacement_overcomes_old_host_scores_without_changing_other_peers() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    let moved = PeerId::from(0);
    let other = PeerId::from(3);
    for reporter in [1, 2, 3] {
        availability(&mut reports, reporter, 9999, 2500, 2500);
        for window in &mut reports
            .availability
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .windows
        {
            window.peers.insert(other, 8000);
        }
    }
    let before = replacement_reasons(&reports, &policy, 10_000);
    assert_eq!(
        before[&moved],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 2500 },
            ReplacementReason::LowMonthScore { basis_points: 2500 },
        ]
    );
    assert_eq!(
        before[&other],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 8000 },
            ReplacementReason::LowMonthScore { basis_points: 8000 },
        ]
    );

    // Two observers have reset and seen a healthy successor. The third still
    // carries the old host's score; the other peer has not been reset.
    for reporter in [1, 2] {
        for window in &mut reports
            .availability
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .windows
        {
            window.peer_coverage_bps.insert(moved, 10);
            window.peers.insert(moved, 10_000);
        }
    }
    let after = replacement_reasons(&reports, &policy, 10_000);
    assert!(after[&moved].is_empty());
    assert_eq!(after[&other], before[&other]);
}

#[test]
fn eligibility_poor_uptime_after_reset_uses_fresh_coverage() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    let moved = PeerId::from(0);
    availability(&mut reports, 1, 9999, 8500, 9100);
    availability(&mut reports, 2, 9999, 8500, 9100);
    availability(&mut reports, 3, 9999, 10_000, 10_000);
    for window in &mut reports
        .availability
        .get_mut(&PeerId::from(2))
        .unwrap()
        .windows
    {
        window.coverage_bps = 100;
    }
    // Shared history differs: without the reset coverage, observer 2 is
    // excluded and the old host's healthy score masks the new host's failures.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&moved].is_empty());
    for reporter in [1, 2] {
        for window in &mut reports
            .availability
            .get_mut(&PeerId::from(reporter))
            .unwrap()
            .windows
        {
            window.peer_coverage_bps.insert(moved, 10);
        }
    }
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 8500 },
            ReplacementReason::LowMonthScore { basis_points: 9100 },
        ]
    );
}

#[test]
fn eligibility_session_fallback_requires_other_reporters_and_uses_policy_duration() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    session(&mut reports, 0, Some(900)); // Self-report cannot help.
    session(&mut reports, 1, Some(21));
    session(&mut reports, 2, None); // Unknown is not zero.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    session(&mut reports, 2, Some(20)); // Exactly one hour is not over the limit.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    session(&mut reports, 2, Some(21));
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::MissedSessions { sessions: 21 }]
    );
    assert!(
        replacement_reasons(&reports, &super::tests::policy(7200, 90, 93), 10_000)
            [&PeerId::from(0)]
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
        replacement_reasons(&reports, &policy(3600, 90, 93), 10_000)[&PeerId::from(0)],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 8500 },
            ReplacementReason::LowMonthScore { basis_points: 9100 }
        ]
    );
    assert!(
        replacement_reasons(&reports, &policy(3600, 85, 91), 10_000)[&PeerId::from(0)].is_empty()
    );
}

#[test]
fn eligibility_ignores_outsiders_and_needs_confirmed_absence() {
    let mut reports = reports();
    availability(&mut reports, 0, 1, 0, 0);
    availability(&mut reports, 1, 1, 10_000, 10_000);
    availability(&mut reports, 99, 1, 0, 0);
    assert!(
        replacement_reasons(&reports, &policy(3600, 90, 93), 10_000)[&PeerId::from(0)].is_empty()
    );
    availability(&mut reports, 2, 6400, 10_000, 10_000);
    assert!(
        replacement_reasons(&reports, &policy(3600, 90, 93), 10_000)[&PeerId::from(0)].is_empty()
    );
    availability(&mut reports, 2, 6399, 10_000, 10_000);
    assert_eq!(
        replacement_reasons(&reports, &policy(3600, 90, 93), 10_000)[&PeerId::from(0)],
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
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    session(&mut reports, 4, None); // Unknown contribution, but not disconnected.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    reports
        .sessions
        .get_mut(&PeerId::from(4))
        .unwrap()
        .disconnected
        .insert(PeerId::from(0));
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)],
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
        replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::Disconnected]
    );
    for report in reports.sessions.values_mut() {
        report
            .last_contribution
            .insert(PeerId::from(0), Some(report.session_count));
    }
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    for report in reports.sessions.values_mut() {
        report.last_contribution.insert(PeerId::from(0), None);
    }
    for reporter in [1, 2] {
        availability(&mut reports, reporter, 9999, 10_000, 10_000);
    }
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
}
