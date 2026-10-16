//! The destination seat of a guardian transfer: its selection and purchase.
//!
//! A guardian transfer first buys one seat on a fresh MG, then moves the
//! guardian onto it. The purchase is the transfer's first stage and lives in
//! the transfer row, so the paid seat always belongs to one move.
//!
//! The purchase pays through the same [`FaPayments`] port as formation, as an
//! aggregate of one member under the transfer's reservation scope. Its
//! value-safety rules are formation's, for one seat:
//!
//! - A paid quote is persisted before its wallet reservation, so a crash can
//!   always find and release the reservation.
//! - Until the durable output boundary is armed, a stored paid quote is
//!   refreshed: its reservation is released and a fresh quote obtained, so a
//!   payment never starts against a stale offer.
//! - After the boundary, the exact quote is replayed and never refreshed,
//!   because funds may be locked to its blind nonces.
//! - A free quote is replayed exactly until the MG accepts or signs a
//!   refusal, so a lost acceptance cannot allocate a second seat.

use std::collections::{BTreeMap, BTreeSet};

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::{
    GetQuoteResponse, Locator, SeatId, SignatureVerified, SignedResponse, Timestamp,
};
use fedimint_core::runtime::Instant;
use nostr_sdk::PublicKey;
use stability_pool_common::Account;
use tokio::sync::OnceCell;

use crate::db::{FaRecovery, GuardianTransferWrite};
use crate::formation::{
    DriverRun, FreeSeatQuote, PaidSeatQuote, QuoteAttemptPolicy, SeatCreation, SeatPresentation,
    now_secs,
};
use crate::mg::MgClient;
use crate::selection::{
    SelectionBadgeVerifier, TransferDestinationApproval, TransferDestinationPreview, dkg_line,
};
use crate::transfer::{StoredGuardianTransfer, TransferStage};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader, FederationId,
    FederationName, FederationSize, FormationId, FormationPhase, ManifoldGuardianConnector,
    MgDiscoveryOptions, PaymentRequirements, PaymentReservationRecovery, PlanPreference,
    ResolvedFormationIntent, SeatPaymentRecovery, SeatPaymentRequirement,
};

/// The seat a transfer moves the guardian onto.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDestinationSeat {
    #[serde(rename = "fman_id")]
    pub(crate) mg_id: PublicKey,
    pub(crate) locator: Locator,
    pub(crate) seat_id: SeatId,
    /// The MG-signed account from the seat's acceptance. Absent for a
    /// destination restored from a backup, which carries no acceptance.
    pub(crate) guardian_fee_account: Option<Account>,
    /// Verified badge holder at purchase. Absent for a restored destination.
    pub(crate) holder: Option<PublicKey>,
}

/// The purchase of a transfer's destination seat.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSeatPurchase {
    #[serde(rename = "fman_id")]
    pub(crate) mg_id: PublicKey,
    pub(crate) holder: PublicKey,
    pub(crate) locator: Locator,
    /// What every quote must match: the federation's size and the Fedimint
    /// line it ran when the seat was selected.
    pub(crate) terms: ResolvedFormationIntent,
    /// Spending cap the user approved for this seat.
    pub(crate) max_msats: u64,
    /// Current quote. Replaced only while it is free of value effects.
    pub(crate) signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    /// Durable output boundary: once set, the quote is replayed exactly.
    pub(crate) outputs_started: bool,
    /// Explicit payer, absent for a free seat.
    pub(crate) payment_federation_id: Option<FederationId>,
}

/// The formed federation a transfer works on, from a formation this device
/// ran or from a restored backup.
pub(crate) struct TransferFederation {
    pub(crate) formation_id: FormationId,
    federation_name: FederationName,
    federation_size: FederationSize,
    /// MG identity, locator, and seat id of every guardian seat.
    pub(crate) seats: Vec<(PublicKey, Locator, SeatId)>,
    /// Known badge holders of the seats and of past hosts. Unknown after a
    /// restore.
    pub(crate) holders: BTreeSet<PublicKey>,
    /// MGs that hosted a peer before a transfer moved it away.
    pub(crate) past_mgs: BTreeSet<PublicKey>,
}

