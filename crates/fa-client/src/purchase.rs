//! The seat purchase: buy the picked slots of a federation creation, one
//! seat at a time, until each slot holds an accepted seat or leaves.

use std::future::Future;
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::*;
use fedimint_core::runtime::Instant;
use stability_pool_common::Account;
use tokio::sync::OnceCell;

use crate::db::{NewCreation, PendingSlot, PickedSeat, PurchaseView, SlotState};
use crate::formation::{Timeouts, log_mg_failure, now_secs, sleep_for_retry};
use crate::mg::MgClient;
use crate::selection::AvailabilityMismatch;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader, FormationId,
    ManifoldGuardianConnector, MgCause, MgFailure, MgOperation, MgSelectionPreview,
    PreparedSeatPayment, SeatPaymentRecovery, SelectionReauthorizationReason,
    state::ResolvedSeatPurchase,
};

const SELECTED_MG_CONNECT_RETRY_BUDGET: Duration = Duration::from_secs(2 * 60);
const SELECTED_MG_CONNECT_RETRY_MIN_DELAY: Duration = Duration::from_millis(100);
const SELECTED_MG_CONNECT_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);

enum SeatCreation {
    Accepted(SeatAcceptance),
    Refused(MgFailure),
}

#[derive(Clone)]
struct SeatAcceptance {
    seat_id: SeatId,
    guardian_fee_account: Account,
}

fn reauthorization(reason: SelectionReauthorizationReason) -> FaError {
    FaError::SelectionReauthorizationRequired {
        reason,
        failure: None,
    }
}

