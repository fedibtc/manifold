use fedimint_core::Amount;

use super::*;

#[test]
fn price_must_be_representable_by_actual_tiers() {
    assert_eq!(
        quote_denominations(5, &[Amount::from_msats(4), Amount::from_msats(2)]),
        None
    );
    assert_eq!(
        quote_denominations(
            10,
            &[
                Amount::from_msats(8),
                Amount::from_msats(2),
                Amount::from_msats(1)
            ]
        )
        .unwrap(),
        [Amount::from_msats(8), Amount::from_msats(2)]
    );
}

#[test]
fn noncanonical_repeated_tiers_remain_deterministic() {
    assert_eq!(
        quote_denominations(6, &[Amount::from_msats(3)]).unwrap(),
        [Amount::from_msats(3), Amount::from_msats(3)]
    );
    assert_eq!(
        quote_denominations(6, &[Amount::from_msats(4), Amount::from_msats(3)]),
        None
    );
}

#[test]
fn note_count_is_bounded_before_expansion() {
    assert_eq!(
        quote_denominations(MAX_LOCKED_PAYMENT_NOTES as u64, &[Amount::from_msats(1)],)
            .unwrap()
            .len(),
        MAX_LOCKED_PAYMENT_NOTES
    );
    assert_eq!(
        quote_denominations(
            MAX_LOCKED_PAYMENT_NOTES as u64 + 1,
            &[Amount::from_msats(1)]
        ),
        None
    );
    assert_eq!(
        quote_denominations(u64::MAX, &[Amount::from_msats(1)]),
        None
    );
}

#[test]
fn zero_and_maximum_tiers_do_not_break_the_bound() {
    assert_eq!(quote_denominations(0, &[Amount::ZERO]), Some(Vec::new()));
    assert_eq!(quote_denominations(1, &[Amount::ZERO]), None);
    assert_eq!(
        quote_denominations(u64::MAX, &[Amount::from_msats(u64::MAX)]),
        Some(vec![Amount::from_msats(u64::MAX)])
    );
}
