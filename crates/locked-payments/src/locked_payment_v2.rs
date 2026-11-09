//! Cryptographic primitives for mint-v2 key-locked payments.

use std::collections::BTreeSet;

use fedimint_client_module::transaction::{ClientOutput, ClientOutputBundle};
use fedimint_core::Amount;
use fedimint_core::core::{IntoDynInstance as _, ModuleInstanceId};
use fedimint_core::encoding::Decodable as _;
use fedimint_core::module::Amounts;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::secp256k1::{Message, SECP256K1};
use fedimint_core::transaction::{Transaction, TransactionSignature};
use fedimint_derive_secret::DerivableSecret;
use fedimint_mintv2_client::SpendableNote;
use fedimint_mintv2_client::issuance::NoteIssuanceRequest;
use fedimint_mintv2_common::{Denomination, MintInput, MintOutput, Note};
use tbs::{BlindedMessage, BlindedSignature};

use crate::denominations::{MAX_LOCKED_PAYMENT_NOTES, MIN_ECONOMICAL_MINT_V2_NOTE_MSATS};
use crate::locked_payment::{self, LockedPaymentError};

/// Public mint-v2 issuance output carried by quote/refund terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssuanceRequest {
    pub denomination: Denomination,
    pub blind_nonce: BlindedMessage,
    pub tweak: [u8; 16],
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum LockedPaymentV2Error {
    #[error("issuance request count does not match signature count")]
    IssuanceCountMismatch,
    #[error("issuance request does not match the private quote root")]
    IssuanceMismatch,
    #[error("invalid mint-v2 denomination {0}")]
    InvalidDenomination(u64),
    #[error("mint signature is invalid")]
    InvalidMintSignature,
    #[error("payment contains a duplicate note nonce")]
    DuplicateNoteNonce,
    #[error("refund outputs do not balance the paid notes after mint fees")]
    InvalidRefundAmount,
}

/// Most a mint-v2 refund may leave unclaimed: less than the smallest
/// economical note. Fedimint accepts a transaction whose inputs exceed its
/// outputs plus fees from core consensus 2.1 (Fedimint v0.10), which every
/// mint-v2 federation runs; the excess stays with the federation.
pub const MAX_REFUND_LEFTOVER_MSATS: u64 = MIN_ECONOMICAL_MINT_V2_NOTE_MSATS - 1;

/// The refund rule the payee enforces for mint-v2: the refund outputs plus
/// every mint fee fit within the paid notes and leave at most
/// [`MAX_REFUND_LEFTOVER_MSATS`] behind. The payer chooses the outputs
/// ([`refund_denominations`]); the payee checks only this property, so the
/// two need not run the same search.
pub fn refund_balances(
    paid_msats: u64,
    input_fees_msats: u64,
    refund_msats: u64,
    output_fees_msats: u64,
) -> bool {
    refund_msats
        .checked_add(input_fees_msats)
        .and_then(|spent| spent.checked_add(output_fees_msats))
        .and_then(|spent| paid_msats.checked_sub(spent))
        .is_some_and(|leftover| leftover <= MAX_REFUND_LEFTOVER_MSATS)
}

/// Choose mint-v2 refund outputs for `target_msats` (the paid amount after
/// input fees). The exact search of
/// [`locked_payment::refund_denominations`] runs first, so every refund it
/// finds is unchanged and payees that still require an exact balance
/// agree. When it finds none, take the fewest notes whose outputs and
/// output fees leave at most [`MAX_REFUND_LEFTOVER_MSATS`]: 2,100,096 msat
/// paid in five notes at a 10 msat fee refunds in 4 notes and leaves
/// 38 msat, where an exact refund needs 59 notes.
pub fn refund_denominations(
    available: &[Amount],
    output_fee: impl Fn(Amount) -> Amount,
    target_msats: u64,
) -> Result<Vec<Amount>, LockedPaymentError> {
    locked_payment::refund_denominations(available, &output_fee, target_msats).or_else(|error| {
        fewest_notes_within_leftover(available, &output_fee, target_msats).ok_or(error)
    })
}

