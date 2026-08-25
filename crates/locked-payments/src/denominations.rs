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

/// The mint-v2 denominations both roles quote and refund in: the consensus
/// set with the dust tiers (at or below 100 msat) removed. One definition,
/// because the payee's quote and the payer's refund commitment must select
/// from the same tiers to agree.
pub fn economical_mint_v2_denominations() -> Vec<fedimint_mintv2_common::Denomination> {
    fedimint_mintv2_common::config::consensus_denominations()
        .filter(|denomination| denomination.amount().msats > 100)
        .collect()
}

#[cfg(test)]
mod tests;
