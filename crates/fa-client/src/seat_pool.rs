//! Spare seats: seats the FA bought that host no guardian of its federation.
//!
//! A spare seat comes from a single-seat purchase. A guardian transfer later
//! moves one guardian onto it; until then it is a paid, unformed `New` seat on
//! its MG. The pool is separate from the formation's own seat rows, which
//! keep describing the guardians the federation runs.
//!
//! The purchase pays through the same [`FaPayments`] port as formation, as an
//! aggregate of one member under its own reservation scope. Its value-safety
//! rules are formation's, for one seat:
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

use crate::db::{FaRecovery, SeatPoolWrite};
use crate::formation::{
    DriverRun, FreeSeatQuote, PaidSeatQuote, QuoteAttemptPolicy, SeatCreation, SeatPresentation,
    finish_driver_run, now_secs, start_driver_run,
};
use crate::mg::MgClient;
use crate::selection::{SpareSeatApproval, SpareSeatPreview};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader, FormationId,
    FormationPhase, FormationRunOptions, ManifoldGuardianConnector, MgDiscoveryOptions,
    MgSelectionRequest, PaymentRequirements, PaymentReservationRecovery, SeatPaymentRecovery,
    SeatPaymentRequirement,
};

/// One seat the FA owns that hosts no guardian.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpareSeat {
    /// Badge-vouched identity of the MG that hosts the seat.
    pub mg_id: PublicKey,
    /// Protocol-owned locator for the MG.
    pub locator: Locator,
    /// MG-minted seat id.
    pub seat_id: SeatId,
}

/// Durable pool entry.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSpareSeat {
    #[serde(rename = "fman_id")]
    pub(crate) mg_id: PublicKey,
    pub(crate) locator: Locator,
    pub(crate) seat_id: SeatId,
    /// The MG-signed account from the seat's acceptance. Absent for a
    /// spare restored from a backup, which carries no acceptance.
    pub(crate) guardian_fee_account: Option<Account>,
}

impl From<StoredSpareSeat> for SpareSeat {
    fn from(seat: StoredSpareSeat) -> Self {
        Self {
            mg_id: seat.mg_id,
            locator: seat.locator,
            seat_id: seat.seat_id,
        }
    }
}

