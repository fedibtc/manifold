//! Cryptographic primitives for mint-v2 key-locked payments.

use std::collections::BTreeSet;

use fedimint_client_module::transaction::{ClientOutput, ClientOutputBundle};
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
