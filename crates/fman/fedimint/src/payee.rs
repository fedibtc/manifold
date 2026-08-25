//! The FMan (payee) side of key-locked payments: [`Wallet`]'s
//! implementation of the [`EcashWallet`] boundary.
//!
//! Quote derivation, offline verification, claiming, and refund-transaction
//! submission all live here; the fleet only ever sees the
//! [`fman_core::wallet`] trait. The FI's side of the same protocol is
//! `fi-cli`'s `payer` module.

use anyhow::Context as _;
use fedi_decentralized_service_fleet_manager::{
    FederationId as WireFederationId, InviteCode as WireInviteCode, LockedBlindedSignature,
    LockedIssuanceRequest, LockedIssuanceRequestV2, PaymentTerms, QuoteId, QuoteTerms,
    RefundIssuance, RefundTransaction,
};
use fedimint_client_module::module::ClientModule as _;
use fedimint_client_module::oplog::OperationLogEntry;
use fedimint_core::Amount;
use fedimint_core::TieredMulti;
use fedimint_core::base32::{self, FEDIMINT_PREFIX};
use fedimint_core::config::FederationId;
use fedimint_core::core::{ModuleInstanceId, OperationId};
use fedimint_core::encoding::{Decodable as _, Encodable as _};
use fedimint_core::module::Amounts;
use fedimint_mint_client::OOBNotes;
use fedimint_mint_client::{
    MintClientModule, ReissueExternalNotesError, ReissueExternalNotesState,
};
use fedimint_mint_common::{MintInput, MintOutput};
use fedimint_mintv2_client::{
    ECash as MintV2Ecash, FinalReceiveOperationState as MintV2ReceiveState,
    MintOperationMeta as MintV2OperationMeta, ReceiveECashError as MintV2ReceiveError,
};
use fedimint_mintv2_common::MintInput as MintV2Input;
use futures::StreamExt as _;
use std::future::Future;

use fman_core::guardian_fee::GuardianFeeVault;
use fman_core::wallet::{
    ClaimOutcome, EcashClaimEvidence, EcashWallet, LockedPaymentPrepareError, Msats,
    VerifiedLockedPayment,
};

use locked_payments::denominations::quote_denominations;
use locked_payments::locked_payment::{self, decode_blinded_signature, decode_issuance_request};
use locked_payments::{locked_payment_v2, mint_v2_module, standard_module_root_secret};

use crate::{Wallet, WalletError};

fn encode_claim_evidence(
    terms: &QuoteTerms,
    signatures: &[LockedBlindedSignature],
    module_id: ModuleInstanceId,
    federation_invite: WireInviteCode,
) -> Result<EcashClaimEvidence, LockedPaymentPrepareError> {
    let claim = match terms
        .payment
        .as_ref()
        .ok_or(LockedPaymentPrepareError::Invalid)?
    {
        PaymentTerms::MintV1 { issuance, .. } => EcashClaimEvidence::MintV1 {
            federation_invite,
            module_id,
            quote_nonce: terms.quote_nonce,
            issuance: issuance.clone(),
            signatures: signatures.to_vec(),
        },
        PaymentTerms::MintV2 { issuance, .. } => EcashClaimEvidence::MintV2 {
            federation_invite,
            module_id,
            issuance: issuance.clone(),
            signatures: signatures.to_vec(),
        },
    };
    Ok(claim)
}

fn validate_claim_evidence(claim: &EcashClaimEvidence) -> anyhow::Result<()> {
    match claim {
        EcashClaimEvidence::MintV1 {
            federation_invite,
            issuance,
            signatures,
            ..
        } => {
            claim_federation_id(federation_invite)?;
            anyhow::ensure!(
                issuance.len() == signatures.len(),
                "signature count mismatch"
            );
            for request in issuance {
                decode_issuance_request(request.amount_msats, &request.blind_nonce)?;
            }
            for signature in signatures {
                decode_blinded_signature(&signature.0)?;
            }
        }
        EcashClaimEvidence::MintV2 {
            federation_invite,
            issuance,
            signatures,
            ..
        } => {
            claim_federation_id(federation_invite)?;
            anyhow::ensure!(
                issuance.len() == signatures.len(),
                "signature count mismatch"
            );
            for request in issuance {
                decode_v2_request(request)?;
            }
            for signature in signatures {
                locked_payment_v2::decode_blinded_signature(&signature.0)?;
            }
        }
    }
    Ok(())
}

/// The claim's federation identity is whatever its retained invite names;
/// the invite is built from the verified client at acceptance, so evidence
/// carries no separate federation id to disagree with it.
fn claim_federation_id(federation_invite: &WireInviteCode) -> anyhow::Result<FederationId> {
    let invite: fedimint_core::invite_code::InviteCode = federation_invite.0.parse()?;
    Ok(invite.federation_id())
}