/// Run one value-free attempt against a selected MG. A definite transport
/// loss reconnects for at most two minutes, and then the MG counts as
/// unavailable. Semantic refusals return immediately.
async fn retry_selected_mg<T, A: Future<Output = FaResult<T>>>(
    formation_id: &FormationId,
    timeouts: Timeouts,
    mut attempt: impl FnMut() -> A,
) -> FaResult<T> {
    let retry_deadline = Instant::now() + SELECTED_MG_CONNECT_RETRY_BUDGET;
    let mut retry_delay = timeouts
        .poll
        .max(SELECTED_MG_CONNECT_RETRY_MIN_DELAY)
        .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
    loop {
        let error = match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let FaError::Mg(failure) = error else {
            return Err(error);
        };
        if !matches!(failure.cause, MgCause::Timeout | MgCause::Transport) {
            return Err(failure.into());
        }
        if sleep_for_retry(retry_deadline, retry_delay).await.is_err() {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                failure: Some(failure),
            });
        }
        retry_delay = retry_delay
            .saturating_mul(2)
            .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
        log_mg_failure(formation_id, &FaError::Mg(failure), "retried");
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
    /// Start creating a federation by buying the seats of one approved
    /// preview.
    ///
    /// The preview is the approval, valid for five minutes, and must select
    /// every seat of the federation. A missing name asks `fa-client` to
    /// generate a human-friendly default. When a completion callback is
    /// given, FA persists it before any remote work, sends the same value to
    /// every guardian at `StartDkg`, never exposes the bearer in public
    /// state, and drops its copy with the DKG checkpoint.
    ///
    /// This saves the creation and its purchase, then returns.
    /// [`Self::run`] requests each seat's quote for exactly the previewed
    /// plan, pays automatically, and forms the federation once every seat is
    /// bought. Without a payment federation, the purchase proceeds only if
    /// every selected Manifold Guardian's live offer and exact signed quote
    /// are free. When a purchase ends before it buys its seats, the state
    /// asks for a new preview of the missing seats with its typed
    /// [`SelectionReauthorizationReason`].
    pub async fn create_federation(
        &self,
        federation_name: Option<FederationName>,
        completion_callback: Option<DkgCompletionCallback>,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        let fa_id = self.fa_id()?;
        let federation_name = federation_name
            .unwrap_or_else(|| default_federation_name(fa_id, now_secs().unwrap_or_default()));
        crate::state::validate_federation_name(&federation_name)?;
        let request = &preview.request;
        if request.seat_count() != request.federation_size().0 {
            return Err(FaError::InvalidIntent(
                "a new federation needs a preview of all its seats".to_owned(),
            ));
        }
        let creation = NewCreation {
            formation_id: FormationId(format!("{}-{}", fa_id.0, now_secs()?)),
            federation_name,
            federation_size: request.federation_size(),
            fedimintd_versions: request.fedimintd_versions().clone(),
            plan: request.plan(),
            dkg_completion_callback: completion_callback,
        };
        self.start_purchase(preview, payment_federation_id, Some(creation))
            .await
    }

    /// Preview the seats the current federation creation still needs.
    ///
    /// The preview keeps the DKG line of the seats already bought and
    /// avoids their MGs, service keys, and badge holders.
    pub async fn preview_missing_seats(
        &self,
        options: crate::MgDiscoveryOptions,
    ) -> FaResult<MgSelectionPreview> {
        let request = self
            .inner
            .store
            .missing_seats_request(self.fa_id()?)
            .await?;
        self.preview_mg_selection(&request, options).await
    }

    /// Buy the missing seats of the current federation creation.
    ///
    /// The preview must come from [`Self::preview_missing_seats`] for the
    /// current pool. Like [`Self::create_federation`], this saves the
    /// purchase and returns; [`Self::run`] buys the seats.
    pub async fn buy_seats(
        &self,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        self.start_purchase(preview, payment_federation_id, None)
            .await
    }

    /// Reject a preview that is not for exactly the seats the creation still
    /// needs. The caller holds the run guard, so no purchase can complete
    /// between this check and the new purchase.
    async fn require_missing_seats_preview(&self, preview: &MgSelectionPreview) -> FaResult<()> {
        let expected = self
            .inner
            .store
            .missing_seats_request(self.fa_id()?)
            .await?;
        if preview.request != expected
            || preview.seats().len() != usize::from(expected.seat_count())
        {
            return Err(FaError::InvalidIntent(
                "the preview does not match the seats this federation still needs".to_owned(),
            ));
        }
        Ok(())
    }

    /// Validate the preview and payer, then save the purchase and wake the
    /// run loop.
    async fn start_purchase(
        &self,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
        creation: Option<NewCreation>,
    ) -> FaResult<()> {
        let request = &preview.request;
        if preview.verifier_provenance != self.inner.peer_badge_verifier.provenance() {
            return Err(FaError::InvalidIntent(
                "selection preview belongs to a different verifier environment".to_owned(),
            ));
        }
        let fedimintd_dkg_version = preview.fedimintd_dkg_version.clone();
        if !fedimintd_dkg_version.is_fedi()
            || !request
                .fedimintd_versions()
                .overlaps_dkg(&fedimintd_dkg_version)
        {
            return Err(FaError::InvalidIntent(
                "selected Fedimint DKG identity is outside the purchase request".to_owned(),
            ));
        }
        let seats = preview
            .seats_to_buy_at(Timestamp(now_secs()?))?
            .into_iter()
            .map(|seat| PickedSeat {
                locator: seat.locator,
                mg_id: seat.mg_id,
                holder: seat.holder,
                plan: seat.plan,
            })
            .collect();
        let _run = self.claim_run_guard().await?;
        if creation.is_none() {
            self.require_missing_seats_preview(preview).await?;
        }
        let fa_id = self.fa_id()?;
        let timeouts = Timeouts::default();
        async {
            if let Some(requested) = &payment_federation_id {
                // A purchase must not obtain even its first exact quote until
                // the explicitly selected payer is authenticated and currently
                // Ready. The wallet checks its balance for each seat payment.
                self.require_setup_payment_federation(requested, timeouts)
                    .await?;
            }
            self.inner
                .store
                .initialize(
                    fa_id,
                    fedimintd_dkg_version,
                    seats,
                    payment_federation_id,
                    creation,
                )
                .await?;
            self.publish(None).await;
            Ok(())
        }
        .await
    }

    /// Stop creating a federation before any seat is bought.
    ///
    /// A running purchase ends while no seat payment has started. Zero-price
    /// seats a Manifold Guardian already accepted server-side are
    /// **forfeited**, not released. Once seats are bought or a seat payment
    /// started, this returns the typed [`FaError::AbandonUnavailable`]: the
    /// seats stay with the creation until its federation forms.
    ///
    /// The authenticated setup-payment policy retention is deliberately
    /// preserved; it is deployment policy, not creation state.
    pub async fn cancel_creation(&self) -> FaResult<()> {
        let _run = self.claim_run_guard().await?;
        let fa_id = self.fa_id()?;
        let timeouts = Timeouts::default();
        async {
            if let Some(purchase) = self.inner.store.load_purchase(fa_id).await? {
                for slot in &purchase.slots {
                    let Some(signed_quote) = slot.state.bound_quote() else {
                        continue;
                    };
                    let quote = self.verify_quote(
                        usize::from(slot.index),
                        signed_quote,
                        &slot.locator,
                        &purchase.intent,
                        fa_id,
                        purchase.payment_federation_id.as_ref(),
                    )?;
                    if quote.terms.payment.is_some()
                        && matches!(
                            self.recover_seat_payment(&quote, timeouts).await?,
                            SeatPaymentRecovery::Prepared(_)
                        )
                    {
                        return Err(FaError::AbandonUnavailable(
                            crate::AbandonUnavailableReason::PaymentOutputsStarted,
                        ));
                    }
                }
            }
            self.inner.store.cancel_creation(fa_id).await?;
            self.inner.progress.modify(|status| status.creation = None);
            Ok(())
        }
        .await
    }

    pub(crate) async fn drive_purchase(
        &self,
        purchase: PurchaseView,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let result = self.drive_purchase_inner(&purchase, fa_id, timeouts).await;
        // A purchase that ended without some of its seats is complete.
        if let Err(error) = &result
            && !matches!(error, FaError::SelectionReauthorizationRequired { .. })
        {
            let formation_id = &purchase.formation_id;
            log_mg_failure(formation_id, error, "returned");
            tracing::warn!(
                formation_id = %formation_id.0,
                %error,
                "seat purchase failed"
            );
            self.publish(Some(error.code())).await;
        }
        result
    }

    /// Buy the purchase's seats one at a time. A seat that cannot be bought
    /// leaves the purchase, and the purchase completes with the seats it
    /// bought. It then returns why the first seat left.
    async fn drive_purchase_inner(
        &self,
        purchase: &PurchaseView,
        fa_id: FaId,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let formation_id = &purchase.formation_id;
        self.publish(None).await;
        // Resolved on first use, so one authenticated read serves every
        // seat, and a purchase of free seats never reads a payer policy.
        let payer = OnceCell::new();
        let mut funds_exhausted = false;
        let mut left = None;
        let mut first_error = None;
        let mut slots = purchase.slots.clone();
        if slots.iter().all(|slot| slot.state.bound_quote().is_none()) {
            funds_exhausted = self
                .preflight_purchase(purchase, &mut slots, &mut left, timeouts)
                .await?;
        }
        for mut slot in slots {
            // A seat bound to its quote may hold a started payment, so it is
            // still recovered after the payer ran out of funds.
            let result = if funds_exhausted && slot.state.bound_quote().is_none() {
                Err(reauthorization(
                    SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                ))
            } else {
                self.buy_seat(purchase, &mut slot, fa_id, &payer, timeouts)
                    .await
            };
            match result {
                Ok(()) => {}
                Err(error @ FaError::SelectionReauthorizationRequired { reason, .. }) => {
                    log_mg_failure(formation_id, &error, "returned");
                    funds_exhausted |=
                        reason == SelectionReauthorizationReason::SelectedPayerInsufficientFunds;
                    self.drop_seat(purchase, &slot, reason).await?;
                    left.get_or_insert(error);
                }
                Err(error) => {
                    if first_error.is_some() {
                        log_mg_failure(formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.or(left).map_or(Ok(()), Err)
    }

    /// While no slot is bound, confirm that every picked MG still offers its
    /// plan and that the payer can fund every picked seat. A seat that fails
    /// leaves `slots` before any payment. Returns whether funds are short.
    async fn preflight_purchase(
        &self,
        purchase: &PurchaseView,
        slots: &mut Vec<PendingSlot>,
        left: &mut Option<FaError>,
        timeouts: Timeouts,
    ) -> FaResult<bool> {
        let intent = &purchase.intent;
        let formation_id = &purchase.formation_id;
        let checks = slots.iter().filter_map(|seat| {
            let SlotState::Picked { plan, .. } = &seat.state else {
                return None;
            };
            let index = seat.index;
            let locator = seat.locator.clone();
            Some(async move {
                let result = retry_selected_mg(formation_id, timeouts, || async {
                    let plan = plan.clone();
                    let client =
                        MgClient::connect(&self.inner.ports.mg_connector, &locator, timeouts)
                            .await?;
                    self.live_fedimintd_version(&client, intent, &plan, timeouts)
                        .await?;
                    Ok(plan)
                })
                .await;
                (index, result)
            })
        });
        let results = futures::future::join_all(checks).await;
        let mut prices = Vec::new();
        let mut ended = Vec::new();
        for (index, result) in results {
            match result {
                Ok(Plan::InfiniteBestEffort { price_msats })
                | Ok(Plan::SubscriptionBased {
                    initial_price_msats: price_msats,
                    ..
                }) => prices.extend((price_msats > 0).then_some(price_msats)),
                Err(error @ FaError::SelectionReauthorizationRequired { .. }) => {
                    ended.push((index, error));
                }
                // The next run checks again. Nothing was paid.
                Err(error) => return Err(error),
            }
        }
        for (index, error) in ended {
            let FaError::SelectionReauthorizationRequired { reason, .. } = &error else {
                unreachable!("only reauthorization errors end a seat");
            };
            let position = slots
                .iter()
                .position(|slot| slot.index == index)
                .expect("each seat leaves once");
            let slot = slots.remove(position);
            log_mg_failure(formation_id, &error, "returned");
            self.drop_seat(purchase, &slot, *reason).await?;
            left.get_or_insert(error);
        }
        let Some(requested) = &purchase.payment_federation_id else {
            return Ok(false);
        };
        if prices.is_empty() {
            return Ok(false);
        }
        let payer = self
            .require_setup_payment_federation(requested, timeouts)
            .await?;
        let checked = timeouts
            .call("checking seat funds", || {
                Ok(self.inner.ports.payments.check_seat_funds(&payer, &prices))
            })
            .await?;
        match checked {
            Ok(()) => Ok(false),
            Err(error) if error.proves_insufficient_funds() => Ok(true),
            Err(error) => Err(FaError::Payment(error.to_string())),
        }
    }

    /// Buy one seat: bind a quote, pay it, and present it to its MG.
    ///
    /// A [`FaError::SelectionReauthorizationRequired`] error proves that the
    /// seat cannot be bought and that no payment of it is outstanding.
    async fn buy_seat(
        &self,
        purchase: &PurchaseView,
        slot: &mut PendingSlot,
        fa_id: FaId,
        payer: &OnceCell<FederationId>,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let formation_id = &purchase.formation_id;
        let intent = &purchase.intent;
        let expected_payer = purchase.payment_federation_id.as_ref();
        let position = usize::from(slot.index);
        let bound_quote = slot.state.bound_quote().cloned();
        let plan = match &slot.state {
            SlotState::Accepted { .. } | SlotState::Ended { .. } => {
                return Err(FaError::Storage(format!(
                    "FA slot {} is not pending",
                    slot.index
                )));
            }
            SlotState::Picked { plan, .. } => plan.clone(),
            SlotState::Bound {
                quote: signed_quote,
                ..
            } => {
                let signed_quote = signed_quote.clone();
                let quote = self.verify_quote(
                    position,
                    &signed_quote,
                    &slot.locator,
                    intent,
                    fa_id,
                    expected_payer,
                )?;
                if quote.terms.payment.is_none() {
                    // CreateSeat may have accepted this quote and lost its
                    // reply, so the bound quote is presented again.
                    return self
                        .present_bound_seat(
                            purchase,
                            slot,
                            fa_id,
                            signed_quote,
                            quote,
                            None,
                            timeouts,
                        )
                        .await;
                }
                match self.recover_seat_payment(&quote, timeouts).await? {
                    SeatPaymentRecovery::Prepared(prepared) => {
                        return self
                            .present_bound_seat(
                                purchase,
                                slot,
                                fa_id,
                                signed_quote,
                                quote,
                                Some(prepared),
                                timeouts,
                            )
                            .await;
                    }
                    SeatPaymentRecovery::Rejected => {
                        return Err(reauthorization(
                            SelectionReauthorizationReason::SelectedPayerUnavailable,
                        ));
                    }
                    // The bound quote may have expired, so the seat pays a new
                    // quote at the same plan.
                    SeatPaymentRecovery::NotStarted => quote.terms.request.plan.clone(),
                }
            }
        };

        let (signed_quote, quote) = self
            .request_quote_with_retry(
                formation_id,
                position,
                &slot.locator,
                intent,
                &plan,
                fa_id,
                expected_payer,
                expected_payer.is_some().then_some(payer),
                timeouts,
            )
            .await?;
        self.inner
            .store
            .bind_quote(
                formation_id,
                slot.index,
                bound_quote.as_ref(),
                signed_quote.clone(),
            )
            .await?;
        let slot_purchase = slot
            .state
            .purchase()
            .cloned()
            .expect("a pending slot carries its purchase");
        slot.state = SlotState::Bound {
            quote: signed_quote.clone(),
            purchase: slot_purchase,
        };
        self.publish(None).await;

        let prepared = if quote.terms.payment.is_some() {
            let prepared = timeouts
                .long_call("funding seat payment", || {
                    Ok(self.inner.ports.payments.create_seat_payment(&quote))
                })
                .await?
                .map_err(|error| {
                    if error.proves_insufficient_funds() {
                        reauthorization(
                            SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                        )
                    } else {
                        FaError::Payment(error.to_string())
                    }
                })?;
            Some(prepared)
        } else {
            None
        };
        self.present_bound_seat(
            purchase,
            slot,
            fa_id,
            signed_quote,
            quote,
            prepared,
            timeouts,
        )
        .await
    }

    /// Present a seat's bound quote, with its payment if it is paid, and
    /// record the accepted seat.
    #[allow(clippy::too_many_arguments)]
    async fn present_bound_seat(
        &self,
        purchase: &PurchaseView,
        slot: &PendingSlot,
        fa_id: FaId,
        signed_quote: SignedResponse<GetQuoteResponse>,
        quote: SignatureVerified<GetQuoteResponse>,
        prepared: Option<PreparedSeatPayment<P::RefundContext>>,
        timeouts: Timeouts,
    ) -> FaResult<()> {
        let formation_id = &purchase.formation_id;
        let client =
            MgClient::connect(&self.inner.ports.mg_connector, &slot.locator, timeouts).await?;
        let creation = self
            .present_seat(
                formation_id,
                usize::from(slot.index),
                &client,
                &slot.locator,
                fa_id,
                signed_quote,
                quote,
                prepared,
                timeouts,
            )
            .await?;
        match creation {
            SeatCreation::Accepted(acceptance) => {
                self.inner
                    .store
                    .record_seat_accepted(
                        formation_id,
                        slot.index,
                        acceptance.seat_id,
                        acceptance.guardian_fee_account,
                    )
                    .await?;
                self.publish(None).await;
                Ok(())
            }
            // A refused payment was refunded before `present_seat` returned.
            SeatCreation::Refused(failure) => Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                failure: Some(failure),
            }),
        }
    }

    async fn recover_seat_payment(
        &self,
        quote: &SignatureVerified<GetQuoteResponse>,
        timeouts: Timeouts,
    ) -> FaResult<SeatPaymentRecovery<P::RefundContext>> {
        timeouts
            .long_call("recovering seat payment", || {
                Ok(self.inner.ports.payments.recover_seat_payment(quote))
            })
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))
    }

    /// Remove a seat that cannot be bought, and keep its reason for the
    /// preview of the missing seats.
    async fn drop_seat(
        &self,
        purchase: &PurchaseView,
        slot: &PendingSlot,
        reason: SelectionReauthorizationReason,
    ) -> FaResult<()> {
        self.inner
            .store
            .empty_slots(
                &purchase.formation_id,
                vec![(slot.index, slot.state.clone())],
                reason,
            )
            .await?;
        self.publish(None).await;
        Ok(())
    }

    /// Connect, inspect availability, and request one quote as a single
    /// bounded value-free attempt.
    #[allow(clippy::too_many_arguments)]
    async fn request_quote_with_retry(
        &self,
        formation_id: &FormationId,
        index: usize,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        pinned_plan: &Plan,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        timeouts: Timeouts,
    ) -> FaResult<(
        SignedResponse<GetQuoteResponse>,
        SignatureVerified<GetQuoteResponse>,
    )> {
        retry_selected_mg(formation_id, timeouts, || async {
            let client =
                MgClient::connect(&self.inner.ports.mg_connector, locator, timeouts).await?;
            self.request_new_quote(
                index,
                &client,
                locator,
                intent,
                pinned_plan,
                fa_id,
                expected_payer,
                payment_federation,
                timeouts,
            )
            .await
        })
        .await
    }

    /// The Fedimint build an MG offers live for this purchase, if it still
    /// accepts a seat at exactly the previewed plan.
    async fn live_fedimintd_version(
        &self,
        client: &MgClient<F::Client>,
        intent: &ResolvedSeatPurchase,
        pinned_plan: &Plan,
        timeouts: Timeouts,
    ) -> FaResult<FedimintdVersion> {
        let availability = client
            .call(timeouts, MgOperation::GetAvailability, |client| async {
                crate::mg::fold_transport(
                    self.inner
                        .ports
                        .mg_connector
                        .get_availability(client, GetAvailabilityRequest)
                        .await,
                )
            })
            .await?;
        // One shared predicate with the selection walk's live probe
        // (`selection::match_requested_availability`), so a candidate the
        // probing preview seats is exactly a candidate this gate accepts.
        // Requiring the previewed plan gives the quote exactly the previewed
        // price.
        match crate::selection::match_requested_availability(
            &availability,
            intent.federation_size,
            &intent.fedimintd_versions,
            &intent.fedimintd_dkg_version,
            pinned_plan,
        ) {
            Ok(fedimintd_version) => Ok(fedimintd_version.clone()),
            Err(mismatch) => {
                let cause = match mismatch {
                    AvailabilityMismatch::NotAcceptingSeats => MgCause::NotAcceptingSeats,
                    AvailabilityMismatch::FederationSize => MgCause::UnsupportedFederationSize,
                    AvailabilityMismatch::FedimintdVersion => MgCause::UnsupportedVersion,
                    AvailabilityMismatch::Plan => MgCause::PlanNotOffered,
                };
                let failure = client.failure(MgOperation::GetAvailability, cause);
                Err(FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SeatUnavailable,
                    failure: Some(failure),
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_new_quote(
        &self,
        index: usize,
        client: &MgClient<F::Client>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        pinned_plan: &Plan,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        timeouts: Timeouts,
    ) -> FaResult<(
        SignedResponse<GetQuoteResponse>,
        SignatureVerified<GetQuoteResponse>,
    )> {
        let fedimintd_version = self
            .live_fedimintd_version(client, intent, pinned_plan, timeouts)
            .await?;
        let plan = pinned_plan.clone();
        // Free-ness is a property of the price, not of the plan: an MG that
        // gives its seats away is quoted against no payment federation
        // whatever this FA is configured to pay from, and one that charges
        // cannot be quoted at all by an FA with nothing to pay from.
        let priced = match &plan {
            Plan::InfiniteBestEffort { price_msats } => *price_msats > 0,
            Plan::SubscriptionBased { .. } => true,
        };
        let payment_federation_id = if priced {
            let payment_federation = payment_federation.ok_or_else(|| {
                reauthorization(SelectionReauthorizationReason::PaymentFederationRequired)
            })?;
            Some(
                payment_federation
                    .get_or_try_init(|| async {
                        match expected_payer {
                            Some(requested) => {
                                self.require_setup_payment_federation(requested, timeouts)
                                    .await
                            }
                            None => Err(FaError::SelectionReauthorizationRequired {
                                reason: SelectionReauthorizationReason::PaymentFederationRequired,
                                failure: None,
                            }),
                        }
                    })
                    .await?
                    .clone(),
            )
        } else {
            None
        };
        let refund_issuance = match &payment_federation_id {
            Some(federation_id) => Some(
                timeouts
                    .call("preparing quote refund", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .prepare_quote_refund(federation_id, &plan))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?,
            ),
            None => None,
        };
        let quote_request = GetQuoteRequest {
            fa_id,
            fedimintd_version,
            federation_size: intent.federation_size,
            plan,
            payment_federation_id,
            refund_issuance,
        };
        let result = async {
            let signed_quote = client
                .call(timeouts, MgOperation::GetQuote, |client| async {
                    crate::mg::fold_transport(
                        self.inner
                            .ports
                            .mg_connector
                            .get_quote(client, quote_request.clone())
                            .await,
                    )
                })
                .await?;
            let quote = self
                .verify_quote(index, &signed_quote, locator, intent, fa_id, expected_payer)
                .map_err(|_| client.failure(MgOperation::GetQuote, MgCause::InvalidResponse))?;
            // The requested plan is the previewed one, so a quote must charge
            // exactly its price.
            let plan_price_msats = match &quote_request.plan {
                Plan::InfiniteBestEffort { price_msats } => Some(*price_msats),
                Plan::SubscriptionBased { .. } => None,
            };
            if quote.terms.request != quote_request
                || plan_price_msats != Some(quote.terms.price_msats)
            {
                return Err(client
                    .failure(MgOperation::GetQuote, MgCause::InvalidResponse)
                    .into());
            }
            Ok((signed_quote, quote))
        }
        .await;
        result.map_err(|error| match error {
            FaError::Mg(failure)
                if matches!(
                    failure.cause,
                    MgCause::CapacityExhausted
                        | MgCause::PlanNotOffered
                        | MgCause::PaymentFederationNotAccepted
                        | MgCause::UnsupportedVersion
                        | MgCause::UnsupportedFederationSize
                        | MgCause::InvalidResponse
                ) =>
            {
                FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SeatUnavailable,
                    failure: Some(failure),
                }
            }
            error => error,
        })
    }

    fn verify_quote(
        &self,
        index: usize,
        signed_quote: &SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
    ) -> FaResult<SignatureVerified<GetQuoteResponse>> {
        let quote = signed_quote
            .verify(&locator.service_pubkey)
            .map_err(|error| mg_error(index, format!("invalid signed quote: {error}")))?;
        let request = &quote.terms.request;
        if request.fa_id != fa_id
            || request.fedimintd_version.dkg_version() != intent.fedimintd_dkg_version
            || !intent
                .fedimintd_versions
                .contains(&request.fedimintd_version)
            || request.federation_size != intent.federation_size
            || !intent.plan.matches(&request.plan)
        {
            return Err(mg_error(
                index,
                "stored quote does not match the formation intent",
            ));
        }
        if let Some(expected_payer) = expected_payer
            && quote
                .terms
                .payment
                .as_ref()
                .is_some_and(|payment| payment.federation_id() != expected_payer)
        {
            return Err(mg_error(
                index,
                "stored quote does not match the selected payment federation",
            ));
        }
        quote
            .terms
            .check_coherent()
            .map_err(|error| mg_error(index, format!("incoherent quote: {error}")))?;
        Ok(quote)
    }

    #[allow(clippy::too_many_arguments)]
    async fn present_seat(
        &self,
        formation_id: &FormationId,
        index: usize,
        client: &MgClient<F::Client>,
        locator: &Locator,
        fa_id: FaId,
        signed_quote: SignedResponse<GetQuoteResponse>,
        quote: SignatureVerified<GetQuoteResponse>,
        prepared: Option<PreparedSeatPayment<P::RefundContext>>,
        timeouts: Timeouts,
    ) -> FaResult<SeatCreation> {
        let (payment_signatures, refund_context) = match (&quote.terms.payment, prepared) {
            (None, None) => (Vec::new(), None),
            (Some(terms), Some(prepared)) => {
                if prepared.settled_under != terms.generation() {
                    return Err(mg_error(
                        index,
                        "wallet payment generation did not match the quote",
                    ));
                }
                (prepared.payment_signatures, Some(prepared.refund_context))
            }
            _ => {
                return Err(FaError::Storage(format!(
                    "FA seat row {index} payment does not match its quote"
                )));
            }
        };
        let quote_id = quote.quote_id();
        let request = CreateSeatRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            quote: signed_quote,
            payment_signatures,
        };
        let request = self.sign(&request)?;
        let response = client
            .call(timeouts, MgOperation::CreateSeat, |client| {
                client.create_seat(request)
            })
            .await?
            .verify(&locator.service_pubkey)
            .map_err(|_| client.failure(MgOperation::CreateSeat, MgCause::InvalidResponse))?;
        if response.quote_id != quote_id {
            return Err(client
                .failure(MgOperation::CreateSeat, MgCause::InvalidResponse)
                .into());
        }
        match response.into_inner().outcome {
            CreateSeatOutcome::Accepted {
                seat_id,
                guardian_fee_account,
            } => {
                canonical_guardian_fee_recipient_list(&[GuardianFeeRecipient::new(
                    guardian_fee_account.clone(),
                    GUARDIAN_GUARDIAN_FEE_WEIGHT,
                )])
                .map_err(|_| client.failure(MgOperation::CreateSeat, MgCause::InvalidResponse))?;
                Ok(SeatCreation::Accepted(SeatAcceptance {
                    seat_id,
                    guardian_fee_account: guardian_fee_account.into_account(),
                }))
            }
            CreateSeatOutcome::Refused {
                reason,
                refund_transaction,
            } => {
                let cause = match reason {
                    RefusalReason::OfferChanged => MgCause::OfferChanged,
                };
                let error = client.failure(MgOperation::CreateSeat, cause);
                log_mg_failure(formation_id, &FaError::Mg(error.clone()), "pending_refund");
                match (refund_context, refund_transaction) {
                    (Some(refund_context), Some(refund)) => {
                        timeouts
                            .long_call("settling refused seat refund", || {
                                Ok(self
                                    .inner
                                    .ports
                                    .payments
                                    .settle_seat_refund(refund_context, refund))
                            })
                            .await?
                            .map_err(|error| FaError::Payment(error.to_string()))?;
                    }
                    (None, None) => {}
                    _ => {
                        return Err(mg_error(
                            index,
                            "refusal payment and refund material did not match the quote",
                        ));
                    }
                }
                Ok(SeatCreation::Refused(error))
            }
        }
    }
}

fn default_federation_name(fa_id: FaId, created_at: u64) -> FederationName {
    const ADJECTIVES: [&str; 16] = [
        "Amber", "Bright", "Calm", "Cedar", "Clear", "Copper", "Gentle", "Golden", "Kind",
        "Lively", "Quiet", "Silver", "Solar", "Steady", "Warm", "Wild",
    ];
    const NOUNS: [&str; 16] = [
        "Badger", "Beacon", "Bison", "Canyon", "Falcon", "Forest", "Harbor", "Juniper", "Meadow",
        "Orchid", "Otter", "Raven", "River", "Summit", "Willow", "Wren",
    ];
    let key = fa_id.0.serialize();
    let adjective = ADJECTIVES[usize::from(key[0] ^ (created_at as u8)) % ADJECTIVES.len()];
    let noun = NOUNS[usize::from(key[31] ^ ((created_at >> 8) as u8)) % NOUNS.len()];
    FederationName(format!("{adjective} {noun}"))
}

fn mg_error(index: usize, message: impl Into<String>) -> FaError {
    FaError::ManifoldGuardian {
        index: u16::try_from(index).expect("validated formation size fits u16"),
        message: message.into(),
    }
}