/// The one open single-seat purchase.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSeatPurchase {
    /// Scope for this purchase's payment authorization and reservation ids.
    pub(crate) purchase_id: FormationId,
    #[serde(rename = "fman_id")]
    pub(crate) mg_id: PublicKey,
    pub(crate) holder: PublicKey,
    pub(crate) locator: Locator,
    /// Spending cap the user approved for this seat.
    pub(crate) max_msats: u64,
    /// Current quote. Replaced only while it is free of value effects.
    pub(crate) signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    /// Durable output boundary: once set, the quote is replayed exactly.
    pub(crate) outputs_started: bool,
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Select one fresh MG for a spare seat of the formed federation.
    ///
    /// The MG holds no seat of this federation or pool, and its badge
    /// holder vouches for none of them.
    pub async fn preview_spare_seat(
        &self,
        options: MgDiscoveryOptions,
    ) -> FaResult<SpareSeatPreview> {
        let fa_id = self.fa_id()?;
        let recovery = self.formed_recovery(fa_id).await?;
        let intent = &recovery.snapshot.intent;
        let request = MgSelectionRequest::new(
            intent.federation_size,
            intent.fedimintd_versions.clone(),
            intent.plan,
        )?;
        let mut excluded = BTreeSet::new();
        let mut retained_service_pubkeys = BTreeMap::new();
        let mut retained_holders = BTreeSet::new();
        for seat in &recovery.seats {
            retained_holders.extend(seat.admission.holder());
            if let Some(mg_id) = seat.admission.mg_id() {
                excluded.insert(mg_id);
                retained_service_pubkeys.insert(seat.progress.locator.service_pubkey, mg_id);
            }
        }
        for spare in self.inner.store.list_spare_seats().await? {
            excluded.insert(spare.mg_id);
            retained_service_pubkeys.insert(spare.locator.service_pubkey, spare.mg_id);
        }
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        crate::selection::preview_spare_seat_with(
            &self.inner.ports.registry,
            &self.inner.peer_badge_verifier,
            &crate::selection::LiveAvailabilityProber {
                connector: Some(&self.inner.ports.mg_connector),
            },
            self.inner.peer_badge_verifier.provenance(),
            &request,
            &intent.fedimintd_dkg_version,
            excluded,
            retained_service_pubkeys,
            retained_holders,
            deadline,
            now,
            || fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }

    /// Buy the approved seat into the pool.
    ///
    /// Only one purchase is open at a time. If this call fails after the
    /// purchase was recorded, [`Self::resume_spare_seat_purchase`] continues
    /// the same purchase.
    pub async fn buy_spare_seat(
        &self,
        approval: SpareSeatApproval,
        options: FormationRunOptions,
    ) -> FaResult<SpareSeat> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let now = now_secs()?;
        if approval.valid_until <= Timestamp(now) {
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
        let recovery = self.formed_recovery(fa_id).await?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            let purchase = StoredSeatPurchase {
                purchase_id: FormationId(format!(
                    "{}:spare-seat:{now}",
                    recovery.snapshot.formation_id.0
                )),
                mg_id: approval.seat.mg_id,
                holder: approval.seat.holder,
                locator: approval.seat.locator,
                max_msats: approval.max_msats,
                signed_quote: None,
                outputs_started: false,
            };
            lease
                .write_seat_pool(SeatPoolWrite::Begin(purchase.clone()))
                .await?;
            self.drive_seat_purchase(purchase, &recovery.snapshot.intent, fa_id, run)
                .await
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Continue the open purchase, if there is one.
    pub async fn resume_spare_seat_purchase(
        &self,
        options: FormationRunOptions,
    ) -> FaResult<Option<SpareSeat>> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let Some(purchase) = self.inner.store.load_seat_purchase().await? else {
            return Ok(None);
        };
        let recovery = self.formed_recovery(fa_id).await?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self
            .drive_seat_purchase(purchase, &recovery.snapshot.intent, fa_id, run)
            .await
            .map(Some);
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Seats the FA owns that host no guardian.
    pub async fn spare_seats(&self) -> FaResult<Vec<SpareSeat>> {
        Ok(self
            .inner
            .store
            .list_spare_seats()
            .await?
            .into_iter()
            .map(SpareSeat::from)
            .collect())
    }

    async fn formed_recovery(
        &self,
        fa_id: crate::FaId,
    ) -> FaResult<crate::db::ActiveFormationRecovery> {
        match self.inner.store.load_recovery(fa_id).await? {
            FaRecovery::Formation(recovery)
                if recovery.snapshot.phase == FormationPhase::Formed =>
            {
                Ok(*recovery)
            }
            FaRecovery::Restored(_) => Err(FaError::InvalidIntent(
                "a restored FA has no local formation intent to buy a seat with".to_owned(),
            )),
            FaRecovery::Idle | FaRecovery::Formation(_) => Err(FaError::NoActiveFormation),
        }
    }

    async fn drive_seat_purchase(
        &self,
        mut purchase: StoredSeatPurchase,
        intent: &crate::ResolvedFormationIntent,
        fa_id: crate::FaId,
        run: DriverRun<'_>,
    ) -> FaResult<SpareSeat> {
        let lease = run.lease();
        let client =
            MgClient::connect(&self.inner.ports.mg_connector, &purchase.locator, run).await?;
        let payment_federation = OnceCell::new();

        // A stored paid quote whose outputs never started may be stale; give
        // back its reservation and quote again before any value moves.
        if !purchase.outputs_started
            && let Some(signed) = &purchase.signed_quote
        {
            let quote = self.verify_quote(0, signed, &purchase.locator, intent, fa_id, None)?;
            if quote.terms.payment.is_some() {
                let requirements = purchase_requirements(&purchase, &quote)?;
                let quotes = [quote];
                let preflight = crate::ExactPaymentPreflight::new(&requirements, &quotes)?;
                let reservation_id =
                    crate::db::payment_reservation_id(&purchase.purchase_id, &requirements);
                let recovered = run
                    .call("recovering spare seat reservation", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .recover_payment_reservation(&reservation_id, &preflight))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                if let PaymentReservationRecovery::Existing(reservation) = recovered {
                    run.call("releasing stale spare seat reservation", || {
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
                lease
                    .write_seat_pool(SeatPoolWrite::Update(purchase.clone()))
                    .await?;
            }
        }

        if purchase.signed_quote.is_none() {
            let (signed, quote) = self
                .request_new_quote(
                    0,
                    &client,
                    &purchase.locator,
                    intent,
                    fa_id,
                    QuoteAttemptPolicy::ExactRecovery,
                    None,
                    Some(&payment_federation),
                    run,
                )
                .await?;
            if purchase.max_msats < quote.terms.price_msats {
                lease.write_seat_pool(SeatPoolWrite::Abort).await?;
                return Err(FaError::SelectionReauthorizationRequired {
                    reason: crate::SelectionReauthorizationReason::QuoteTotalExceedsLimit,
                    failure: None,
                });
            }
            purchase.signed_quote = Some(signed);
            lease
                .write_seat_pool(SeatPoolWrite::Update(purchase.clone()))
                .await?;
        }

        let signed = purchase
            .signed_quote
            .clone()
            .expect("the purchase holds a quote here");
        let verified = self.verify_quote(0, &signed, &purchase.locator, intent, fa_id, None)?;
        let presentation = if verified.terms.payment.is_none() {
            SeatPresentation::Free(FreeSeatQuote { signed, verified })
        } else {
            let prepared = self
                .fund_seat_purchase(&mut purchase, &verified, run)
                .await?;
            let Some(prepared) = prepared else {
                // The wallet proved the payment rejected and refunded.
                lease.write_seat_pool(SeatPoolWrite::Abort).await?;
                return Err(FaError::Payment(
                    "the spare seat payment was rejected and its funds returned".to_owned(),
                ));
            };
            SeatPresentation::Paid {
                quote: PaidSeatQuote { signed, verified },
                prepared,
            }
        };
        let formation_id = purchase.purchase_id.clone();
        match self
            .present_seat(
                &formation_id,
                0,
                &client,
                &purchase.locator,
                fa_id,
                presentation,
                run,
            )
            .await?
        {
            SeatCreation::Accepted(acceptance) => {
                let spare = StoredSpareSeat {
                    mg_id: purchase.mg_id,
                    locator: purchase.locator,
                    seat_id: acceptance.seat_id,
                    guardian_fee_account: Some(acceptance.guardian_fee_account),
                };
                lease
                    .write_seat_pool(SeatPoolWrite::Complete(spare.clone()))
                    .await?;
                Ok(spare.into())
            }
            SeatCreation::Refused(refusal) => {
                if let Some(proof) = refusal.release_proof {
                    run.call("releasing refused spare seat payment", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .release_seat_payment_reservation(proof))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                }
                lease.write_seat_pool(SeatPoolWrite::Abort).await?;
                Err(refusal.error.into())
            }
        }
    }

    /// Fund the purchase's paid quote, or replay its existing funding.
    ///
    /// `None` means the wallet proved the funding rejected and refunded.
    async fn fund_seat_purchase(
        &self,
        purchase: &mut StoredSeatPurchase,
        quote: &SignatureVerified<GetQuoteResponse>,
        run: DriverRun<'_>,
    ) -> FaResult<Option<crate::PreparedSeatPayment<P::RefundContext>>> {
        let lease = run.lease();
        let requirements = purchase_requirements(purchase, quote)?;
        let reservation_id =
            crate::db::payment_reservation_id(&purchase.purchase_id, &requirements);
        if purchase.outputs_started {
            let recovered = run
                .call("recovering spare seat payment", || {
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
                    run.call("releasing rejected spare seat payment", || {
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
        let preflight =
            crate::ExactPaymentPreflight::new(&requirements, std::slice::from_ref(quote))?;
        let recovered = run
            .call("recovering spare seat reservation", || {
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
                    "spare seat payment started but its wallet reservation is absent".to_owned(),
                ));
            }
            PaymentReservationRecovery::Absent => run
                .call("reserving spare seat payment", || {
                    Ok(self
                        .inner
                        .ports
                        .payments
                        .reserve_payment_requirements(&reservation_id, &preflight))
                })
                .await?
                .map_err(|error| FaError::Payment(error.to_string()))?,
        };
        if !purchase.outputs_started {
            purchase.outputs_started = true;
            lease
                .write_seat_pool(SeatPoolWrite::Update(purchase.clone()))
                .await?;
        }
        let budget = run
            .prepare_value_call_budget("funding spare seat payment")
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

/// The purchase's payment as an aggregate of one member.
fn purchase_requirements(
    purchase: &StoredSeatPurchase,
    quote: &SignatureVerified<GetQuoteResponse>,
) -> FaResult<PaymentRequirements> {
    let terms = quote
        .terms
        .payment
        .as_ref()
        .ok_or_else(|| FaError::Storage("a free spare seat quote has no payment".to_owned()))?;
    let seats = vec![SeatPaymentRequirement {
        index: 0,
        mg_id: Some(purchase.mg_id),
        quote_id: quote.quote_id(),
        payment_federation_id: terms.federation_id().clone(),
        amount_msats: quote.terms.price_msats,
    }];
    let total_msats = quote.terms.price_msats;
    Ok(PaymentRequirements {
        authorization_id: crate::db::payment_authorization_id(
            &purchase.purchase_id,
            total_msats,
            &seats,
        ),
        total_msats,
        max_total_msats: Some(purchase.max_msats),
        seats,
    })
}
