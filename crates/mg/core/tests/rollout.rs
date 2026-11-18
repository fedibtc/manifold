//! Tests for rollout draws and the rollout log.

use super::*;

fn seat(index: u16) -> SeatId {
    SeatId::new(format!("{index:064x}")).unwrap()
}

fn feature(name: &str) -> RolloutFeature {
    RolloutFeature::new(name).unwrap()
}

fn rollouts(json: &str) -> Rollouts {
    serde_json::from_str(json).unwrap()
}

/// Seed `0, 1, ..., 31`.
fn test_seed() -> RolloutSeed {
    RolloutSeed(std::array::from_fn(|index| u8::try_from(index).unwrap()))
}

/// The draw format is fixed: a seat's draw must not change across releases,
/// or a rollout would reshuffle which seats run a feature. The expected
/// values are SHA-256 computed outside this code.
#[test]
fn draws_follow_the_pinned_format() {
    let seat_id = SeatId::new("ab".repeat(32)).unwrap();
    let aleph = feature("aleph_backup_batch");
    assert_eq!(
        test_seed().draw(RolloutUnit::Seat(&seat_id), &aleph),
        RolloutDraw::new(1_157_731_906)
    );
    assert_eq!(
        test_seed().draw(RolloutUnit::Mg, &aleph),
        RolloutDraw::new(2_412_067_763)
    );
}

/// At 50%, about half the seats run each feature and about a quarter run
/// both: draws are independent between features and between seats. A draw
/// that ignored the feature would put the same half in both; one that ignored
/// the seat would put all or none.
#[test]
fn draws_are_independent_between_seats_and_features() {
    let seed = test_seed();
    let half = rollouts(r#"{"a":[[0,50]],"b":[[0,50]]}"#);
    let (a, b) = (feature("a"), feature("b"));
    let (mut in_a, mut in_b, mut in_both) = (0, 0, 0);
    for index in 0..2_000 {
        let seat_id = seat(index);
        let runs = |feature: &RolloutFeature| {
            half.get(feature)
                .unwrap()
                .includes(seed.draw(RolloutUnit::Seat(&seat_id), feature), 0)
        };
        let (runs_a, runs_b) = (runs(&a), runs(&b));
        in_a += usize::from(runs_a);
        in_b += usize::from(runs_b);
        in_both += usize::from(runs_a && runs_b);
    }
    // Binomial standard deviations are about 22 and 19; allow 5 of them.
    assert!((890..=1_110).contains(&in_a), "{in_a}");
    assert!((890..=1_110).contains(&in_b), "{in_b}");
    assert!((405..=595).contains(&in_both), "{in_both}");
}

/// The MG draw is not any seat's, and another seed draws differently.
#[test]
fn scopes_and_seeds_draw_independently() {
    let seed = test_seed();
    let other = RolloutSeed([7; 32]);
    let aleph = feature("aleph_backup_batch");
    let mg = seed.draw(RolloutUnit::Mg, &aleph);
    assert_ne!(mg, other.draw(RolloutUnit::Mg, &aleph));
    for index in 0..256 {
        let seat_id = seat(index);
        let draw = seed.draw(RolloutUnit::Seat(&seat_id), &aleph);
        assert_ne!(draw, mg);
        assert_ne!(draw, other.draw(RolloutUnit::Seat(&seat_id), &aleph));
    }
}

/// The first start stores a seed; later starts, even with another fresh
/// candidate, keep it, so every draw stays the same.
#[tokio::test]
async fn the_seed_persists_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let (seat_id, aleph) = (seat(1), feature("aleph_backup_batch"));
    let first = {
        let db = Db::open(dir.path()).await.unwrap();
        let seed = RolloutSeed::load(&db).await.unwrap();
        assert_eq!(db.rollout_seed([9; 32]).await.unwrap(), seed.0);
        seed
    };
    let db = Db::open(dir.path()).await.unwrap();
    let again = RolloutSeed::load(&db).await.unwrap();
    assert_eq!(again.0, first.0);
    assert_eq!(
        again.draw(RolloutUnit::Seat(&seat_id), &aleph),
        first.draw(RolloutUnit::Seat(&seat_id), &aleph)
    );
    // A new data root draws a new seed.
    let elsewhere = tempfile::tempdir().unwrap();
    let fresh = RolloutSeed::load(&Db::open(elsewhere.path()).await.unwrap())
        .await
        .unwrap();
    assert_ne!(fresh.0, first.0);
}

/// The log reports every first state, then only changes: seats joining a
/// ramp, a dropped feature's running seats, and a new seat's first state.
#[test]
fn the_log_reports_only_changes() {
    let seed = test_seed();
    let ramp = rollouts(r#"{"a":[[0,0],[1000,100]],"b":[[0,0]]}"#);
    let seats: Vec<_> = (0..200).map(seat).collect();
    let mut log = RolloutLog::default();

    let first = log.update(&seed, &ramp, &seats, 0);
    assert_eq!(first.len(), 400);
    assert!(first.iter().all(|flip| !flip.enabled));
    assert_eq!(log.update(&seed, &ramp, &seats, 0), []);

    // Halfway up the ramp, the seats whose draw is now below 50% turn on,
    // and only those.
    let halfway = log.update(&seed, &ramp, &seats, 500);
    let expected: Vec<_> = seats
        .iter()
        .filter(|seat_id| {
            ramp.get(&feature("a"))
                .unwrap()
                .includes(seed.draw(RolloutUnit::Seat(seat_id), &feature("a")), 500)
        })
        .cloned()
        .collect();
    assert!((60..=140).contains(&expected.len()), "{}", expected.len());
    assert_eq!(
        halfway,
        expected
            .iter()
            .map(|seat_id| Flip {
                seat_id: seat_id.clone(),
                feature: feature("a"),
                enabled: true,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(log.update(&seed, &ramp, &seats, 500), []);

    // Dropping `a` turns its running seats off; dropping `b`, which ran
    // nowhere, and a decommissioned seat log nothing. A new seat logs its
    // first state.
    let mut later_seats = seats[1..].to_vec();
    later_seats.push(seat(500));
    let dropped = log.update(&seed, &rollouts("{}"), &later_seats, 500);
    let mut turned_off: Vec<_> = expected
        .iter()
        .filter(|seat_id| **seat_id != seats[0])
        .map(|seat_id| Flip {
            seat_id: seat_id.clone(),
            feature: feature("a"),
            enabled: false,
        })
        .collect();
    turned_off.sort_by(|left, right| left.seat_id.cmp(&right.seat_id));
    assert_eq!(dropped, turned_off);
    // Republished at 100%, every live seat logs its first state again.
    let republished = log.update(&seed, &ramp, &later_seats, 1_000);
    assert_eq!(republished.len(), 2 * later_seats.len());
    assert!(
        republished
            .iter()
            .all(|flip| flip.enabled == (flip.feature == feature("a")))
    );
}
