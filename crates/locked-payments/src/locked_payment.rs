//! Cryptographic primitives for mint-v1 key-locked payments.
//!
//! This module deliberately does not allocate from fedimint-client's note
//! index tree. A quote's note keys are a pure derivation from the FMan wallet
//! root and caller-provided canonical quote terms.

use std::collections::{BTreeMap, BTreeSet};

use bls12_381::Scalar;
use fedimint_client_module::transaction::{ClientOutput, ClientOutputBundle};
use fedimint_core::Amount;
use fedimint_core::core::{IntoDynInstance as _, ModuleInstanceId};
use fedimint_core::encoding::Decodable as _;
use fedimint_core::module::Amounts;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::secp256k1::{Keypair, Message, PublicKey, SECP256K1, SecretKey};
use fedimint_core::transaction::{Transaction, TransactionSignature};
use fedimint_mint_common::{BlindNonce, MintInput, MintOutput, Nonce, Note};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tbs::{AggregatePublicKey, BlindedSignature, BlindedSignatureShare, BlindingKey};

use crate::denominations::MAX_LOCKED_PAYMENT_NOTES;

const QUOTE_NOTE_INFO: &[u8] = b"fman/v1/quote-note";

/// Public mint-v1 issuance request included in a quote (or refund request).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IssuanceRequest {
    pub amount: Amount,
    pub blind_nonce: BlindNonce,
}

/// Per-note secret material, rederived from the wallet root and the quote's
/// public randomness whenever it is needed — it never travels or persists.
///
/// Byte representations keep this type stable and serializable without
/// exposing secret values through `Debug`.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NoteSecrets {
    pub amount: Amount,
    spend_key: [u8; 32],
    blinding_key: [u8; 32],
}

impl std::fmt::Debug for NoteSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteSecrets")
            .field("amount", &self.amount)
            .finish_non_exhaustive()
    }
}

/// A verified mint-v1 note together with the key required to spend it.
#[derive(Clone)]
pub struct SpendableLockedNote {
    pub amount: Amount,
    pub note: Note,
    spend_key: SecretKey,
}

impl std::fmt::Debug for SpendableLockedNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpendableLockedNote")
            .field("amount", &self.amount)
            .field("note", &self.note)
            .finish_non_exhaustive()
    }
}