/// Fewest notes, at most [`MAX_LOCKED_PAYMENT_NOTES`], whose amounts plus
/// output fees leave at most [`MAX_REFUND_LEFTOVER_MSATS`] of `target_msats`.
/// For each note count it estimates the fees at the smallest tier's fee,
/// splits the largest whole multiple of the smallest tier greedily, halves
/// the largest note until the count is reached, and keeps the result only if
/// its real fees satisfy the bound. The refund value only falls as the count
/// grows, so the first count that works also loses the least. Largest first.
fn fewest_notes_within_leftover(
    available: &[Amount],
    output_fee: &impl Fn(Amount) -> Amount,
    target_msats: u64,
) -> Option<Vec<Amount>> {
    let mut tiers = available.to_vec();
    tiers.sort_unstable_by(|a, b| b.cmp(a));
    tiers.dedup();
    let smallest = *tiers.last()?;
    if smallest.msats == 0 {
        return None;
    }
    for count in 1..=MAX_LOCKED_PAYMENT_NOTES {
        let estimated_fees = output_fee(smallest).msats.checked_mul(count as u64)?;
        let Some(budget) = target_msats.checked_sub(estimated_fees) else {
            break;
        };
        let mut remaining = budget / smallest.msats * smallest.msats;
        let mut notes = Vec::new();
        for &tier in &tiers {
            let take = remaining / tier.msats;
            if take > (count - notes.len()) as u64 {
                break;
            }
            notes.extend(std::iter::repeat_n(tier, take as usize));
            remaining %= tier.msats;
        }
        if remaining != 0 || notes.is_empty() {
            continue;
        }
        while notes.len() < count {
            let largest = notes[0];
            let half = Amount::from_msats(largest.msats / 2);
            if largest.msats % 2 != 0 || !tiers.contains(&half) {
                break;
            }
            notes[0] = half;
            notes.push(half);
            notes.sort_unstable_by(|a, b| b.cmp(a));
        }
        if notes.len() != count {
            continue;
        }
        let refund = notes.iter().map(|note| note.msats).sum::<u64>();
        let fees = notes
            .iter()
            .try_fold(0u64, |sum, note| sum.checked_add(output_fee(*note).msats))?;
        if refund_balances(target_msats, 0, refund, fees) {
            return Some(notes);
        }
    }
    None
}

pub fn denomination_from_amount(amount_msats: u64) -> Result<Denomination, LockedPaymentV2Error> {
    if !amount_msats.is_power_of_two() {
        return Err(LockedPaymentV2Error::InvalidDenomination(amount_msats));
    }
    let exponent = amount_msats.trailing_zeros();
    let exponent = u8::try_from(exponent)
        .map_err(|_| LockedPaymentV2Error::InvalidDenomination(amount_msats))?;
    let denomination = Denomination(exponent);
    if denomination.amount().msats != amount_msats {
        return Err(LockedPaymentV2Error::InvalidDenomination(amount_msats));
    }
    Ok(denomination)
}

pub fn decode_blinded_message(bytes: &[u8]) -> Result<BlindedMessage, LockedPaymentV2Error> {
    BlindedMessage::consensus_decode_whole(bytes, &ModuleDecoderRegistry::default())
        .map_err(|_| LockedPaymentV2Error::IssuanceMismatch)
}

pub fn decode_blinded_signature(bytes: &[u8]) -> Result<BlindedSignature, LockedPaymentV2Error> {
    BlindedSignature::consensus_decode_whole(bytes, &ModuleDecoderRegistry::default())
        .map_err(|_| LockedPaymentV2Error::InvalidMintSignature)
}

pub fn issuance_request(
    root: &DerivableSecret,
    public: IssuanceRequest,
) -> Result<NoteIssuanceRequest, LockedPaymentV2Error> {
    let request = NoteIssuanceRequest::new(public.denomination, public.tweak, root);
    if request.blinded_message() != public.blind_nonce {
        return Err(LockedPaymentV2Error::IssuanceMismatch);
    }
    Ok(request)
}

