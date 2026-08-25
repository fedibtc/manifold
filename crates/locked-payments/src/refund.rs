//! Refund preparation for key-locked payments, shared by both roles.
//!
//! A quote's refund commitment is a deterministic function of the wallet
//! secret, the federation's mint generation and fee schedules, the quoted
//! price, and the FI-chosen refund nonce. The payer (FI) runs
//! [`prepare_quote_refund`] to build the commitment it sends with a signed
//! quote request, and later re-runs [`prepare_refund_v1`] /
//! [`prepare_refund_v2`] to reconstruct the secrets when presenting or
//! refunding. The payee (FMan) runs the same preparation under its own
//! secrets as the validation oracle: the denomination amounts are
//! wallet-independent, so comparing them (and nonce decodability) against a
//! presented commitment proves the eventual refund transaction balances
//! exactly after fees. Only the amounts survive that comparison — the
//! payee's derived nonces and secrets are discarded.
//!
//! Every function takes the caller's own Fedimint client material; this
//! module holds no wallet state.

use anyhow::Context as _;
use bitcoin_hashes::{Hash as _, sha256};
use fedimint_client::ClientHandleArc;
use fedimint_client_module::module::ClientModule as _;
use fedimint_core::config::FederationId;
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::encoding::Encodable as _;
use fedimint_core::module::Amounts;
use fedimint_derive_secret::DerivableSecret;
use fedimint_mint_client::MintClientModule;
use fedimint_mint_common::{MintInput, MintOutput};
use fedimint_mintv2_client::MintClientModule as MintV2ClientModule;
use fedimint_mintv2_common::Denomination as MintV2Denomination;
use fedimint_mintv2_common::MintInput as MintV2Input;
use hkdf::Hkdf;
use sha2::Sha256;

use crate::denominations::{economical_mint_v2_denominations, quote_denominations};
use crate::{locked_payment, locked_payment_v2};
use fedi_decentralized_service_fleet_manager::{
    LockedIssuanceRequest, LockedIssuanceRequestV2, RefundIssuance,
};

const REFUND_V2_TWEAK_SALT: &[u8] = b"fman/v2/refund-tweak/v1";