impl TransferFederation {
    /// Whether an MG or holder hosts, or hosted, a peer of the federation.
    /// Such a host could hold two key shares.
    pub(crate) fn has_custody(&self, mg_id: PublicKey, holder: Option<PublicKey>) -> bool {
        self.seats
            .iter()
            .any(|(seat_mg_id, _, _)| *seat_mg_id == mg_id)
            || self.past_mgs.contains(&mg_id)
            || holder.is_some_and(|holder| self.holders.contains(&holder))
    }
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Select one fresh MG to receive a guardian of the formed federation.
    ///
    /// The MG holds no seat of this federation, never held one, and its badge
    /// holder vouches for none of them.
    pub async fn preview_transfer_destination(
        &self,
        options: MgDiscoveryOptions,
    ) -> FaResult<TransferDestinationPreview> {
        self.preview_transfer_destination_with_verifier(options, &self.inner.peer_badge_verifier)
            .await
    }

    pub(crate) async fn preview_transfer_destination_with_verifier(
        &self,
        options: MgDiscoveryOptions,
        verifier: &impl SelectionBadgeVerifier,
    ) -> FaResult<TransferDestinationPreview> {
        let fa_id = self.fa_id()?;
        let federation = self.transfer_federation(fa_id).await?;
        let mut members = BTreeSet::new();
        let mut retained_service_pubkeys = BTreeMap::new();
        for (mg_id, locator, _) in &federation.seats {
            members.insert(*mg_id);
            retained_service_pubkeys.insert(locator.service_pubkey, *mg_id);
        }
        let mut excluded = members.clone();
        excluded.extend(federation.past_mgs);
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        crate::selection::preview_transfer_destination_with(
            &self.inner.ports.registry,
            verifier,
            &crate::selection::LiveAvailabilityProber {
                connector: Some(&self.inner.ports.mg_connector),
            },
            self.inner.peer_badge_verifier.provenance(),
            federation.federation_size,
            &members,
            excluded,
            retained_service_pubkeys,
            federation.holders,
            deadline,
            now,
            || fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }

    /// The purchase the approval allows, before any network call.
    pub(crate) fn seat_purchase(
        &self,
        approval: TransferDestinationApproval,
        federation: &TransferFederation,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<StoredSeatPurchase> {
        if approval.valid_until <= Timestamp(now_secs()?) {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: crate::SelectionReauthorizationReason::PreviewExpired,
                failure: None,
            });
        }
        if approval.verifier_provenance != self.inner.peer_badge_verifier.provenance() {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: crate::SelectionReauthorizationReason::VerifierEnvironmentChanged,
                failure: None,
            });
        }
        // A seat advertised with a price cannot be bought without a payer;
        // refuse before the transfer is saved rather than leave it open with
        // nothing to pay.
        if 0 < approval.advertised_price_msats && payment_federation_id.is_none() {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: crate::SelectionReauthorizationReason::PaymentFederationRequired,
                failure: None,
            });
        }
        Ok(StoredSeatPurchase {
            mg_id: approval.seat.mg_id,
            holder: approval.seat.holder,
            locator: approval.seat.locator,
            terms: ResolvedFormationIntent {
                federation_name: federation.federation_name.clone(),
                federation_size: federation.federation_size,
                plan: PlanPreference::default(),
                fedimintd_versions: dkg_line(&approval.fedimintd_dkg_version)?,
                fedimintd_dkg_version: approval.fedimintd_dkg_version,
                max_total_msats: Some(approval.max_msats),
            },
            max_msats: approval.max_msats,
            signed_quote: None,
            outputs_started: false,
            payment_federation_id,
        })
    }

    pub(crate) async fn transfer_federation(
        &self,
        fa_id: crate::FaId,
    ) -> FaResult<TransferFederation> {
        match self.inner.store.load_recovery(fa_id).await? {
            FaRecovery::Formation(recovery)
                if recovery.snapshot.phase == FormationPhase::Formed =>
            {
                let mut seats = Vec::with_capacity(recovery.seats.len());
                let mut holders = BTreeSet::new();
                for seat in recovery.seats {
                    holders.extend(seat.admission.holder());
                    let seat_id = seat
                        .progress
                        .seat_id
                        .ok_or_else(|| FaError::Storage("formed seat has no seat id".to_owned()))?;
                    seats.push((seat.admission.mg_id(), seat.progress.locator, seat_id));
                }
                Ok(TransferFederation {
                    formation_id: recovery.snapshot.formation_id,
                    federation_name: recovery.snapshot.intent.federation_name,
                    federation_size: recovery.snapshot.intent.federation_size,
                    seats,
                    holders,
                    past_mgs: BTreeSet::new(),
                })
            }
            FaRecovery::Restored(snapshot) => {
                let (holders, past_mgs) = self.inner.store.restored_custody().await;
                Ok(TransferFederation {
                    federation_size: FederationSize(u16::try_from(snapshot.seats.len()).map_err(
                        |_| FaError::Storage("restored federation has too many seats".to_owned()),
                    )?),
                    federation_name: snapshot.federation_name.unwrap_or_else(|| {
                        crate::formation::default_federation_name(
                            fa_id,
                            snapshot.snapshot_generation,
                        )
                    }),
                    formation_id: snapshot.formation_id,
                    seats: snapshot
                        .seats
                        .into_iter()
                        .map(|seat| (seat.mg_identity, seat.locator, seat.seat_id))
                        .collect(),
                    holders,
                    past_mgs,
                })
            }
            FaRecovery::Idle | FaRecovery::Formation(_) => Err(FaError::NoActiveFormation),
        }
    }

    /// Buy the transfer's destination seat. Every change to the purchase is
    /// saved in the transfer row. A purchase that ends without a seat and
    /// without value at risk closes the transfer.
    pub(crate) async fn drive_seat_purchase(
        &self,
        transfer: &StoredGuardianTransfer,
        mut purchase: StoredSeatPurchase,
        fa_id: crate::FaId,
        run: DriverRun<'_>,
    ) -> FaResult<StoredDestinationSeat> {
        let lease = run.lease();
        let intent = &purchase.terms.clone();
        let client =
            MgClient::connect(&self.inner.ports.mg_connector, &purchase.locator, run).await?;
        let payment_federation = OnceCell::new();

        // A stored paid quote whose outputs never started may be stale; give
        // back its reservation and quote again before any value moves.
        if !purchase.outputs_started
            && let Some(signed) = &purchase.signed_quote
        {
            let quote = self.verify_quote(
                0,
                signed,
                &purchase.locator,
                intent,
                fa_id,
                purchase.payment_federation_id.as_ref(),
            )?;
            if quote.terms.payment.is_some() {
                let requirements = purchase_requirements(&transfer.transfer_id, &purchase, &quote)?;
                let quotes = [quote];
                let preflight = crate::ExactPaymentPreflight::new(&requirements, &quotes)?;
                let reservation_id =
                    crate::db::payment_reservation_id(&transfer.transfer_id, &requirements);
                let recovered = run
                    .call("recovering destination seat reservation", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .recover_payment_reservation(&reservation_id, &preflight))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                if let PaymentReservationRecovery::Existing(reservation) = recovered {
                    run.call("releasing stale destination seat reservation", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .release_payment_reservation(reservation))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                }
                purchase.signed_quote = None;
                save_purchase(transfer, &purchase, fa_id, run).await?;
            }
        }

        if purchase.signed_quote.is_none() {
            let (signed, quote) = match self
                .request_new_quote(
                    0,
                    &client,
                    &purchase.locator,
                    intent,
                    fa_id,
                    // Without a quote nothing is reserved or paid yet, so
                    // a destination that no longer sells this seat needs a
                    // new selection, which closes the transfer below.
                    QuoteAttemptPolicy::ReplaceableSelection,
                    purchase.payment_federation_id.as_ref(),
                    Some(&payment_federation),
                    run,
                )
                .await
            {
                Ok(quoted) => quoted,
                // No quote and no reservation: nothing is at risk, and a
                // retry with the same payer and approval cannot succeed.
                Err(error @ FaError::SelectionReauthorizationRequired { .. }) => {
                    lease
                        .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                        .await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            if purchase.max_msats < quote.terms.price_msats {
                lease
                    .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                    .await?;
                return Err(FaError::SelectionReauthorizationRequired {
                    reason: crate::SelectionReauthorizationReason::QuoteTotalExceedsLimit,
                    failure: None,
                });
            }
            purchase.signed_quote = Some(signed);
            save_purchase(transfer, &purchase, fa_id, run).await?;
        }

        let signed = purchase
            .signed_quote
            .clone()
            .expect("the purchase holds a quote here");
        let verified = self.verify_quote(
            0,
            &signed,
            &purchase.locator,
            intent,
            fa_id,
            purchase.payment_federation_id.as_ref(),
        )?;
        let presentation = if verified.terms.payment.is_none() {
            SeatPresentation::Free(FreeSeatQuote { signed, verified })
        } else {
            let prepared = self
                .fund_seat_purchase(transfer, &mut purchase, &verified, fa_id, run)
                .await?;
            let Some(prepared) = prepared else {
                // The wallet proved the payment rejected and refunded.
                lease
                    .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                    .await?;
                return Err(FaError::Payment(
                    "the destination seat payment was rejected and its funds returned".to_owned(),
                ));
            };
            SeatPresentation::Paid {
                quote: PaidSeatQuote { signed, verified },
                prepared,
            }
        };
        match self
            .present_seat(
                &transfer.transfer_id,
                0,
                &client,
                &purchase.locator,
                fa_id,
                presentation,
                run,
            )
            .await?
        {
            SeatCreation::Accepted(acceptance) => Ok(StoredDestinationSeat {
                mg_id: purchase.mg_id,
                locator: purchase.locator,
                seat_id: acceptance.seat_id,
                guardian_fee_account: Some(acceptance.guardian_fee_account),
                holder: Some(purchase.holder),
            }),
            SeatCreation::Refused(refusal) => {
                if let Some(proof) = refusal.release_proof {
                    run.call("releasing refused destination seat payment", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .release_seat_payment_reservation(proof))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                }
                lease
                    .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                    .await?;
                Err(refusal.error.into())
            }
        }
    }

    /// Fund the purchase's paid quote, or replay its existing funding.
    ///
    /// `None` means the wallet proved the funding rejected and refunded.
    async fn fund_seat_purchase(
        &self,
        transfer: &StoredGuardianTransfer,
        purchase: &mut StoredSeatPurchase,
        quote: &SignatureVerified<GetQuoteResponse>,
        fa_id: crate::FaId,
        run: DriverRun<'_>,
    ) -> FaResult<Option<crate::PreparedSeatPayment<P::RefundContext>>> {
        let requirements = purchase_requirements(&transfer.transfer_id, purchase, quote)?;
        let reservation_id =
            crate::db::payment_reservation_id(&transfer.transfer_id, &requirements);
        if purchase.outputs_started {
            let recovered = run
                .call("recovering destination seat payment", || {
                    Ok(self
                        .inner
                        .ports
                        .payments
                        .recover_seat_payment(&reservation_id, quote))
                })
                .await?
                .map_err(|error| FaError::Payment(error.to_string()))?;
            match recovered {
                SeatPaymentRecovery::Prepared(prepared) => return Ok(Some(prepared)),
                SeatPaymentRecovery::Rejected(proof) => {
                    run.call("releasing rejected destination seat payment", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .release_seat_payment_reservation(proof))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                    return Ok(None);
                }
                SeatPaymentRecovery::NotStarted => {}
            }
        }
        let payer = quote
            .terms
            .payment
            .as_ref()
            .map(|terms| terms.federation_id().clone())
            .ok_or_else(|| {
                FaError::Storage("a free destination seat quote has no payment".to_owned())
            })?;
        let preflight =
            crate::ExactPaymentPreflight::new(&requirements, std::slice::from_ref(quote))?;
        let recovered = run
            .call("recovering destination seat reservation", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .recover_payment_reservation(&reservation_id, &preflight))
            })
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))?;
        let reservation = match recovered {
            PaymentReservationRecovery::Existing(reservation) => reservation,
            PaymentReservationRecovery::Absent if purchase.outputs_started => {
                return Err(FaError::Storage(
                    "destination seat payment started but its wallet reservation is absent"
                        .to_owned(),
                ));
            }
            PaymentReservationRecovery::Absent => run
                .call("reserving destination seat payment", || {
                    Ok(self
                        .inner
                        .ports
                        .payments
                        .reserve_payment_requirements(&reservation_id, &preflight))
                })
                .await?
                .map_err(|error| FaError::Payment(error.to_string()))?,
        };
        // The payer was checked when the quote was made. A payment that
        // never started may resume much later, so the current policy must
        // still allow the payer before any value moves.
        if purchase.outputs_started {
            match self.require_setup_payment_federation(&payer, run).await {
                Ok(_) => {}
                Err(error @ FaError::SelectionReauthorizationRequired { .. }) => {
                    // Mark the payment unstarted first: a crash before the
                    // close then replays as a stale quote, which releases the
                    // reservation and closes on the same policy check.
                    purchase.outputs_started = false;
                    save_purchase(transfer, purchase, fa_id, run).await?;
                    run.call("releasing destination seat reservation", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .release_payment_reservation(reservation))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                    run.lease()
                        .write_guardian_transfer(fa_id, GuardianTransferWrite::Close)
                        .await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        } else {
            purchase.outputs_started = true;
            save_purchase(transfer, purchase, fa_id, run).await?;
        }
        let budget = run
            .prepare_value_call_budget("funding destination seat payment")
            .await?;
        let prepared = budget
            .poll_value_call(
                run,
                self.inner
                    .ports
                    .payments
                    .create_seat_payment(&reservation, quote),
            )
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))?;
        Ok(Some(prepared))
    }
}