/// Fully verified mint-v1 payment plus validated refund outputs, held only
/// as long as it takes the boundary to build the claw-back: the fleet above
/// receives that transaction, not these ingredients.
pub(crate) struct VerifiedLockedV1Payment {
    mint_module: ModuleInstanceId,
    notes: Vec<locked_payment::SpendableLockedNote>,
    refund_issuance: Vec<locked_payment::IssuanceRequest>,
    nonce: [u8; 8],
}

impl VerifiedLockedV1Payment {
    /// Consume the verified ingredients into the claw-back. Called once, by
    /// the boundary, while the note secrets are still in hand — the fleet
    /// then holds a transaction it can always emit.
    fn into_refund_transaction(self) -> RefundTransaction {
        RefundTransaction(
            locked_payment::build_refund_transaction(
                self.mint_module,
                &self.notes,
                &self.refund_issuance,
                self.nonce,
            )
            .consensus_encode_to_vec(),
        )
    }
}

/// Offline-verified mint-v2 payment retained until allocation chooses
/// acceptance or refusal.
pub(crate) struct VerifiedLockedV2Payment {
    mint_module: ModuleInstanceId,
    notes: Vec<fedimint_mintv2_client::SpendableNote>,
    refund_issuance: Vec<locked_payment_v2::IssuanceRequest>,
    nonce: [u8; 8],
}

impl VerifiedLockedV2Payment {
    fn into_refund_transaction(self) -> RefundTransaction {
        RefundTransaction(
            locked_payment_v2::build_refund_transaction(
                self.mint_module,
                &self.notes,
                &self.refund_issuance,
                self.nonce,
            )
            .consensus_encode_to_vec(),
        )
    }
}

fn decode_v2_request(
    request: &LockedIssuanceRequestV2,
) -> Result<locked_payment_v2::IssuanceRequest, LockedPaymentPrepareError> {
    Ok(locked_payment_v2::IssuanceRequest {
        denomination: locked_payment_v2::denomination_from_amount(request.amount_msats)
            .map_err(|_| LockedPaymentPrepareError::Invalid)?,
        blind_nonce: locked_payment_v2::decode_blinded_message(&request.blind_nonce)
            .map_err(|_| LockedPaymentPrepareError::Invalid)?,
        tweak: request.tweak,
    })
}

fn internal(err: impl Into<anyhow::Error>) -> LockedPaymentPrepareError {
    LockedPaymentPrepareError::Internal(err.into())
}

fn unrepresentable() -> LockedPaymentPrepareError {
    LockedPaymentPrepareError::Internal(anyhow::anyhow!(
        "price is not representable by the mint denomination tiers"
    ))
}

#[derive(Debug, thiserror::Error)]
#[error("payment signature count does not match the signed issuance set")]
struct PaymentSignatureCountMismatch;

/// Reject evidence whose number of signatures differs from the signed quote's
/// issuance set, without examining any signature bytes.
fn check_payment_signature_count(
    payment: &PaymentTerms,
    payment_signatures: &[LockedBlindedSignature],
) -> Result<(), PaymentSignatureCountMismatch> {
    let issuance_count = match payment {
        PaymentTerms::MintV1 { issuance, .. } => issuance.len(),
        PaymentTerms::MintV2 { issuance, .. } => issuance.len(),
    };
    if payment_signatures.len() == issuance_count {
        Ok(())
    } else {
        Err(PaymentSignatureCountMismatch)
    }
}

/// Decode remote signature evidence only after its signed count has been
/// checked. The generation-specific decoder remains a caller concern.
fn decode_remote_payment_signatures<T>(
    payment: &PaymentTerms,
    payment_signatures: &[LockedBlindedSignature],
    mut decode: impl FnMut(&LockedBlindedSignature) -> Result<T, LockedPaymentPrepareError>,
) -> Result<Vec<T>, LockedPaymentPrepareError> {
    check_payment_signature_count(payment, payment_signatures)
        .map_err(|_| LockedPaymentPrepareError::Invalid)?;
    payment_signatures.iter().map(&mut decode).collect()
}

fn resolve_mint_v2_receive(
    operation_id: OperationId,
    encoded_ecash: &str,
    result: Result<OperationId, MintV2ReceiveError>,
    existing: Option<&OperationLogEntry>,
) -> Result<OperationId, MintV2ReceiveError> {
    match result {
        Ok(operation_id) => Ok(operation_id),
        Err(MintV2ReceiveError::AlreadyReceived)
            if existing.is_some_and(|operation| {
                operation.operation_module_kind() == fedimint_mintv2_common::KIND.as_str()
                    && matches!(
                        operation.try_meta::<MintV2OperationMeta>(),
                        Ok(MintV2OperationMeta::Receive {
                            change_outpoint_range: _,
                            ecash,
                            custom_meta: _,
                        })
                            if ecash == encoded_ecash
                    )
            }) =>
        {
            Ok(operation_id)
        }
        Err(err) => Err(err),
    }
}

