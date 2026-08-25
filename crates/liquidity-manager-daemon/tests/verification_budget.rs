use super::*;

fn budget() -> VerificationBudget {
    VerificationBudget::new(Duration::from_secs(60), 3, 4)
}

#[test]
fn a_federation_spends_its_allowance_then_is_refused() {
    let budget = budget();
    let now = Instant::now();
    for attempt in 0..3 {
        assert!(budget.try_spend("fed-a", now), "attempt {attempt}");
    }
    assert!(!budget.try_spend("fed-a", now));
}

/// The bound is per federation, so one requester exhausting the federation
/// it holds an endorsement for must not affect any other.
#[test]
fn one_exhausted_federation_does_not_bound_another() {
    let budget = budget();
    let now = Instant::now();
    for _ in 0..3 {
        assert!(budget.try_spend("fed-a", now));
    }
    assert!(!budget.try_spend("fed-a", now));
    assert!(budget.try_spend("fed-b", now));
}

/// An honest requester that waited must not stay locked out.
#[test]
fn the_allowance_returns_in_the_next_window() {
    let budget = budget();
    let start = Instant::now();
    for _ in 0..3 {
        assert!(budget.try_spend("fed-a", start));
    }
    assert!(!budget.try_spend("fed-a", start + Duration::from_secs(59)));
    assert!(budget.try_spend("fed-a", start + Duration::from_secs(60)));
}

/// The map must stay bounded even when many authenticated federations spend.
#[test]
fn tracked_federations_stay_bounded() {
    let budget = budget();
    let now = Instant::now();
    for index in 0..100 {
        assert_eq!(budget.try_spend(&format!("fed-{index}"), now), index < 4);
    }
    assert_eq!(budget.entries.lock().expect("lock").len(), 4);
}

/// Authenticated map pressure must not refill a spent federation.
#[test]
fn map_pressure_does_not_refill_a_spent_federation() {
    let budget = budget();
    let now = Instant::now();
    for _ in 0..3 {
        assert!(budget.try_spend("fed-a", now));
    }
    // Fill the map with other authenticated federation identities.
    for index in 0..100 {
        budget.try_spend(&format!("filler-{index}"), now);
    }
    for _ in 0..10 {
        assert!(!budget.try_spend("fed-a", now));
    }
}
