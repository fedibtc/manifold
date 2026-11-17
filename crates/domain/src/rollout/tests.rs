//! Tests for staged-rollout schedules.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md#staged-rollouts`.

use super::*;

fn schedule(points: &[(u64, u8)]) -> RolloutSchedule {
    RolloutSchedule::new(
        points
            .iter()
            .map(|&(at_secs, percent)| RolloutPoint { at_secs, percent })
            .collect(),
    )
    .expect("test schedule is valid")
}

/// The largest draw a seat runs the feature with at `percent`, which is
/// `floor(percent / 100 * 2^32)`, or 2^32 - 1 at 100%, from exact arithmetic.
fn last_included(numerator: u128, denominator: u128) -> u32 {
    // percent = numerator / denominator; included iff draw * 100 < percent * 2^32.
    let bound = (numerator << 32).div_ceil(denominator * 100);
    u32::try_from(bound - 1).expect("bound is within the draw range")
}

fn assert_threshold(schedule: &RolloutSchedule, now_secs: u64, last: u32) {
    assert!(
        schedule.includes(RolloutDraw::new(last), now_secs),
        "draw {last} at {now_secs} must be included"
    );
    if let Some(first_excluded) = last.checked_add(1) {
        assert!(
            !schedule.includes(RolloutDraw::new(first_excluded), now_secs),
            "draw {first_excluded} at {now_secs} must be excluded"
        );
    }
}

#[test]
fn constant_percent_selects_draws_strictly_below_it() {
    let half = schedule(&[(1_000, 50)]);
    assert_threshold(&half, 1_000, (1 << 31) - 1);

    let none = schedule(&[(1_000, 0)]);
    assert!(!none.includes(RolloutDraw::new(0), 1_000));

    let all = schedule(&[(1_000, 100)]);
    assert!(all.includes(RolloutDraw::new(u32::MAX), 1_000));
}

#[test]
fn percent_moves_linearly_between_points() {
    // A quarter of the way from 10% to 90% is 30%.
    let ramp = schedule(&[(1_000, 10), (1_400, 90)]);
    assert_threshold(&ramp, 1_100, last_included(30, 1));
    assert!((ramp.percent_at(1_100) - 30.0).abs() < 1e-9);

    // A third of the way down from 100% to 0% is 66 2/3%.
    let down = schedule(&[(0, 100), (300, 0)]);
    assert_threshold(&down, 100, last_included(200, 3));
}

#[test]
fn percent_holds_the_first_point_before_it_and_the_last_after_it() {
    let ramp = schedule(&[(1_000, 40), (2_000, 100), (3_000, 20)]);
    assert_threshold(&ramp, 0, last_included(40, 1));
    assert_threshold(&ramp, 999, last_included(40, 1));
    assert_threshold(&ramp, 3_000, last_included(20, 1));
    assert_threshold(&ramp, u64::MAX, last_included(20, 1));
    assert!((ramp.percent_at(0) - 40.0).abs() < f64::EPSILON);
}

#[test]
fn a_point_time_takes_that_point_s_percent() {
    let peak = schedule(&[(0, 0), (100, 100), (200, 0)]);
    assert!(peak.includes(RolloutDraw::new(u32::MAX), 100));
    assert_threshold(&peak, 150, last_included(50, 1));
    assert_threshold(&peak, 99, last_included(99, 1));
    assert_threshold(&peak, 101, last_included(99, 1));
}

#[test]
fn the_widest_schedule_stays_exact() {
    // Just under halfway along a segment spanning the whole u64 range: the
    // percentage is a hair below 50%, so draw 2^31 is out and 2^31 - 1 in.
    let wide = schedule(&[(0, 0), (u64::MAX, 100)]);
    let now = u64::MAX / 2;
    let last = last_included(100 * u128::from(now), u128::from(u64::MAX));
    assert_eq!(last, (1 << 31) - 1);
    assert_threshold(&wide, now, last);
}

#[test]
fn higher_percent_selects_a_superset() {
    let ramp = schedule(&[(0, 0), (1_000, 100)]);
    let mut state = 0x9e37_79b9_u32;
    for _ in 0..10_000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let draw = RolloutDraw::new(state);
        let at = u64::from(state % 1_000);
        if ramp.includes(draw, at) {
            assert!(ramp.includes(draw, at + 1));
        }
    }
}

#[test]
fn schedules_round_trip_as_point_arrays() {
    let json = r#"{"aleph_backup_batch":[[1000,0],[2000,25]],"x":[[5,100]]}"#;
    let rollouts: Rollouts = serde_json::from_str(json).expect("valid rollouts");
    let feature = RolloutFeature::new("aleph_backup_batch").unwrap();
    assert_eq!(
        rollouts.get(&feature).unwrap().points(),
        [
            RolloutPoint {
                at_secs: 1_000,
                percent: 0
            },
            RolloutPoint {
                at_secs: 2_000,
                percent: 25
            },
        ]
    );
    assert_eq!(serde_json::to_string(&rollouts).unwrap(), json);
    assert_eq!(
        rollouts.percent_at(&RolloutFeature::new("absent").unwrap(), 0),
        0.0
    );
}

#[test]
fn invalid_rollouts_are_rejected() {
    let points = |count: u64| {
        (0..count)
            .map(|at| format!("[{at},1]"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let features = |count: usize| {
        (0..count)
            .map(|index| format!(r#""f{index}":[[0,1]]"#))
            .collect::<Vec<_>>()
            .join(",")
    };
    let long_name = "a".repeat(ROLLOUT_FEATURE_NAME_MAX_BYTES + 1);
    for invalid in [
        r#"{"a":[[0,1]],"a":[[0,2]]}"#.to_owned(),
        format!("{{{}}}", features(ROLLOUT_MAX_FEATURES + 1)),
        r#"{"a":[]}"#.to_owned(),
        format!(r#"{{"a":[{}]}}"#, points(ROLLOUT_MAX_POINTS as u64 + 1)),
        r#"{"a":[[5,1],[5,2]]}"#.to_owned(),
        r#"{"a":[[6,1],[5,2]]}"#.to_owned(),
        r#"{"a":[[0,101]]}"#.to_owned(),
        r#"{"a":[[0,-1]]}"#.to_owned(),
        r#"{"a":[[0,1.5]]}"#.to_owned(),
        r#"{"a":[[-1,1]]}"#.to_owned(),
        r#"{"a":[[0,1,2]]}"#.to_owned(),
        r#"{"a":[[0]]}"#.to_owned(),
        r#"{"A":[[0,1]]}"#.to_owned(),
        r#"{"a-b":[[0,1]]}"#.to_owned(),
        r#"{"":[[0,1]]}"#.to_owned(),
        format!(r#"{{"{long_name}":[[0,1]]}}"#),
        r#"[]"#.to_owned(),
    ] {
        assert!(
            serde_json::from_str::<Rollouts>(&invalid).is_err(),
            "{invalid} must be rejected"
        );
    }

    let longest_name = "a".repeat(ROLLOUT_FEATURE_NAME_MAX_BYTES);
    for valid in [
        format!("{{{}}}", features(ROLLOUT_MAX_FEATURES)),
        format!(r#"{{"a":[{}]}}"#, points(ROLLOUT_MAX_POINTS as u64)),
        format!(r#"{{"{longest_name}":[[0,0]]}}"#),
        r#"{"a_0":[[0,100]]}"#.to_owned(),
        r#"{}"#.to_owned(),
    ] {
        assert!(
            serde_json::from_str::<Rollouts>(&valid).is_ok(),
            "{valid} must be admitted"
        );
    }
}