/// Start or recover one exact mint-v2 receive operation.
///
/// `AlreadyReceived` is accepted only when the deterministic operation-log
/// entry proves that it belongs to the byte-identical ecash.
pub(super) async fn handoff_mint_v2_receive<Receive, ReceiveFuture, Lookup, LookupFuture>(
    ecash: &MintV2Ecash,
    receive: Receive,
    lookup: Lookup,
) -> Result<OperationId, MintV2ReceiveError>
where
    Receive: FnOnce() -> ReceiveFuture,
    ReceiveFuture: Future<Output = Result<OperationId, MintV2ReceiveError>>,
    Lookup: FnOnce(OperationId) -> LookupFuture,
    LookupFuture: Future<Output = Option<OperationLogEntry>>,
{
    let operation_id = OperationId::from_encodable(ecash);
    let encoded_ecash = base32::encode_prefixed(FEDIMINT_PREFIX, ecash);
    let result = receive().await;
    let existing = if result == Err(MintV2ReceiveError::AlreadyReceived) {
        lookup(operation_id).await
    } else {
        None
    };
    resolve_mint_v2_receive(operation_id, &encoded_ecash, result, existing.as_ref())
}

impl Wallet {
    /// Verify a mint-v1 key-locked presentation and construct its
    /// deterministic, fee-balanced refund transaction entirely offline.
    pub(crate) async fn verify_locked_v1(
        &self,
        federation_id: FederationId,
        issuance: &[locked_payment::IssuanceRequest],
        secrets: &[locked_payment::NoteSecrets],
        signatures: &[tbs::BlindedSignature],
        refund_issuance: &[locked_payment::IssuanceRequest],
        nonce: [u8; 8],
    ) -> Result<VerifiedLockedV1Payment, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let mint_keys = mint
            .context()
            .tbs_pks
            .iter()
            .map(|(amount, key)| (amount, *key))
            .collect();
        let notes = locked_payment::verify_payment(issuance, secrets, signatures, &mint_keys)?;

        let input_amount = notes
            .iter()
            .try_fold(0u64, |total, note| total.checked_add(note.amount.msats));
        let output_amount = refund_issuance
            .iter()
            .try_fold(0u64, |total, output| total.checked_add(output.amount.msats));
        let input_fees = notes.iter().try_fold(0u64, |total, note| {
            let input = MintInput::new_v0(note.amount, note.note);
            let fee = mint
                .input_fee(&Amounts::new_bitcoin(note.amount), &input)
                .expect("mint-v1 supplies bitcoin input fees")
                .get_bitcoin()
                .msats;
            total.checked_add(fee)
        });
        let output_fees = refund_issuance.iter().try_fold(0u64, |total, output| {
            let mint_output = MintOutput::new_v0(output.amount, output.blind_nonce);
            let fee = mint
                .output_fee(&Amounts::new_bitcoin(output.amount), &mint_output)
                .expect("mint-v1 supplies bitcoin output fees")
                .get_bitcoin()
                .msats;
            total.checked_add(fee)
        });
        let balances = input_amount
            .zip(output_amount)
            .zip(input_fees)
            .zip(output_fees);
        let Some((((input_amount, output_amount), input_fees), output_fees)) = balances else {
            return Err(locked_payment::LockedPaymentError::InvalidRefundAmount.into());
        };
        let required = output_amount
            .checked_add(input_fees)
            .and_then(|amount| amount.checked_add(output_fees));
        if required != Some(input_amount) {
            return Err(locked_payment::LockedPaymentError::InvalidRefundAmount.into());
        }