/// Derive standard-recoverable issuance requests under a wallet's global
/// client root, through the same module-root path fedimint-client uses.
/// Both roles run this: the payee derives real quote outputs, and the
/// shared refund preparation derives its fee-shape template.
pub fn derive_standard_issuance_requests(
    global_root_secret: &DerivableSecret,
    federation_id: fedimint_core::config::FederationId,
    mint_module: ModuleInstanceId,
    denominations: &[Denomination],
    tweaks: &[[u8; 16]],
) -> Result<(Vec<IssuanceRequest>, Vec<NoteIssuanceRequest>), LockedPaymentV2Error> {
    let root = crate::standard_module_root_secret(global_root_secret, federation_id, mint_module);
    derive_issuance_requests(&root, denominations, tweaks)
}

pub fn derive_issuance_requests(
    root: &DerivableSecret,
    denominations: &[Denomination],
    tweaks: &[[u8; 16]],
) -> Result<(Vec<IssuanceRequest>, Vec<NoteIssuanceRequest>), LockedPaymentV2Error> {
    if denominations.len() != tweaks.len() {
        return Err(LockedPaymentV2Error::IssuanceCountMismatch);
    }
    let private = denominations
        .iter()
        .copied()
        .zip(tweaks.iter().copied())
        .map(|(denomination, tweak)| NoteIssuanceRequest::new(denomination, tweak, root))
        .collect::<Vec<_>>();
    let public = private
        .iter()
        .map(|request| IssuanceRequest {
            denomination: request.denomination,
            blind_nonce: request.blinded_message(),
            tweak: request.tweak,
        })
        .collect();
    Ok((public, private))
}

pub fn foreign_output_bundle(
    mint_module: ModuleInstanceId,
    issuance: &[IssuanceRequest],
) -> ClientOutputBundle {
    ClientOutputBundle::new_no_sm(
        issuance
            .iter()
            .map(|request| ClientOutput {
                output: MintOutput::new_v0(
                    request.denomination,
                    request.blind_nonce,
                    request.tweak,
                ),
                amounts: Amounts::new_bitcoin(request.denomination.amount()),
            })
            .collect(),
    )
    .into_dyn(mint_module)
}

/// Refuse a note set containing a repeated nonce: duplicates would collapse
/// into one spendable note while still counting twice toward the payment.
pub fn ensure_distinct_note_nonces(notes: &[SpendableNote]) -> Result<(), LockedPaymentV2Error> {
    let mut nonces = BTreeSet::new();
    if notes
        .iter()
        .any(|note| !nonces.insert(note.keypair.public_key()))
    {
        return Err(LockedPaymentV2Error::DuplicateNoteNonce);
    }
    Ok(())
}