impl SpendableLockedNote {
    /// The client-wallet form of this note: the payer reissues verified
    /// refund notes with it, the payee its handoff notes.
    pub fn client_spendable_note(&self) -> fedimint_mint_client::SpendableNote {
        fedimint_mint_client::SpendableNote {
            signature: self.note.signature,
            spend_key: Keypair::from_secret_key(SECP256K1, &self.spend_key),
        }
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum LockedPaymentError {
    #[error("issuance request count does not match secret or signature count")]
    IssuanceCountMismatch,
    #[error("issuance request does not match its derived secret")]
    IssuanceMismatch,
    #[error("payment uses unsupported denomination {0}")]
    UnsupportedDenomination(Amount),
    #[error("mint signature is invalid")]
    InvalidMintSignature,
    #[error("payment contains a duplicate note nonce")]
    DuplicateNoteNonce,
    #[error("no signature shares were supplied")]
    NoSignatureShares,
    #[error("refund outputs do not balance the paid notes after mint fees")]
    InvalidRefundAmount,
    #[error("refund amount cannot be represented exactly by fee-bearing mint tiers")]
    UnrepresentableRefund,
}

/// Choose refund outputs satisfying the mint transaction equation exactly.
///
/// Each candidate coin costs `denomination + output_fee(denomination)` from
/// the transaction's input side. The result uses only configured tiers and
/// equals `target_msats` exactly; it first takes large coins greedily, then
/// replaces the smallest selected coins until a bounded minimum-note dynamic
/// program can repair the fee-sized remainder. Representations requiring more
/// than [`MAX_LOCKED_PAYMENT_NOTES`] outputs are refused before expansion.
pub fn refund_denominations(
    available: &[Amount],
    output_fee: impl Fn(Amount) -> Amount,
    target_msats: u64,
) -> Result<Vec<Amount>, LockedPaymentError> {
    const REPAIR_LIMIT_MSATS: u64 = 1_000_000;

    let mut coins = available
        .iter()
        .copied()
        .filter_map(|amount| {
            amount
                .msats
                .checked_add(output_fee(amount).msats)
                .filter(|cost| *cost != 0)
                .map(|cost| (amount, cost))
        })
        .collect::<Vec<_>>();
    coins.sort_unstable_by(|(a, ac), (b, bc)| bc.cmp(ac).then_with(|| b.cmp(a)));
    coins.dedup_by_key(|(_, cost)| *cost);
    if coins.is_empty() {
        return Err(LockedPaymentError::UnrepresentableRefund);
    }

    let mut selected = Vec::new();
    let mut remainder = target_msats;
    for &(amount, cost) in &coins {
        let count = remainder / cost;
        let remaining_capacity = MAX_LOCKED_PAYMENT_NOTES - selected.len();
        if count > remaining_capacity as u64 {
            return Err(LockedPaymentError::UnrepresentableRefund);
        }
        remainder %= cost;
        selected.extend(std::iter::repeat_n((amount, cost), count as usize));
    }

    loop {
        if remainder <= REPAIR_LIMIT_MSATS
            && let Some(mut repair) = minimum_coin_representation(
                &coins,
                remainder as usize,
                MAX_LOCKED_PAYMENT_NOTES - selected.len(),
            )
        {
            selected.append(&mut repair);
            let mut result = selected
                .into_iter()
                .map(|(amount, _)| amount)
                .collect::<Vec<_>>();
            result.sort_unstable_by(|a, b| b.cmp(a));
            return Ok(result);
        }
        let Some((_, cost)) = selected.pop() else {
            return Err(LockedPaymentError::UnrepresentableRefund);
        };
        remainder = remainder
            .checked_add(cost)
            .ok_or(LockedPaymentError::UnrepresentableRefund)?;
        if remainder > REPAIR_LIMIT_MSATS {
            return Err(LockedPaymentError::UnrepresentableRefund);
        }
    }
}

fn minimum_coin_representation(
    coins: &[(Amount, u64)],
    target: usize,
    max_coins: usize,
) -> Option<Vec<(Amount, u64)>> {
    let mut counts = vec![usize::MAX; target + 1];
    let mut previous = vec![None; target + 1];
    counts[0] = 0;
    for value in 1..=target {
        for (coin_index, &(_, cost)) in coins.iter().enumerate() {
            let Ok(cost) = usize::try_from(cost) else {
                continue;
            };
            if cost <= value && counts[value - cost] != usize::MAX {
                let candidate = counts[value - cost] + 1;
                if candidate <= max_coins && candidate < counts[value] {
                    counts[value] = candidate;
                    previous[value] = Some((value - cost, coin_index));
                }
            }
        }
    }
    if counts[target] == usize::MAX {
        return None;
    }
    let mut result = Vec::with_capacity(counts[target]);
    let mut value = target;
    while value != 0 {
        let (prior, coin_index) = previous[value]?;
        result.push(coins[coin_index]);
        value = prior;
    }
    Some(result)
}

pub fn decode_issuance_request(
    amount_msats: u64,
    blind_nonce: &[u8],
) -> Result<IssuanceRequest, LockedPaymentError> {
    let blind_nonce =
        BlindNonce::consensus_decode_whole(blind_nonce, &ModuleDecoderRegistry::default())
            .map_err(|_| LockedPaymentError::IssuanceMismatch)?;
    Ok(IssuanceRequest {
        amount: Amount::from_msats(amount_msats),
        blind_nonce,
    })
}

pub fn decode_blinded_signature(bytes: &[u8]) -> Result<BlindedSignature, LockedPaymentError> {
    BlindedSignature::consensus_decode_whole(bytes, &ModuleDecoderRegistry::default())
        .map_err(|_| LockedPaymentError::InvalidMintSignature)
}

/// Turn quoted foreign nonces into a transaction-builder output bundle.
///
/// The bundle intentionally has no client state machines: the FI relays and
/// aggregates outcomes itself after the transaction reaches consensus.
pub fn foreign_output_bundle(
    mint_module: ModuleInstanceId,
    issuance: &[IssuanceRequest],
) -> ClientOutputBundle {
    ClientOutputBundle::new_no_sm(
        issuance
            .iter()
            .map(|request| ClientOutput {
                output: MintOutput::new_v0(request.amount, request.blind_nonce),
                amounts: Amounts::new_bitcoin(request.amount),
            })
            .collect(),
    )
    .into_dyn(mint_module)
}

/// Aggregate the threshold shares collected by the FI for every foreign
/// output and verify each aggregate before it is sent to the FMan.
pub fn aggregate_payment_signatures(
    issuance: &[IssuanceRequest],
    shares: &[BTreeMap<u64, BlindedSignatureShare>],
    mint_keys: &BTreeMap<Amount, AggregatePublicKey>,
) -> Result<Vec<BlindedSignature>, LockedPaymentError> {
    if issuance.len() != shares.len() {
        return Err(LockedPaymentError::IssuanceCountMismatch);
    }
    issuance
        .iter()
        .zip(shares)
        .map(|(request, shares)| {
            if shares.is_empty() {
                return Err(LockedPaymentError::NoSignatureShares);
            }
            let key = mint_keys
                .get(&request.amount)
                .copied()
                .ok_or(LockedPaymentError::UnsupportedDenomination(request.amount))?;
            let signature = tbs::aggregate_signature_shares(shares);
            if !tbs::verify_blinded_signature(request.blind_nonce.0, signature, key) {
                return Err(LockedPaymentError::InvalidMintSignature);
            }
            Ok(signature)
        })
        .collect()
}

/// Derive mint-v1 issuance requests and their secrets without mutable state.
///
/// `quote_binding` must be the canonical encoding of every quote term known
/// before the issuance set is created. Repeating the call returns byte-for-byte
/// identical keys and requests.
pub fn derive_issuance_requests(
    wallet_root: &[u8; 64],
    quote_binding: &[u8],
    denominations: &[Amount],
) -> (Vec<IssuanceRequest>, Vec<NoteSecrets>) {
    let binding_hash = Sha256::digest(quote_binding);
    let hkdf = Hkdf::<Sha256>::new(None, wallet_root);
    let mut requests = Vec::with_capacity(denominations.len());
    let mut secrets = Vec::with_capacity(denominations.len());

    for (index, amount) in denominations.iter().copied().enumerate() {
        let mut info = Vec::with_capacity(QUOTE_NOTE_INFO.len() + 32 + 16);
        info.extend_from_slice(QUOTE_NOTE_INFO);
        info.extend_from_slice(&binding_hash);
        info.extend_from_slice(&(index as u64).to_be_bytes());
        info.extend_from_slice(&amount.msats.to_be_bytes());
        let mut material = [0u8; 96];
        hkdf.expand(&info, &mut material)
            .expect("96 bytes is a valid HKDF-SHA256 output length");

        let spend_key: [u8; 32] = material[..32].try_into().expect("fixed slice");
        // Invalid with probability ~2^-128 (and the zero scalar below with
        // ~2^-256); treated as unreachable, like the identity derivations
        // (ARCH-fleet-manager-identity).
        let spend =
            SecretKey::from_slice(&spend_key).expect("HKDF output is a valid secp256k1 scalar");
        let wide: [u8; 64] = material[32..].try_into().expect("fixed slice");
        let scalar = Scalar::from_bytes_wide(&wide);
        assert!(
            scalar != Scalar::zero(),
            "derived blinding scalar is nonzero"
        );
        let blinding_key = BlindingKey(scalar);
        let blinding_key_bytes = scalar.to_bytes();
        let nonce = Nonce(PublicKey::from_secret_key(SECP256K1, &spend));
        let blind_nonce = BlindNonce(tbs::blind_message(nonce.to_message(), blinding_key));

        requests.push(IssuanceRequest {
            amount,
            blind_nonce,
        });
        secrets.push(NoteSecrets {
            amount,
            spend_key,
            blinding_key: blinding_key_bytes,
        });
    }
    (requests, secrets)
}

/// Open and verify a complete mint-v1 payment presentation offline.
pub fn verify_payment(
    issuance: &[IssuanceRequest],
    secrets: &[NoteSecrets],
    signatures: &[BlindedSignature],
    mint_keys: &BTreeMap<Amount, AggregatePublicKey>,
) -> Result<Vec<SpendableLockedNote>, LockedPaymentError> {
    if issuance.len() != secrets.len() || issuance.len() != signatures.len() {
        return Err(LockedPaymentError::IssuanceCountMismatch);
    }

    let notes = issuance
        .iter()
        .zip(secrets)
        .zip(signatures)
        .map(|((request, secret), signature)| {
            if request.amount != secret.amount {
                return Err(LockedPaymentError::IssuanceMismatch);
            }
            let spend_key = SecretKey::from_slice(&secret.spend_key)
                .map_err(|_| LockedPaymentError::IssuanceMismatch)?;
            let nonce = Nonce(PublicKey::from_secret_key(SECP256K1, &spend_key));
            let scalar = Option::<Scalar>::from(Scalar::from_bytes(&secret.blinding_key))
                .filter(|scalar| *scalar != Scalar::zero())
                .ok_or(LockedPaymentError::IssuanceMismatch)?;
            let blinding_key = BlindingKey(scalar);
            if BlindNonce(tbs::blind_message(nonce.to_message(), blinding_key))
                != request.blind_nonce
            {
                return Err(LockedPaymentError::IssuanceMismatch);
            }
            let mint_key = mint_keys
                .get(&request.amount)
                .copied()
                .ok_or(LockedPaymentError::UnsupportedDenomination(request.amount))?;
            let note = Note {
                nonce,
                signature: tbs::unblind_signature(blinding_key, *signature),
            };
            if !note.verify(mint_key) {
                return Err(LockedPaymentError::InvalidMintSignature);
            }
            Ok(SpendableLockedNote {
                amount: request.amount,
                note,
                spend_key,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut nonces = BTreeSet::new();
    if notes.iter().any(|note| !nonces.insert(note.note.nonce)) {
        return Err(LockedPaymentError::DuplicateNoteNonce);
    }
    Ok(notes)
}

/// Construct and sign a refund transaction entirely offline.
///
/// The caller is responsible for choosing refund denominations whose total,
/// after the mint's input/output fees, balances the transaction.
pub fn build_refund_transaction(
    mint_module: ModuleInstanceId,
    paid_notes: &[SpendableLockedNote],
    refund_issuance: &[IssuanceRequest],
    nonce: [u8; 8],
) -> Transaction {
    let inputs = paid_notes
        .iter()
        .map(|locked| MintInput::new_v0(locked.amount, locked.note).into_dyn(mint_module))
        .collect::<Vec<_>>();
    let outputs = refund_issuance
        .iter()
        .map(|request| {
            MintOutput::new_v0(request.amount, request.blind_nonce).into_dyn(mint_module)
        })
        .collect::<Vec<_>>();
    let txid = Transaction::tx_hash_from_parts(&inputs, &outputs, nonce);
    let message = Message::from_digest(*txid.as_ref());
    let signatures = paid_notes
        .iter()
        .map(|locked| {
            SECP256K1.sign_schnorr_no_aux_rand(
                &message,
                &Keypair::from_secret_key(SECP256K1, &locked.spend_key),
            )
        })
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
    use super::*;
    use std::collections::BTreeMap;
    use tbs::{SecretKeyShare, aggregate_public_key_shares, aggregate_signature_shares};

    #[test]
    fn distinct_nonces_derive_disjoint_issuance_sets() {
        let root = [42; 64];
        let amounts = [Amount::from_msats(4), Amount::from_msats(1)];
        let nonces = |quote_nonce: &[u8]| {
            derive_issuance_requests(&root, quote_nonce, &amounts)
                .0
                .into_iter()
                .map(|request| request.blind_nonce)
                .collect::<Vec<_>>()
        };
        let (first, second) = (nonces(&[1; 32]), nonces(&[2; 32]));
        assert!(first.iter().all(|nonce| !second.contains(nonce)));
    }

    #[test]
    fn crypto_spine_derives_verifies_and_builds_signed_refund() {
        let root = [42; 64];
        let amounts = [Amount::from_sats(1), Amount::from_sats(2)];
        let (issuance, secrets) =
            derive_issuance_requests(&root, b"canonical quote terms", &amounts);
        assert_eq!(
            derive_issuance_requests(&root, b"canonical quote terms", &amounts).0,
            issuance
        );

        // One-of-one dealer keys exercise the same blind/unblind primitives
        // without requiring a running federation.
        let mut mint_keys = BTreeMap::new();
        let mut signatures = Vec::new();
        for (index, request) in issuance.iter().enumerate() {
            let sk = SecretKeyShare(Scalar::from(index as u64 + 7));
            let mut public_shares = BTreeMap::new();
            public_shares.insert(0, tbs::derive_pk_share(&sk));
            mint_keys.insert(request.amount, aggregate_public_key_shares(&public_shares));
            let mut signature_shares = BTreeMap::new();
            signature_shares.insert(0, tbs::sign_message(request.blind_nonce.0, sk));
            signatures.push(aggregate_signature_shares(&signature_shares));
        }

        let notes = verify_payment(&issuance, &secrets, &signatures, &mint_keys).unwrap();
        let (refund, _) =
            derive_issuance_requests(&[99; 64], b"fi refund", &[Amount::from_sats(2)]);
        let tx = build_refund_transaction(1, &notes, &refund, [3; 8]);
        tx.validate_signatures(
            &notes
                .iter()
                .map(|note| note.note.nonce.0)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }

    #[test]
    fn rejects_a_signature_for_another_nonce() {
        let amount = Amount::from_sats(1);
        let (issuance, secrets) = derive_issuance_requests(&[1; 64], b"quote", &[amount]);
        let (other, _) = derive_issuance_requests(&[2; 64], b"other", &[amount]);
        let sk = SecretKeyShare(Scalar::from(7u64));
        let mut public_shares = BTreeMap::new();
        public_shares.insert(0, tbs::derive_pk_share(&sk));
        let mut signature_shares = BTreeMap::new();
        signature_shares.insert(0, tbs::sign_message(other[0].blind_nonce.0, sk));
        let err = verify_payment(
            &issuance,
            &secrets,
            &[aggregate_signature_shares(&signature_shares)],
            &BTreeMap::from([(amount, aggregate_public_key_shares(&public_shares))]),
        )
        .unwrap_err();
        assert_eq!(err, LockedPaymentError::InvalidMintSignature);
    }

    #[test]
    fn rejects_a_duplicate_finalized_note_nonce() {
        let amount = Amount::from_sats(1);
        let (issuance, secrets) = derive_issuance_requests(&[1; 64], b"quote", &[amount]);
        let secret = SecretKeyShare(Scalar::from(7u64));
        let public_shares = BTreeMap::from([(0, tbs::derive_pk_share(&secret))]);
        let signature_shares =
            BTreeMap::from([(0, tbs::sign_message(issuance[0].blind_nonce.0, secret))]);
        let signature = aggregate_signature_shares(&signature_shares);

        let err = verify_payment(
            &[issuance[0], issuance[0]],
            &[secrets[0].clone(), secrets[0].clone()],
            &[signature, signature],
            &BTreeMap::from([(amount, aggregate_public_key_shares(&public_shares))]),
        )
        .unwrap_err();
        assert_eq!(err, LockedPaymentError::DuplicateNoteNonce);
    }

    #[test]
    fn refund_denominations_exactly_include_per_output_fees() {
        let tiers = [
            Amount::from_msats(1),
            Amount::from_msats(2),
            Amount::from_msats(4),
            Amount::from_msats(8),
        ];
        let denominations = refund_denominations(&tiers, |_| Amount::from_msats(100), 311).unwrap();
        assert_eq!(
            denominations
                .iter()
                .map(|amount| amount.msats + 100)
                .sum::<u64>(),
            311
        );
        assert!(denominations.iter().all(|amount| tiers.contains(amount)));
    }

    #[test]
    fn refund_denominations_reject_an_inexact_amount() {
        assert_eq!(
            refund_denominations(&[Amount::from_msats(2)], |_| Amount::from_msats(100), 101,),
            Err(LockedPaymentError::UnrepresentableRefund)
        );
    }

    #[test]
    fn refund_denominations_do_not_fill_large_refunds_with_smallest_notes() {
        let tiers = [
            Amount::from_msats(1),
            Amount::from_msats(2),
            Amount::from_msats(4),
            Amount::from_msats(8),
            Amount::from_msats(1 << 20),
        ];
        let large_cost = (1 << 20) + 100;
        let denominations =
            refund_denominations(&tiers, |_| Amount::from_msats(100), 10 * large_cost + 108)
                .unwrap();
        assert_eq!(denominations.len(), 11);
        assert_eq!(
            denominations
                .iter()
                .map(|amount| amount.msats + 100)
                .sum::<u64>(),
            10 * large_cost + 108
        );
    }

    #[test]
    fn refund_denominations_reject_attacker_scale_expansion_before_allocating() {
        assert_eq!(
            refund_denominations(
                &[Amount::from_msats(10_000_000_000), Amount::from_msats(1)],
                |_| Amount::from_msats(100),
                9_999_999_900,
            ),
            Err(LockedPaymentError::UnrepresentableRefund)
        );
    }

    #[test]
    fn refund_denomination_repairs_respect_the_exact_note_budget() {
        let denominations = refund_denominations(
            &[Amount::from_msats(10), Amount::from_msats(6)],
            |_| Amount::ZERO,
            624,
        )
        .unwrap();
        assert_eq!(denominations.len(), MAX_LOCKED_PAYMENT_NOTES);
        assert_eq!(
            denominations.iter().map(|amount| amount.msats).sum::<u64>(),
            624
        );
        assert_eq!(
            refund_denominations(
                &[Amount::from_msats(10), Amount::from_msats(6)],
                |_| Amount::ZERO,
                646,
            ),
            Err(LockedPaymentError::UnrepresentableRefund)
        );
    }
}