        Ok(VerifiedLockedV1Payment {
            mint_module: mint.id,
            notes,
            refund_issuance: refund_issuance.to_vec(),
            nonce,
        })
    }

    /// Verify a mint-v2 key-locked presentation and its refund balance
    /// entirely offline.
    pub(crate) async fn verify_locked_v2(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        issuance: &[locked_payment_v2::IssuanceRequest],
        signatures: &[tbs::BlindedSignature],
        refund_issuance: &[locked_payment_v2::IssuanceRequest],
        nonce: [u8; 8],
    ) -> Result<VerifiedLockedV2Payment, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        let root = standard_module_root_secret(&self.root_secret, federation_id, mint_module);
        let private = issuance
            .iter()
            .copied()
            .map(|request| locked_payment_v2::issuance_request(&root, request))
            .collect::<Result<Vec<_>, _>>()?;
        let notes = mint
            .finalize_external_issuance(&private, signatures)
            .map_err(|_| locked_payment_v2::LockedPaymentV2Error::InvalidMintSignature)?;
        locked_payment_v2::ensure_distinct_note_nonces(&notes)?;

        let input_amount = notes
            .iter()
            .try_fold(0u64, |sum, note| sum.checked_add(note.amount().msats));
        let output_amount = refund_issuance.iter().try_fold(0u64, |sum, output| {
            sum.checked_add(output.denomination.amount().msats)
        });
        let input_fees = notes.iter().try_fold(0u64, |sum, note| {
            let input = MintV2Input::new_v0(fedimint_mintv2_common::Note {
                denomination: note.denomination,
                nonce: note.keypair.public_key(),
                signature: note.signature,
            });
            let fee = mint
                .input_fee(&Amounts::new_bitcoin(note.amount()), &input)
                .expect("bitcoin mint-v2 supplies input fees")
                .get_bitcoin()
                .msats;
            sum.checked_add(fee)
        });
        let output_fees = refund_issuance.iter().try_fold(0u64, |sum, output| {
            let mint_output = fedimint_mintv2_common::MintOutput::new_v0(
                output.denomination,
                output.blind_nonce,
                output.tweak,
            );
            let fee = mint
                .output_fee(
                    &Amounts::new_bitcoin(output.denomination.amount()),
                    &mint_output,
                )
                .expect("bitcoin mint-v2 supplies output fees")
                .get_bitcoin()
                .msats;
            sum.checked_add(fee)
        });
        let Some((((input_amount, output_amount), input_fees), output_fees)) = input_amount
            .zip(output_amount)
            .zip(input_fees)
            .zip(output_fees)
        else {
            return Err(locked_payment_v2::LockedPaymentV2Error::InvalidRefundAmount.into());
        };
        if output_amount
            .checked_add(input_fees)
            .and_then(|amount| amount.checked_add(output_fees))
            != Some(input_amount)
        {
            return Err(locked_payment_v2::LockedPaymentV2Error::InvalidRefundAmount.into());
        }
        Ok(VerifiedLockedV2Payment {
            mint_module,
            notes,
            refund_issuance: refund_issuance.to_vec(),
            nonce,
        })
    }

    /// Hand accepted mint-v1 locked notes to the ordinary
    /// external-note reissue path. Admission must be durably accepted
    /// before invoking this own-spend operation.
    pub(crate) async fn handoff_locked_v1(
        &self,
        federation_id: FederationId,
        notes: &[locked_payment::SpendableLockedNote],
    ) -> Result<OperationId, WalletError> {
        let notes = notes
            .iter()
            .map(|note| (note.amount, note.client_spendable_note()))
            .collect::<TieredMulti<_>>();
        let token = OOBNotes::new(federation_id.to_prefix(), notes);
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let expected = crate::reissue_operation_id(&token);
        if client.operation_exists(expected).await {
            return Ok(expected);
        }
        match mint.reissue_external_notes(token.clone(), ()).await {
            Ok(operation_id) => {
                if operation_id != expected {
                    return Err(WalletError::Client(anyhow::anyhow!(
                        "mint-v1 reissue operation id changed"
                    )));
                }
                Ok(operation_id)
            }
            Err(err)
                if matches!(
                    err.downcast_ref::<ReissueExternalNotesError>(),
                    Some(ReissueExternalNotesError::AlreadyReissued)
                ) && client.operation_exists(expected).await =>
            {
                Ok(expected)
            }
            Err(err) => Err(WalletError::ReceiveFailed(format!("{err:#}"))),
        }
    }

    /// Hand accepted mint-v2 locked notes to their durable receive state
    /// machine.
    pub(crate) async fn handoff_locked_v2(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        notes: Vec<fedimint_mintv2_client::SpendableNote>,
    ) -> Result<OperationId, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        let operation_log_client = client.clone();
        let ecash = MintV2Ecash::new(federation_id, notes);
        let expected = OperationId::from_encodable(&ecash);
        if let Some(existing) = operation_log_client
            .operation_log()
            .get_operation(expected)
            .await
        {
            let encoded_ecash = base32::encode_prefixed(FEDIMINT_PREFIX, &ecash);
            return resolve_mint_v2_receive(
                expected,
                &encoded_ecash,
                Err(MintV2ReceiveError::AlreadyReceived),
                Some(&existing),
            )
            .context("recover mint-v2 locked-payment claim")
            .map_err(Into::into);
        }
        handoff_mint_v2_receive(
            &ecash,
            || mint.receive(ecash.clone(), serde_json::Value::Null),
            |operation_id| async move {
                operation_log_client
                    .operation_log()
                    .get_operation(operation_id)
                    .await
            },
        )
        .await
        .context("start mint-v2 locked-payment claim")
        .map_err(Into::into)
    }

    /// Submit a fully signed transaction without claiming any outputs.
    /// Repeated submission of identical bytes is harmless.
    pub(crate) async fn submit_transaction(
        &self,
        federation_id: FederationId,
        raw_transaction: &[u8],
    ) -> Result<(), WalletError> {
        let client = self.client(federation_id).await?;
        let transaction = fedimint_core::transaction::Transaction::consensus_decode_whole(
            raw_transaction,
            client.decoders(),
        )
        .context("decode signed transaction")?;
        let txid = transaction.tx_hash();
        let outcome = client.api().submit_transaction(transaction).await;
        let fedimint_core::transaction::TransactionSubmissionOutcome(submission) = outcome
            .try_into_inner(client.decoders())
            .context("decode transaction submission outcome")?;
        submission.map_err(|error| anyhow::anyhow!("transaction rejected: {error}"))?;
        client.api().await_transaction(txid).await;
        Ok(())
    }
}