pub fn build_refund_transaction(
    mint_module: ModuleInstanceId,
    paid_notes: &[SpendableNote],
    refund_issuance: &[IssuanceRequest],
    nonce: [u8; 8],
) -> Transaction {
    let inputs = paid_notes
        .iter()
        .map(|note| {
            MintInput::new_v0(Note {
                denomination: note.denomination,
                nonce: note.keypair.public_key(),
                signature: note.signature,
            })
            .into_dyn(mint_module)
        })
        .collect::<Vec<_>>();
    let outputs = refund_issuance
        .iter()
        .map(|request| {
            MintOutput::new_v0(request.denomination, request.blind_nonce, request.tweak)
                .into_dyn(mint_module)
        })
        .collect::<Vec<_>>();
    let txid = Transaction::tx_hash_from_parts(&inputs, &outputs, nonce);
    let message = Message::from_digest(*txid.as_ref());
    let signatures = paid_notes
        .iter()
        .map(|note| SECP256K1.sign_schnorr_no_aux_rand(&message, &note.keypair))
        .collect();
    Transaction {
        inputs,
        outputs,
        nonce,
        signatures: TransactionSignature::NaiveMultisig(signatures),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bls12_381::Scalar;
    use fedimint_core::Amount;
    use fedimint_mintv2_common::verify_note;
    use tbs::{
        AggregatePublicKey, SecretKeyShare, aggregate_public_key_shares, aggregate_signature_shares,
    };

    use super::*;

    fn economical_tiers() -> Vec<Amount> {
        crate::denominations::economical_mint_v2_denominations()
            .into_iter()
            .map(Denomination::amount)
            .collect()
    }

    /// The paid amount after input fees, for `price_msats` paid in the
    /// quote's greedy notes under a base-only `fee_msats`.
    fn refund_target(tiers: &[Amount], price_msats: u64, fee_msats: u64) -> u64 {
        let paid = crate::denominations::quote_denominations(price_msats, tiers).unwrap();
        price_msats - paid.len() as u64 * fee_msats
    }

    #[test]
    fn refund_balance_admits_a_leftover_below_one_smallest_note() {
        // 1,000 paid, 20 input fees, 30 output fees: 950 refunded is exact,
        // 823 leaves 127, 822 leaves 128, 951 overspends.
        assert!(refund_balances(1_000, 20, 950, 30));
        assert!(refund_balances(1_000, 20, 823, 30));
        assert!(!refund_balances(1_000, 20, 822, 30));
        assert!(!refund_balances(1_000, 20, 951, 30));
        assert!(!refund_balances(u64::MAX, 1, u64::MAX, 0));
    }

    /// 2,100,096 msat is paid in five notes (2^21 + 2048 + 512 + 256 +
    /// 128). At a 10 msat fee the exact search finds no refund (one needs
    /// 59 notes); the fewest-notes search refunds 2^21 + 2048 + 512 + 256 =
    /// 2,099,968 msat in 4 notes, losing 50 + 40 fees + 38 leftover.
    #[test]
    fn a_refund_the_exact_search_misses_takes_the_fewest_notes() {
        let tiers = economical_tiers();
        let fee = |_| Amount::from_msats(10);
        let target = refund_target(&tiers, 2_100_096, 10);
        assert_eq!(target, 2_100_046);
        assert_eq!(
            locked_payment::refund_denominations(&tiers, fee, target),
            Err(LockedPaymentError::UnrepresentableRefund)
        );
        let refund = refund_denominations(&tiers, fee, target).unwrap();
        assert_eq!(
            refund.iter().map(|note| note.msats).collect::<Vec<_>>(),
            [2_097_152, 2048, 512, 256]
        );
        assert_eq!(target - 2_099_968 - 4 * 10, 38);
    }

    /// Over 128 msat grid prices from 1,000 to 10,000,000 sat, under the
    /// base-only fees seen in practice, the fewest-notes search alone finds a
    /// refund within the rule and the note limit for every price, so the
    /// fallback never fails. It is called directly because the exact search
    /// is slow in debug builds when it fails; the stride keeps the run short.
    #[test]
    fn every_payable_price_refunds_within_the_rule() {
        let tiers = economical_tiers();
        for fee_msats in [0, 10, 100, 128, 200] {
            let fee = |_| Amount::from_msats(fee_msats);
            for price in (1_000_064u64..=10_000_000_000).step_by(128 * 9_973) {
                let target = refund_target(&tiers, price, fee_msats);
                let refund = fewest_notes_within_leftover(&tiers, &fee, target)
                    .unwrap_or_else(|| panic!("price {price} fee {fee_msats}"));
                let refunded = refund.iter().map(|note| note.msats).sum::<u64>();
                assert!(refund.len() <= MAX_LOCKED_PAYMENT_NOTES);
                assert!(
                    refund_balances(target, 0, refunded, refund.len() as u64 * fee_msats),
                    "price {price} fee {fee_msats}"
                );
            }
        }
    }

    /// Where the exact search finds a refund it is returned unchanged, so a
    /// payee that still requires an exact balance agrees with this payer.
    #[test]
    fn an_exact_refund_is_unchanged() {
        let tiers = economical_tiers();
        let fee = |_| Amount::from_msats(10);
        for price in [20_480, 1_000_064, 50_000_000] {
            let target = refund_target(&tiers, price, 10);
            let exact = locked_payment::refund_denominations(&tiers, fee, target).unwrap();
            assert_eq!(refund_denominations(&tiers, fee, target).unwrap(), exact);
        }
    }

    /// Offline mirror of the production finalize-and-verify step, which
    /// delegates to `MintClientModule::finalize_external_issuance` and so
    /// needs a live client these tests do not have.
    fn verify_finalized_notes(
        requests: &[NoteIssuanceRequest],
        signatures: &[BlindedSignature],
        aggregate_keys: &BTreeMap<Denomination, AggregatePublicKey>,
    ) -> Result<Vec<SpendableNote>, LockedPaymentV2Error> {
        if requests.len() != signatures.len() {
            return Err(LockedPaymentV2Error::IssuanceCountMismatch);
        }
        let notes = requests
            .iter()
            .zip(signatures)
            .map(|(request, signature)| {
                let note = request.finalize(*signature);
                let aggregate_key = aggregate_keys.get(&request.denomination).ok_or(
                    LockedPaymentV2Error::InvalidDenomination(request.denomination.amount().msats),
                )?;
                let public_note = Note {
                    denomination: note.denomination,
                    nonce: note.keypair.public_key(),
                    signature: note.signature,
                };
                if !verify_note(public_note, *aggregate_key) {
                    return Err(LockedPaymentV2Error::InvalidMintSignature);
                }
                Ok(note)
            })
            .collect::<Result<Vec<_>, _>>()?;
        ensure_distinct_note_nonces(&notes)?;
        Ok(notes)
    }

    #[test]
    fn derives_verifies_and_refunds_mint_v2_notes() {
        let root = DerivableSecret::new_root(&[42; 64], b"fman-v2-test");
        let denominations = [Denomination(10), Denomination(11)];
        let tweaks = [[1; 16], [2; 16]];
        let (public, private) = derive_issuance_requests(&root, &denominations, &tweaks).unwrap();
        assert_eq!(
            derive_issuance_requests(&root, &denominations, &tweaks)
                .unwrap()
                .0,
            public
        );

        let mut keys = BTreeMap::new();
        let mut signatures = Vec::new();
        for (index, request) in private.iter().enumerate() {
            let secret = SecretKeyShare(Scalar::from(index as u64 + 7));
            let public_shares = BTreeMap::from([(0, tbs::derive_pk_share(&secret))]);
            keys.insert(
                request.denomination,
                aggregate_public_key_shares(&public_shares),
            );
            let shares =
                BTreeMap::from([(0, tbs::sign_message(request.blinded_message(), secret))]);
            signatures.push(aggregate_signature_shares(&shares));
        }

        let notes = verify_finalized_notes(&private, &signatures, &keys).unwrap();
        let refund_root = DerivableSecret::new_root(&[99; 64], b"fi-v2-refund");
        let (refund, _) =
            derive_issuance_requests(&refund_root, &[Denomination(11)], &[[3; 16]]).unwrap();
        let transaction = build_refund_transaction(4, &notes, &refund, [3; 8]);
        transaction
            .validate_signatures(
                &notes
                    .iter()
                    .map(|note| note.keypair.public_key())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert!(
            !fedimint_core::encoding::Encodable::consensus_encode_to_vec(&transaction).is_empty()
        );
    }

    #[test]
    fn rejects_a_duplicate_finalized_note_nonce() {
        let root = DerivableSecret::new_root(&[42; 64], b"fman-v2-test");
        let (public, private) =
            derive_issuance_requests(&root, &[Denomination(10)], &[[1; 16]]).unwrap();
        let secret = SecretKeyShare(Scalar::from(7u64));
        let public_shares = BTreeMap::from([(0, tbs::derive_pk_share(&secret))]);
        let signature_shares =
            BTreeMap::from([(0, tbs::sign_message(public[0].blind_nonce, secret))]);
        let signature = aggregate_signature_shares(&signature_shares);

        let err = verify_finalized_notes(
            &[private[0].clone(), private[0].clone()],
            &[signature, signature],
            &BTreeMap::from([(
                Denomination(10),
                aggregate_public_key_shares(&public_shares),
            )]),
        )
        .unwrap_err();
        assert_eq!(err, LockedPaymentV2Error::DuplicateNoteNonce);
    }

    #[test]
    fn economical_v2_tiers_can_balance_refund_fees_exactly() {
        let tiers = (7..14)
            .map(|exponent| Denomination(exponent).amount())
            .collect::<Vec<_>>();
        let paid_after_two_input_fees = 10_240 - 200;
        let refund = crate::locked_payment::refund_denominations(
            &tiers,
            |_| Amount::from_msats(100),
            paid_after_two_input_fees,
        )
        .unwrap();
        assert_eq!(
            refund.iter().map(|amount| amount.msats + 100).sum::<u64>(),
            paid_after_two_input_fees
        );
        assert!(refund.iter().all(|amount| amount.msats > 100));
    }
}
