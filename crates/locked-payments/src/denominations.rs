//! Splitting a price into the mint's fixed denominations. Shared by the
//! payer (funding a quote) and the payee (quoting one), so it lives beside
//! neither.

use fedimint_core::Amount;

/// Maximum number of notes in one locked-payment quote.
///
/// A canonical power-of-two representation of any `u64` amount needs at most
/// one note per bit. Refusing larger, noncanonical representations keeps a
/// pathological mint tier set from turning quote generation into unbounded
/// work.
pub const MAX_LOCKED_PAYMENT_NOTES: usize = u64::BITS as usize;

/// Deterministic fixed-denomination breakdown used by quotes; `None` when
/// the price is not representable in at most [`MAX_LOCKED_PAYMENT_NOTES`] notes.
///
/// Standard mint denominations are powers of two millisatoshis. Each set
/// bit becomes one output, highest denomination first, so the issuance set
/// is minimal and canonical for the standard power-of-two tier sets. Greedy
/// may conservatively refuse a representable amount for a pathological
/// non-canonical set (for example `{4, 3}` and `6`), but can never quote an
/// invalid tier. Representations requiring more than 64 repeated notes were
/// previously accepted; callers must now treat them as unrepresentable.
pub fn quote_denominations(price_msats: u64, available: &[Amount]) -> Option<Vec<Amount>> {
    let mut available = available.to_vec();
    available.sort_unstable_by(|a, b| b.cmp(a));
    available.dedup();
    let mut remaining = price_msats;
    let mut selected = Vec::new();
    for tier in available {
        if tier.msats == 0 {
            continue;
        }
        let count = remaining / tier.msats;
        let remaining_capacity = MAX_LOCKED_PAYMENT_NOTES - selected.len();
        if count > remaining_capacity as u64 {
            return None;
        }
        selected.extend(std::iter::repeat_n(tier, count as usize));
        remaining %= tier.msats;
    }
    if remaining != 0 {
        return None;
    }
    Some(selected)
}

/// The smallest mint-v2 note worth issuing: mint-v2 denominations are powers
/// of two, and those at or below the 100 msat base fee cost more to spend
/// than they carry. Every multiple of it is representable in
/// [`economical_mint_v2_denominations`].
pub const MIN_ECONOMICAL_MINT_V2_NOTE_MSATS: u64 = 128;

/// The mint-v2 denominations both roles quote and refund in: the consensus
/// set from [`MIN_ECONOMICAL_MINT_V2_NOTE_MSATS`] up. One definition,
/// because the payee's quote and the payer's refund commitment must select
/// from the same tiers to agree.
pub fn economical_mint_v2_denominations() -> Vec<fedimint_mintv2_common::Denomination> {
    fedimint_mintv2_common::config::consensus_denominations()
        .filter(|denomination| denomination.amount().msats >= MIN_ECONOMICAL_MINT_V2_NOTE_MSATS)
        .collect()
}

/// `price_msats` rounded up to a multiple of
/// [`MIN_ECONOMICAL_MINT_V2_NOTE_MSATS`], the finest price a mint-v2 payer
/// can fund (mint-v1 tiers reach down to 1 msat, so it stays payable there).
/// `None` only when rounding up overflows `u64`.
pub fn payable_price_msats(price_msats: u64) -> Option<u64> {
    price_msats
        .div_ceil(MIN_ECONOMICAL_MINT_V2_NOTE_MSATS)
        .checked_mul(MIN_ECONOMICAL_MINT_V2_NOTE_MSATS)
}

#[cfg(test)]
mod tests;