impl Wallet {
    async fn quote_locked_v1(
        &self,
        wire_federation_id: &WireFederationId,
        federation_id: FederationId,
        price: Msats,
        quote_nonce: &[u8; 32],
    ) -> Result<PaymentTerms, LockedPaymentPrepareError> {
        let tiers = self
            .mint_denominations(federation_id)
            .await
            .map_err(internal)?;
        let denominations = quote_denominations(price.0, &tiers).ok_or_else(unrepresentable)?;
        let (issuance, _secrets) = self.derive_locked_v1_quote(quote_nonce, &denominations);
        Ok(PaymentTerms::MintV1 {
            federation_id: wire_federation_id.clone(),
            issuance: issuance
                .into_iter()
                .map(|request| LockedIssuanceRequest {
                    amount_msats: request.amount.msats,
                    blind_nonce: request.blind_nonce.consensus_encode_to_vec(),
                })
                .collect(),
        })
    }

    async fn quote_locked_v2(
        &self,
        wire_federation_id: &WireFederationId,
        federation_id: FederationId,
        price: Msats,
    ) -> Result<PaymentTerms, LockedPaymentPrepareError> {
        let mint_module = self
            .first_mint_v2_module_id(federation_id)
            .await
            .map_err(internal)?;
        let tiers = self
            .mint_v2_denominations(federation_id, mint_module)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|denomination| denomination.amount())
            .collect::<Vec<_>>();
        let denominations = quote_denominations(price.0, &tiers).ok_or_else(unrepresentable)?;
        let denominations = denominations
            .into_iter()
            .map(|amount| locked_payment_v2::denomination_from_amount(amount.msats))
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        let tweaks = (0..denominations.len())
            .map(|_| rand::random::<[u8; 16]>())
            .collect::<Vec<_>>();
        let issuance = self
            .derive_locked_v2_quote(federation_id, mint_module, &denominations, &tweaks)
            .await
            .map_err(internal)?;
        Ok(PaymentTerms::MintV2 {
            federation_id: wire_federation_id.clone(),
            issuance: issuance
                .into_iter()
                .map(|request| LockedIssuanceRequestV2 {
                    amount_msats: request.denomination.amount().msats,
                    blind_nonce: request.blind_nonce.consensus_encode_to_vec(),
                    tweak: request.tweak,
                })
                .collect(),
        })
    }
}

impl Wallet {
    async fn claim_federation_invite(
        &self,
        federation_id: FederationId,
    ) -> Result<WireInviteCode, WalletError> {
        let client = self.client(federation_id).await?;
        let peer_urls = client.get_peer_urls().await;
        Ok(WireInviteCode(
            fedimint_core::invite_code::InviteCode::new_with_essential_num_guardians(
                &peer_urls,
                federation_id,
            )
            .to_string(),
        ))
    }

    async fn ensure_claim_client(&self, claim: &EcashClaimEvidence) -> anyhow::Result<()> {
        let federation_invite = match claim {
            EcashClaimEvidence::MintV1 {
                federation_invite, ..
            }
            | EcashClaimEvidence::MintV2 {
                federation_invite, ..
            } => federation_invite,
        };
        let invite: fedimint_core::invite_code::InviteCode = federation_invite.0.parse()?;
        let joined = Wallet::join(self, &invite).await?;
        anyhow::ensure!(
            joined == invite.federation_id(),
            "joined unexpected claim federation"
        );
        Ok(())
    }

