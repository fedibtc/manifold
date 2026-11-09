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

/// Prices observed in production adverts that a mint-v2 payer could not fund:
/// round sat amounts not divisible by 16 sats (128 msat).
#[test]
fn payable_price_is_the_next_price_a_mint_v2_payer_can_fund() {
    let tiers = economical_mint_v2_denominations()
        .into_iter()
        .map(|denomination| denomination.amount())
        .collect::<Vec<_>>();
    for (price, payable) in [
        (0, 0),
        (1, 128),
        (127, 128),
        (128, 128),
        (129, 256),
        (1_000_000, 1_000_064),
        (2_000_000, 2_000_000),
        (2_100_000, 2_100_096),
        (2_575_000, 2_575_104),
    ] {
        assert_eq!(payable_price_msats(price), Some(payable), "{price}");
        assert!(
            quote_denominations(payable, &tiers).is_some(),
            "{payable} must be fundable from mint-v2"
        );
    }
    assert_eq!(quote_denominations(2_100_000, &tiers), None);
    assert_eq!(payable_price_msats(u64::MAX - 127), Some(u64::MAX - 127));
    assert_eq!(payable_price_msats(u64::MAX - 126), None);
}
