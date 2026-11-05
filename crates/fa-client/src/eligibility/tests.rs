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
        disconnected: BTreeMap::new(),
        availability: BTreeMap::new(),
    }
}

/// A status answer from `reporter`, naming the given peers as disconnected.
fn status(reports: &mut GuardianReports, reporter: u16, disconnected: &[u16]) {
    reports.disconnected.insert(
        PeerId::from(reporter),
        disconnected.iter().copied().map(PeerId::from).collect(),
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
fn eligibility_just_moved_guardian_needs_evidence_about_its_new_host() {
    let mut reports = reports();
    let policy = policy(3600, 90, 93);
    let moved = PeerId::from(0);

    // Status answers but every availability read failed. Nothing says the new
    // host is down, whatever the old host's contributions were.
    for reporter in [1, 2, 3] {
        status(&mut reports, reporter, &[]);
    }
    assert!(replacement_reasons(&reports, &policy, 10_000)[&moved].is_empty());

    // Availability answers too, with every reporter reset and nothing observed.
    for reporter in [1, 2, 3] {
        reset_availability(&mut reports, reporter, 0, None);
    }
    assert!(replacement_reasons(&reports, &policy, 10_000)[&moved].is_empty());

    // f reporters cannot call it down.
    status(&mut reports, 1, &[0]);
    assert!(replacement_reasons(&reports, &policy, 10_000)[&moved].is_empty());

    // f+1 can, at once and with or without availability.
    status(&mut reports, 2, &[0]);
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![ReplacementReason::Disconnected]
    );
    let availability = std::mem::take(&mut reports.availability);
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![ReplacementReason::Disconnected]
    );
    reports.availability = availability;

    // Fresh scores still qualify a connected new host that takes no part.
    for reporter in [1, 2] {
        status(&mut reports, reporter, &[]);
    }
    for reporter in [1, 2, 3] {
        reset_availability(&mut reports, reporter, 10, Some(0));
    }
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&moved],
        vec![
            ReplacementReason::LowWeekScore { basis_points: 0 },
            ReplacementReason::LowMonthScore { basis_points: 0 },
        ]
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
fn eligibility_recent_timestamps_override_disconnection_and_scores_use_policy() {
    let mut reports = reports();
    for peer in [1, 2] {
        status(&mut reports, peer, &[0]);
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
        status(&mut reports, reporter, &[0]);
    }
    let policy = policy(86_400, 90, 93);
    // Self and outsider reports cannot make three actual peers count as four.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    status(&mut reports, 4, &[]); // Answered, but not disconnected.
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
    status(&mut reports, 4, &[0]);
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
        status(&mut reports, reporter, &[0]);
    }
    assert_eq!(
        replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)],
        vec![ReplacementReason::Disconnected]
    );
    for reporter in [1, 2] {
        availability(&mut reports, reporter, 9999, 10_000, 10_000);
    }
    assert!(replacement_reasons(&reports, &policy, 10_000)[&PeerId::from(0)].is_empty());
}