    async fn handoff_stored(&self, claim: &EcashClaimEvidence) -> anyhow::Result<OperationId> {
        let operation_id = match claim {
            EcashClaimEvidence::MintV1 {
                federation_invite,
                module_id,
                quote_nonce,
                issuance,
                signatures: payment_signatures,
                ..
            } => {
                let federation_id = claim_federation_id(federation_invite)?;
                let denominations = issuance
                    .iter()
                    .map(|request| Amount::from_msats(request.amount_msats))
                    .collect::<Vec<_>>();
                let (_, secrets) = self.derive_locked_v1_quote(quote_nonce, &denominations);
                let issuance = issuance
                    .iter()
                    .map(|request| {
                        decode_issuance_request(request.amount_msats, &request.blind_nonce)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let signatures = payment_signatures
                    .iter()
                    .map(|signature| decode_blinded_signature(&signature.0))
                    .collect::<Result<Vec<_>, _>>()?;
                let client = self.client(federation_id).await?;
                let mint = client
                    .get_first_module::<MintClientModule>()
                    .context("mint module")?;
                anyhow::ensure!(mint.id == *module_id, "mint module changed");
                let keys = mint
                    .context()
                    .tbs_pks
                    .iter()
                    .map(|(amount, key)| (amount, *key))
                    .collect();
                let notes =
                    locked_payment::verify_payment(&issuance, &secrets, &signatures, &keys)?;
                self.handoff_locked_v1(federation_id, &notes).await?
            }
            EcashClaimEvidence::MintV2 {
                federation_invite,
                module_id: mint_module,
                issuance,
                signatures: payment_signatures,
                ..
            } => {
                let federation_id = claim_federation_id(federation_invite)?;
                let client = self.client(federation_id).await?;
                let mint = mint_v2_module(&client, *mint_module)?;
                let root =
                    standard_module_root_secret(&self.root_secret, federation_id, *mint_module);
                let issuance = issuance
                    .iter()
                    .map(decode_v2_request)
                    .collect::<Result<Vec<_>, _>>()?;
                let private = issuance
                    .iter()
                    .copied()
                    .map(|request| locked_payment_v2::issuance_request(&root, request))
                    .collect::<Result<Vec<_>, _>>()?;
                let signatures = payment_signatures
                    .iter()
                    .map(|signature| locked_payment_v2::decode_blinded_signature(&signature.0))
                    .collect::<Result<Vec<_>, _>>()?;
                let notes = mint.finalize_external_issuance(&private, &signatures)?;
                locked_payment_v2::ensure_distinct_note_nonces(&notes)?;
                self.handoff_locked_v2(federation_id, *mint_module, notes)
                    .await?
            }
        };
        Ok(operation_id)
    }

    async fn await_claim(
        &self,
        claim: &EcashClaimEvidence,
        operation_id: OperationId,
    ) -> anyhow::Result<ClaimOutcome> {
        let outcome = match claim {
            EcashClaimEvidence::MintV1 {
                federation_invite,
                module_id,
                ..
            } => {
                let federation_id = claim_federation_id(federation_invite)?;
                let client = self.client(federation_id).await?;
                let mint = client
                    .get_first_module::<MintClientModule>()
                    .context("mint module")?;
                anyhow::ensure!(mint.id == *module_id, "mint module changed");
                let mut updates = mint
                    .subscribe_reissue_external_notes(operation_id)
                    .await
                    .context("subscribe to locked-payment claim")?
                    .into_stream();
                loop {
                    match updates.next().await {
                        Some(ReissueExternalNotesState::Done) => break ClaimOutcome::Success,
                        Some(ReissueExternalNotesState::Failed(_)) => {
                            break ClaimOutcome::AlreadySpent;
                        }
                        Some(
                            ReissueExternalNotesState::Created | ReissueExternalNotesState::Issuing,
                        ) => {}
                        None => anyhow::bail!("claim update stream ended before completion"),
                    }
                }
            }
            EcashClaimEvidence::MintV2 {
                federation_invite,
                module_id: mint_module,
                ..
            } => {
                let federation_id = claim_federation_id(federation_invite)?;
                let client = self.client(federation_id).await?;
                let mint = mint_v2_module(&client, *mint_module)?;
                match mint
                    .await_final_receive_operation_state(operation_id)
                    .await?
                {
                    MintV2ReceiveState::Success => ClaimOutcome::Success,
                    MintV2ReceiveState::Rejected => ClaimOutcome::AlreadySpent,
                }
            }
        };
        Ok(outcome)
    }

    /// Validate evidence and finish any required client recovery before the
    /// bounded claim-attempt phase starts.
    pub(crate) async fn prepare_claim(&self, evidence: &EcashClaimEvidence) -> anyhow::Result<()> {
        validate_claim_evidence(evidence)?;
        self.ensure_claim_client(evidence).await
    }

    /// Hand prepared evidence to the wallet and wait for its terminal outcome.
    pub(crate) async fn reconcile_prepared_claim(
        &self,
        evidence: &EcashClaimEvidence,
    ) -> anyhow::Result<ClaimOutcome> {
        let operation_id = self.handoff_stored(evidence).await?;
        self.await_claim(evidence, operation_id).await
    }
}

#[async_trait::async_trait]
impl EcashWallet for Wallet {
    fn start_claim_worker(
        self: std::sync::Arc<Self>,
        db: fman_core::db::Db,
    ) -> std::sync::Arc<dyn fman_core::wallet::EcashClaimWorker> {
        crate::claim_worker::ClaimWorker::start(self, db)
    }

    fn start_payout_worker(
        self: std::sync::Arc<Self>,
        db: fman_core::db::Db,
        guardian_key: std::sync::Arc<
            dyn Fn(
                    &fedi_decentralized_service_fleet_manager::SeatId,
                ) -> fman_core::guardian_fee::GuardianFeeAccountKey
                + Send
                + Sync,
        >,
    ) -> std::sync::Arc<dyn fman_core::wallet::EcashPayoutWorker> {
        crate::payout_worker::PayoutWorker::start(self, db, guardian_key)
    }

    async fn quote_locked(
        &self,
        federation_id: &WireFederationId,
        price: Msats,
        quote_nonce: &[u8; 32],
    ) -> Result<PaymentTerms, LockedPaymentPrepareError> {
        let parsed = federation_id
            .0
            .parse()
            .map_err(|_| LockedPaymentPrepareError::Invalid)?;
        if self.has_mint_v2_module(parsed).await.map_err(internal)? {
            self.quote_locked_v2(federation_id, parsed, price).await
        } else {
            self.quote_locked_v1(federation_id, parsed, price, quote_nonce)
                .await
        }
    }

    async fn validate_quote_refund(
        &self,
        payment: &PaymentTerms,
        refund: &RefundIssuance,
    ) -> Result<(), LockedPaymentPrepareError> {
        let federation_id = payment
            .federation_id()
            .0
            .parse()
            .map_err(|_| LockedPaymentPrepareError::Invalid)?;
        let nonce = match refund {
            RefundIssuance::MintV1 { refund_nonce, .. }
            | RefundIssuance::MintV2 { refund_nonce, .. } => *refund_nonce,
        };
        let expected = self
            .prepare_quote_refund(
                federation_id,
                payment
                    .total_msats()
                    .ok_or(LockedPaymentPrepareError::Invalid)?,
                nonce,
            )
            .await
            .map_err(internal)?;
        let matches = match (refund, expected) {
            (
                RefundIssuance::MintV1 { issuance, .. },
                RefundIssuance::MintV1 {
                    issuance: expected, ..
                },
            ) => {
                issuance
                    .iter()
                    .map(|r| r.amount_msats)
                    .eq(expected.iter().map(|r| r.amount_msats))
                    && issuance
                        .iter()
                        .all(|r| decode_issuance_request(r.amount_msats, &r.blind_nonce).is_ok())
            }
            (
                RefundIssuance::MintV2 { issuance, .. },
                RefundIssuance::MintV2 {
                    issuance: expected, ..
                },
            ) => {
                issuance
                    .iter()
                    .map(|r| r.amount_msats)
                    .eq(expected.iter().map(|r| r.amount_msats))
                    && issuance.iter().all(|r| decode_v2_request(r).is_ok())
            }
            _ => false,
        };
        if matches {
            Ok(())
        } else {
            Err(LockedPaymentPrepareError::Invalid)
        }
    }

    async fn verify_locked(
        &self,
        quote_id: &QuoteId,
        terms: &QuoteTerms,
        payment_signatures: &[LockedBlindedSignature],
    ) -> Result<VerifiedLockedPayment, LockedPaymentPrepareError> {
        let quote_nonce = &terms.quote_nonce;
        let mut refund_nonce = [0; 8];
        refund_nonce.copy_from_slice(&quote_id.0[..8]);
        let Some(payment) = &terms.payment else {
            return Err(LockedPaymentPrepareError::Invalid);
        };
        match (payment, &terms.request.refund_issuance) {
            (
                PaymentTerms::MintV1 {
                    federation_id,
                    issuance,
                },
                Some(RefundIssuance::MintV1 {
                    issuance: refund_issuance,
                    ..
                }),
            ) => {
                let federation_id = federation_id
                    .0
                    .parse()
                    .map_err(|_| LockedPaymentPrepareError::Invalid)?;
                // Re-derive the per-note secrets from the wallet root and
                // the quoted public randomness: the derivation reproduces
                // the quoted blind nonces only for terms this wallet
                // issued, which verification checks note by note.
                let denominations = issuance
                    .iter()
                    .map(|request| Amount::from_msats(request.amount_msats))
                    .collect::<Vec<_>>();
                let (_, secrets) = self.derive_locked_v1_quote(quote_nonce, &denominations);
                let issuance = issuance
                    .iter()
                    .map(|request| {
                        decode_issuance_request(request.amount_msats, &request.blind_nonce)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| LockedPaymentPrepareError::Invalid)?;
                let signatures =
                    decode_remote_payment_signatures(payment, payment_signatures, |signature| {
                        decode_blinded_signature(&signature.0)
                            .map_err(|_| LockedPaymentPrepareError::Invalid)
                    })?;
                let refund_issuance = refund_issuance
                    .iter()
                    .map(|request| {
                        decode_issuance_request(request.amount_msats, &request.blind_nonce)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| LockedPaymentPrepareError::Invalid)?;
                let verified = self
                    .verify_locked_v1(
                        federation_id,
                        &issuance,
                        &secrets,
                        &signatures,
                        &refund_issuance,
                        refund_nonce,
                    )
                    .await
                    .map_err(|err| match err {
                        WalletError::InvalidLockedPayment(_) => LockedPaymentPrepareError::Invalid,
                        other => internal(other),
                    })?;
                let module_id = verified.mint_module;
                let federation_invite = self
                    .claim_federation_invite(federation_id)
                    .await
                    .map_err(internal)?;
                Ok(VerifiedLockedPayment::new(
                    encode_claim_evidence(terms, payment_signatures, module_id, federation_invite)?,
                    move || verified.into_refund_transaction(),
                ))
            }
            (
                PaymentTerms::MintV2 {
                    federation_id,
                    issuance,
                },
                Some(RefundIssuance::MintV2 {
                    issuance: refund_issuance,
                    ..
                }),
            ) => {
                let federation_id = federation_id
                    .0
                    .parse()
                    .map_err(|_| LockedPaymentPrepareError::Invalid)?;
                let mint_module = self
                    .first_mint_v2_module_id(federation_id)
                    .await
                    .map_err(internal)?;
                let issuance = issuance
                    .iter()
                    .map(decode_v2_request)
                    .collect::<Result<Vec<_>, _>>()?;
                let refund_issuance = refund_issuance
                    .iter()
                    .map(decode_v2_request)
                    .collect::<Result<Vec<_>, _>>()?;
                let signatures =
                    decode_remote_payment_signatures(payment, payment_signatures, |signature| {
                        locked_payment_v2::decode_blinded_signature(&signature.0)
                            .map_err(|_| LockedPaymentPrepareError::Invalid)
                    })?;
                let verified = self
                    .verify_locked_v2(
                        federation_id,
                        mint_module,
                        &issuance,
                        &signatures,
                        &refund_issuance,
                        refund_nonce,
                    )
                    .await
                    .map_err(|err| match err {
                        WalletError::InvalidLockedPaymentV2(_) => {
                            LockedPaymentPrepareError::Invalid
                        }
                        other => internal(other),
                    })?;
                let module_id = verified.mint_module;
                let federation_invite = self
                    .claim_federation_invite(federation_id)
                    .await
                    .map_err(internal)?;
                Ok(VerifiedLockedPayment::new(
                    encode_claim_evidence(terms, payment_signatures, module_id, federation_invite)?,
                    move || verified.into_refund_transaction(),
                ))
            }
            _ => Err(LockedPaymentPrepareError::Invalid),
        }
    }

    async fn submit_refund_transaction(
        &self,
        federation_id: &WireFederationId,
        transaction: &RefundTransaction,
    ) -> anyhow::Result<()> {
        let federation_id = federation_id.0.parse()?;
        self.submit_transaction(federation_id, &transaction.0)
            .await?;
        Ok(())
    }

    async fn receivable(&self, federation_id: &WireFederationId) -> bool {
        // v1 health: the federation is joined and its client is open. A
        // deeper receive-readiness probe (guardian quorum reachable) is a
        // refinement that belongs behind this same method.
        let Ok(federation_id) = federation_id.0.parse() else {
            return false;
        };
        self.federation_ids().await.contains(&federation_id)
    }

    async fn joined_federation_ids(&self) -> Vec<WireFederationId> {
        self.federation_ids()
            .await
            .into_iter()
            .map(|federation_id| WireFederationId(federation_id.to_string()))
            .collect()
    }

    async fn retained_federation_ids(&self) -> Vec<WireFederationId> {
        self.retained_federation_ids()
            .await
            .into_iter()
            .map(|federation_id| WireFederationId(federation_id.to_string()))
            .collect()
    }

    async fn join(&self, invite_code: &str) -> anyhow::Result<WireFederationId> {
        let invite_code: fedimint_core::invite_code::InviteCode = invite_code
            .trim()
            .parse()
            .map_err(|err| anyhow::anyhow!("invalid invite code: {err:#}"))?;
        let federation_id = Wallet::join(self, &invite_code).await?;
        Ok(WireFederationId(federation_id.to_string()))
    }

    /// This wallet also holds a client in each guarded federation, so it
    /// is the fee vault too (`crate::guardian_fee`).
    fn guardian_fees(&self) -> Option<&dyn GuardianFeeVault> {
        Some(self)
    }
}

#[cfg(test)]
#[path = "payee/tests.rs"]
mod tests;