/// Save the purchase's progress in its transfer row.
async fn save_purchase(
    transfer: &StoredGuardianTransfer,
    purchase: &StoredSeatPurchase,
    fa_id: crate::FaId,
    run: DriverRun<'_>,
) -> FaResult<()> {
    let mut row = transfer.clone();
    row.stage = TransferStage::Buying(purchase.clone());
    run.lease()
        .write_guardian_transfer(fa_id, GuardianTransferWrite::Update(row))
        .await
}

/// The purchase's payment as an aggregate of one member.
fn purchase_requirements(
    transfer_id: &FormationId,
    purchase: &StoredSeatPurchase,
    quote: &SignatureVerified<GetQuoteResponse>,
) -> FaResult<PaymentRequirements> {
    let terms = quote.terms.payment.as_ref().ok_or_else(|| {
        FaError::Storage("a free destination seat quote has no payment".to_owned())
    })?;
    let seats = vec![SeatPaymentRequirement {
        index: 0,
        mg_id: purchase.mg_id,
        quote_id: quote.quote_id(),
        payment_federation_id: terms.federation_id().clone(),
        amount_msats: quote.terms.price_msats,
    }];
    let total_msats = quote.terms.price_msats;
    Ok(PaymentRequirements {
        authorization_id: crate::db::payment_authorization_id(transfer_id, total_msats, &seats),
        total_msats,
        max_total_msats: Some(purchase.max_msats),
        seats,
    })
}