/// Refund-preparation failures, typed by generation so each role can map
/// them onto its own wallet error surface.
#[derive(Debug, thiserror::Error)]
pub enum RefundError {
    #[error(transparent)]
    V1(#[from] locked_payment::LockedPaymentError),
    #[error(transparent)]
    V2(#[from] locked_payment_v2::LockedPaymentV2Error),
    #[error(transparent)]
    Client(#[from] anyhow::Error),
}

/// FI-owned secret material for one possible refund. It is intentionally not
/// serializable or printable. A consumer keeps it only across one
/// `CreateSeat` attempt; an interrupted consumer reconstructs it from the
/// wallet secret and exact quote binding.
pub struct PreparedRefund {
    issuance: Vec<locked_payment::IssuanceRequest>,
    secrets: Vec<locked_payment::NoteSecrets>,
}

/// FI-owned mint-v2 refund requests, reconstructed for one presentation and
/// retained until acceptance or refund.
pub struct PreparedRefundV2 {
    issuance: Vec<locked_payment_v2::IssuanceRequest>,
    private: Vec<fedimint_mintv2_client::issuance::NoteIssuanceRequest>,
}

impl PreparedRefundV2 {
    pub fn issuance(&self) -> &[locked_payment_v2::IssuanceRequest] {
        &self.issuance
    }

    /// The private issuance requests matching [`Self::issuance`]. Consumed
    /// only by the payer's refund-submission path; the payee discards them
    /// after deriving the issuance amounts.
    pub fn private(&self) -> &[fedimint_mintv2_client::issuance::NoteIssuanceRequest] {
        &self.private
    }
}

impl PreparedRefund {
    pub fn issuance(&self) -> &[locked_payment::IssuanceRequest] {
        &self.issuance
    }

    /// The note secrets matching [`Self::issuance`]. Consumed only by the
    /// payer's refund-submission path; the payee discards them after
    /// deriving the issuance amounts.
    pub fn secrets(&self) -> &[locked_payment::NoteSecrets] {
        &self.secrets
    }
}

/// Prepare the public refund commitment before requesting a quote.
///
/// `root_secret` is the caller's global client root (the one its Fedimint
/// clients open under) and `wallet_secret` its raw wallet secret bytes;
/// both roles pass their own.
pub async fn prepare_quote_refund(
    client: &ClientHandleArc,
    root_secret: &DerivableSecret,
    wallet_secret: &[u8; 64],
    federation_id: FederationId,
    price_msats: u64,
    refund_nonce: [u8; 32],
) -> Result<RefundIssuance, RefundError> {
    if let Ok(mint) = client.get_first_module::<MintV2ClientModule>() {
        let module = mint.id;
        let tiers = economical_mint_v2_denominations()
            .into_iter()
            .map(|d| d.amount())
            .collect::<Vec<_>>();
        let amounts = quote_denominations(price_msats, &tiers)
            .ok_or(locked_payment_v2::LockedPaymentV2Error::InvalidRefundAmount)?;
        let denominations = amounts
            .into_iter()
            .map(|a| locked_payment_v2::denomination_from_amount(a.msats))
            .collect::<Result<Vec<_>, _>>()?;
        let tweaks = (0..denominations.len())
            .map(|_| [0; 16])
            .collect::<Vec<_>>();
        let (paid, _) = locked_payment_v2::derive_standard_issuance_requests(
            root_secret,
            federation_id,
            module,
            &denominations,
            &tweaks,
        )?;
        let prepared = prepare_refund_v2(
            &mint,
            root_secret,
            wallet_secret,
            federation_id,
            module,
            &paid,
            refund_nonce,
        )?;
        Ok(RefundIssuance::MintV2 {
            refund_nonce,
            issuance: prepared
                .issuance()
                .iter()
                .map(|r| LockedIssuanceRequestV2 {
                    amount_msats: r.denomination.amount().msats,
                    blind_nonce: r.blind_nonce.consensus_encode_to_vec(),
                    tweak: r.tweak,
                })
                .collect(),
        })
    } else {
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let tiers = mint.context().tbs_pks.tiers().copied().collect::<Vec<_>>();
        let amounts = quote_denominations(price_msats, &tiers)
            .ok_or(locked_payment::LockedPaymentError::InvalidRefundAmount)?;
        let (paid, _) =
            locked_payment::derive_issuance_requests(&[1; 64], b"refund fee template", &amounts);
        let prepared = prepare_refund_v1(&mint, wallet_secret, &paid, refund_nonce)?;
        Ok(RefundIssuance::MintV1 {
            refund_nonce,
            issuance: prepared
                .issuance()
                .iter()
                .map(|r| LockedIssuanceRequest {
                    amount_msats: r.amount.msats,
                    blind_nonce: r.blind_nonce.consensus_encode_to_vec(),
                })
                .collect(),
        })
    }
}

/// Prepare FI-owned mint-v2 refund outputs. `refund_nonce` is the FI-chosen
/// binding carried in the signed quote request and must be identical on
/// every retry. The refund keys are reproduced from the wallet secrets plus
/// that refund nonce, so callers persist the public quote but no refund
/// secrets.
pub fn prepare_refund_v2(
    mint: &MintV2ClientModule,
    root_secret: &DerivableSecret,
    wallet_secret: &[u8; 64],
    federation_id: FederationId,
    mint_module: ModuleInstanceId,
    paid: &[locked_payment_v2::IssuanceRequest],
    quote_id: [u8; 32],
) -> Result<PreparedRefundV2, RefundError> {
    let paid_amount = paid.iter().try_fold(0u64, |sum, request| {
        sum.checked_add(request.denomination.amount().msats)
    });
    let dummy_key = fedimint_core::secp256k1::Keypair::from_secret_key(
        fedimint_core::secp256k1::SECP256K1,
        &fedimint_core::secp256k1::SecretKey::from_slice(&[1; 32]).expect("one is a valid scalar"),
    );
    let input_fees = paid.iter().try_fold(0u64, |sum, request| {
        let amount = request.denomination.amount();
        let input = MintV2Input::new_v0(fedimint_mintv2_common::Note {
            denomination: request.denomination,
            nonce: dummy_key.public_key(),
            signature: tbs::Signature(bls12_381::G1Affine::generator()),
        });
        let fee = mint
            .input_fee(&Amounts::new_bitcoin(amount), &input)
            .expect("bitcoin mint-v2 supplies input fees")
            .get_bitcoin()
            .msats;
        sum.checked_add(fee)
    });
    let target = paid_amount
        .zip(input_fees)
        .and_then(|(amount, fees)| amount.checked_sub(fees))
        .ok_or(locked_payment_v2::LockedPaymentV2Error::InvalidRefundAmount)?;
    let fee_nonce = paid
        .first()
        .ok_or(locked_payment_v2::LockedPaymentV2Error::InvalidRefundAmount)?;
    let amounts = economical_mint_v2_denominations()
        .into_iter()
        .map(MintV2Denomination::amount)
        .collect::<Vec<_>>();
    let denominations = locked_payment::refund_denominations(
        &amounts,
        |amount| {
            let denomination = locked_payment_v2::denomination_from_amount(amount.msats)
                .expect("client denomination is a power of two");
            mint.output_fee(
                &Amounts::new_bitcoin(amount),
                &fedimint_mintv2_common::MintOutput::new_v0(
                    denomination,
                    fee_nonce.blind_nonce,
                    fee_nonce.tweak,
                ),
            )
            .expect("bitcoin mint-v2 supplies output fees")
            .get_bitcoin()
        },
        target,
    )?
    .into_iter()
    .map(|amount| locked_payment_v2::denomination_from_amount(amount.msats))
    .collect::<Result<Vec<_>, _>>()?;
    let tweaks = derive_refund_v2_tweaks(
        wallet_secret,
        federation_id,
        mint_module,
        quote_id,
        denominations.len(),
    );
    let (issuance, private) = locked_payment_v2::derive_standard_issuance_requests(
        root_secret,
        federation_id,
        mint_module,
        &denominations,
        &tweaks,
    )?;
    Ok(PreparedRefundV2 { issuance, private })
}

/// Create FI-owned refund nonces that make the FMan's refund transaction
/// balance exactly after all configured mint input and output fees.
/// `refund_nonce` is the FI-chosen binding carried in the signed quote
/// request; the same wallet secret and binding reproduce the same refund
/// without persistence.
pub fn prepare_refund_v1(
    mint: &MintClientModule,
    wallet_secret: &[u8; 64],
    paid: &[locked_payment::IssuanceRequest],
    quote_id: [u8; 32],
) -> Result<PreparedRefund, RefundError> {
    let context = mint.context();
    let paid_amount = paid
        .iter()
        .try_fold(0u64, |sum, request| sum.checked_add(request.amount.msats));
    let fee_nonce = paid
        .first()
        .ok_or(locked_payment::LockedPaymentError::InvalidRefundAmount)?
        .blind_nonce;
    let output_fee_for = |amount| {
        mint.output_fee(
            &Amounts::new_bitcoin(amount),
            &MintOutput::new_v0(amount, fee_nonce),
        )
        .expect("mint-v1 supplies bitcoin output fees")
        .get_bitcoin()
    };
    // Fee methods ignore note contents, but using the actual input API
    // keeps this calculation correct if input and output fee schedules
    // diverge in a future compatible config.
    let dummy_note = fedimint_mint_common::Note {
        nonce: fedimint_mint_common::Nonce(fedimint_core::secp256k1::PublicKey::from_secret_key(
            fedimint_core::secp256k1::SECP256K1,
            &fedimint_core::secp256k1::SecretKey::from_slice(&[1; 32])
                .expect("one is a valid secret scalar"),
        )),
        signature: tbs::Signature(bls12_381::G1Affine::generator()),
    };
    let input_fees = paid.iter().try_fold(0u64, |sum, request| {
        let fee = mint
            .input_fee(
                &Amounts::new_bitcoin(request.amount),
                &MintInput::new_v0(request.amount, dummy_note),
            )
            .expect("mint-v1 supplies bitcoin input fees")
            .get_bitcoin();
        sum.checked_add(fee.msats)
    });
    let target = paid_amount
        .zip(input_fees)
        .and_then(|(amount, fees)| amount.checked_sub(fees))
        .ok_or(locked_payment::LockedPaymentError::InvalidRefundAmount)?;
    let denominations = locked_payment::refund_denominations(
        &context.tbs_pks.tiers().copied().collect::<Vec<_>>(),
        output_fee_for,
        target,
    )?;
    let (issuance, secrets) =
        locked_payment::derive_issuance_requests(wallet_secret, &quote_id, &denominations);
    Ok(PreparedRefund { issuance, secrets })
}

/// Derive public mint-v2 tweaks from secret wallet material and a fixed-size,
/// domain-separated FI-chosen refund context. The
/// tweaks reveal neither the wallet secret nor the quote binding, while an
/// exact retry reproduces the private issuance requests through mint-v2's
/// standard module-root derivation.
fn derive_refund_v2_tweaks(
    wallet_secret: &[u8; 64],
    federation_id: FederationId,
    mint_module: ModuleInstanceId,
    refund_nonce: [u8; 32],
    count: usize,
) -> Vec<[u8; 16]> {
    let hkdf = Hkdf::<Sha256>::new(Some(REFUND_V2_TWEAK_SALT), wallet_secret);
    let binding_hash = sha256::Hash::hash(&refund_nonce).to_byte_array();
    let mut context = federation_id.consensus_encode_to_vec();
    context.extend(mint_module.consensus_encode_to_vec());
    context.extend(binding_hash);

    (0..count)
        .map(|index| {
            let index = u64::try_from(index).expect("output index fits in u64");
            let mut info = context.clone();
            info.extend(index.to_be_bytes());
            let mut tweak = [0; 16];
            hkdf.expand(&info, &mut tweak)
                .expect("16-byte HKDF output is valid");
            tweak
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_v2_refund_tweaks_are_reproducible_and_nonce_bound() {
        let secret = [11; 64];
        let federation_id = FederationId::dummy();
        let mint_module = ModuleInstanceId::from(7u16);
        let quote_id = [21; 32];
        let tweaks = derive_refund_v2_tweaks(&secret, federation_id, mint_module, quote_id, 3);

        assert_eq!(
            tweaks,
            derive_refund_v2_tweaks(&secret, federation_id, mint_module, quote_id, 3,)
        );
        assert_eq!(tweaks.len(), 3);
        assert_ne!(tweaks[0], tweaks[1]);
        assert_ne!(
            tweaks,
            derive_refund_v2_tweaks(&secret, federation_id, mint_module, [20; 32], 3,)
        );
        assert_ne!(
            tweaks,
            derive_refund_v2_tweaks(
                &secret,
                federation_id,
                ModuleInstanceId::from(8u16),
                quote_id,
                3,
            )
        );
    }
}
