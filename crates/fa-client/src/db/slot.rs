//! The slots: one row per picked MG, from its pick through its bound
//! quote to its accepted seat.

use super::*;

/// What one purchase gives each slot it buys. Every slot that is not yet
/// accepted belongs to the same purchase.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct SlotPurchase {
    /// Major/minor/vendor identity of every seat the purchase buys.
    pub(crate) fedimintd_dkg_version: FedimintdDkgVersion,
    /// Explicit consumer-selected ready payer, or `None` for the verified
    /// zero-price deployment-bootstrap path.
    pub(crate) payment_federation_id: Option<FederationId>,
}

/// One guardian slot of the creation, keyed by a stable id. A slot exists
/// from the preview that picked its MG until the federation forms. It leaves
/// when its seat cannot be bought.
#[derive(Debug, Decodable, Encodable)]
pub(super) struct SlotKey {
    pub(super) index: u16,
}

#[derive(Debug, Decodable, Encodable)]
pub(super) struct SlotKeyPrefix;

impl_db_record!(
    key = SlotKey,
    value = StoredSlot,
    db_prefix = FaDbPrefix::Slot,
);

impl_db_lookup!(key = SlotKey, query_prefix = SlotKeyPrefix);

/// The verified MG picked for one slot, and how far its seat got.
///
/// `mg_id` is the badge-vouched Nostr author. The locator's service key is
/// deliberately not identity: it is rotatable commitment/dialing material.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct StoredSlot {
    pub(super) locator: Locator,
    pub(super) mg_id: PublicKey,
    /// Verified badge holder of the MG. Only slots migrated from schema 11
    /// storage, which did not always record it, lack it.
    pub(super) holder: Option<PublicKey>,
    pub(super) state: SlotState,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SlotState {
    /// No quote is bound. The previewed plan gives the quote exactly the
    /// previewed price.
    Picked { plan: Plan, purchase: SlotPurchase },
    /// The slot's payment or presentation may have started for this exact
    /// MG-signed quote. It holds only public terms.
    Bound {
        quote: SignedResponse<GetQuoteResponse>,
        purchase: SlotPurchase,
    },
    /// The MG accepted the seat.
    Accepted {
        seat_id: SeatId,
        /// MG-signed account from the seat's acceptance.
        guardian_fee_account: Account,
        /// Exact Fedimint build from the seat's quote.
        fedimintd_version: FedimintdVersion,
        /// The code the MG returned for the full pool, kept until the
        /// federation forms. MGs refuse the code request once DKG starts, and
        /// a DKG restart needs every code again.
        // TODO: remove once MGs return the code in every state.
        guardian_code: Option<GuardianCode>,
    },
}

impl StoredSlot {
    pub(super) fn accepted(&self) -> Option<AcceptedSlot<'_>> {
        match &self.state {
            SlotState::Accepted {
                seat_id,
                guardian_fee_account,
                fedimintd_version,
                guardian_code,
            } => Some(AcceptedSlot {
                slot: self,
                seat_id,
                guardian_fee_account,
                fedimintd_version,
                guardian_code: guardian_code.as_ref(),
            }),
            SlotState::Picked { .. } | SlotState::Bound { .. } => None,
        }
    }
}

impl SlotState {
    pub(crate) fn bound_quote(&self) -> Option<&SignedResponse<GetQuoteResponse>> {
        match self {
            Self::Bound { quote, .. } => Some(quote),
            Self::Picked { .. } | Self::Accepted { .. } => None,
        }
    }

    /// The purchase that buys this slot, until the MG accepts it.
    pub(crate) fn purchase(&self) -> Option<&SlotPurchase> {
        match self {
            Self::Picked { purchase, .. } | Self::Bound { purchase, .. } => Some(purchase),
            Self::Accepted { .. } => None,
        }
    }

    pub(crate) fn guardian_code(&self) -> Option<&GuardianCode> {
        match self {
            Self::Accepted { guardian_code, .. } => guardian_code.as_ref(),
            Self::Picked { .. } | Self::Bound { .. } => None,
        }
    }
}

/// A slot whose seat the MG accepted.
#[derive(Clone, Copy)]
pub(super) struct AcceptedSlot<'a> {
    pub(super) slot: &'a StoredSlot,
    pub(super) seat_id: &'a SeatId,
    pub(super) guardian_fee_account: &'a Account,
    pub(super) fedimintd_version: &'a FedimintdVersion,
    pub(super) guardian_code: Option<&'a GuardianCode>,
}

json_database_value!(StoredSlot);

impl FaStore {
    /// Record one accepted seat. Only its own slot changes, so concurrent
    /// acceptances never conflict. The purchase ends with its last empty
    /// slot.
    pub(crate) async fn record_seat_accepted(
        &self,
        formation_id: &FormationId,
        index: u16,
        seat_id: SeatId,
        guardian_fee_account: Account,
    ) -> FaResult<SlotState> {
        let formation_id = formation_id.clone();
        let store = self.clone();
        let result = self
            .database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let seat_id = seat_id.clone();
                    let guardian_fee_account = guardian_fee_account.clone();
                    let store = store.clone();
                    Box::pin(async move {
                        let mut creation = running_purchase(dbtx, &formation_id).await?;
                        let size = creation.stored.federation_size;
                        let slot = creation.slot_mut(index)?;
                        let accepted = match &slot.state {
                            SlotState::Bound { quote, .. } => {
                                let quote = quote.verify(&slot.locator.service_pubkey).map_err(
                                    |error| {
                                        FaError::Storage(format!(
                                            "invalid quote for FA slot {index}: {error}"
                                        ))
                                    },
                                )?;
                                if quote.terms.request.federation_size != size {
                                    return Err(FaError::Storage(format!(
                                        "FA slot {index} was quoted for another federation size"
                                    )));
                                }
                                SlotState::Accepted {
                                    seat_id,
                                    guardian_fee_account,
                                    fedimintd_version: quote
                                        .terms
                                        .request
                                        .fedimintd_version
                                        .clone(),
                                    guardian_code: None,
                                }
                            }
                            SlotState::Accepted {
                                seat_id: existing,
                                guardian_fee_account: existing_account,
                                ..
                            } if existing == &seat_id
                                && existing_account == &guardian_fee_account =>
                            {
                                return Ok(slot.state.clone());
                            }
                            SlotState::Picked { .. } | SlotState::Accepted { .. } => {
                                return Err(FaError::Storage(format!(
                                    "FA slot {index} changed before its seat was accepted"
                                )));
                            }
                        };
                        slot.state = accepted;
                        let slot = slot.clone();
                        validate_creation(&creation)?;
                        dbtx.insert_entry(&SlotKey { index }, &slot).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(slot.state)
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error);
        #[cfg(test)]
        if result.is_ok()
            && let Some(hook) = self
                .seat_checkpoint_hook
                .lock()
                .expect("test seat checkpoint hook lock")
                .clone()
        {
            hook(index);
        }
        result
    }

    /// Bind one slot to its quote before its payment or presentation can
    /// start. A paid slot moves to a new quote only after
    /// the wallet proved that no payment of its bound quote started.
    pub(crate) async fn bind_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        bound_quote: Option<&SignedResponse<GetQuoteResponse>>,
        signed_quote: SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let bound_quote = bound_quote.cloned();
        let store = self.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let bound_quote = bound_quote.clone();
                    let signed_quote = signed_quote.clone();
                    let store = store.clone();
                    Box::pin(async move {
                        let mut creation = running_purchase(dbtx, &formation_id).await?;
                        let slot = creation.slot_mut(index)?.clone();
                        let quote =
                            signed_quote
                                .verify(&slot.locator.service_pubkey)
                                .map_err(|error| {
                                    FaError::Storage(format!(
                                        "invalid quote for FA slot {index}: {error}"
                                    ))
                                })?;
                        let purchase = match (&slot.state, &bound_quote) {
                            (SlotState::Picked { purchase, .. }, None) => purchase.clone(),
                            // Only a paid quote moves, to a paid quote.
                            (
                                SlotState::Bound {
                                    quote: bound,
                                    purchase,
                                },
                                Some(expected),
                            ) if bound == expected => {
                                let bound_paid = bound
                                    .verify(&slot.locator.service_pubkey)
                                    .is_ok_and(|bound| bound.terms.payment.is_some());
                                if !bound_paid || quote.terms.payment.is_none() {
                                    return Err(FaError::Storage(format!(
                                        "FA slot {index} cannot move its bound quote"
                                    )));
                                }
                                purchase.clone()
                            }
                            _ => {
                                return Err(FaError::Storage(format!(
                                    "FA slot {index} changed before its quote was bound"
                                )));
                            }
                        };
                        let slot = StoredSlot {
                            state: SlotState::Bound {
                                quote: signed_quote,
                                purchase,
                            },
                            ..slot
                        };
                        dbtx.insert_entry(&SlotKey { index }, &slot).await;
                        dbtx.insert_entry(&CreationKey, &creation.stored).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Empty pending slots, each still in its expected state, and keep the
    /// highest-ranked reason a slot left for. A slot may go only when no payment of its
    /// bound quote is outstanding.
    pub(crate) async fn empty_slots(
        &self,
        formation_id: &FormationId,
        emptied: Vec<(u16, SlotState)>,
        reason: crate::SelectionReauthorizationReason,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let emptied = &emptied;
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let mut creation = running_purchase(dbtx, &formation_id).await?;
                        for (index, expected) in emptied {
                            if expected.purchase().is_none()
                                || creation.slot_mut(*index)?.state != *expected
                            {
                                return Err(FaError::Storage(format!(
                                    "FA slot {index} changed before it was emptied"
                                )));
                            }
                            dbtx.remove_entry(&SlotKey { index: *index }).await;
                        }
                        let shown = &mut creation.stored.reauthorization;
                        if shown.is_none_or(|shown| reason.rank() > shown.rank()) {
                            *shown = Some(reason);
                        }
                        dbtx.insert_entry(&CreationKey, &creation.stored).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Record one guardian code.
    pub(crate) async fn record_guardian_code(
        &self,
        formation_id: &FormationId,
        index: u16,
        guardian_code: GuardianCode,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let guardian_code = guardian_code.clone();
                    Box::pin(async move {
                        let (mut creation, _) = active_formation(dbtx, &formation_id).await?;
                        let slot = creation.slot_mut(index)?;
                        let SlotState::Accepted {
                            guardian_code: code,
                            ..
                        } = &mut slot.state
                        else {
                            return Err(FaError::Storage(format!(
                                "missing FA formation seat {index}"
                            )));
                        };
                        if code
                            .as_ref()
                            .is_some_and(|existing| existing != &guardian_code)
                        {
                            return Err(FaError::Storage(format!(
                                "FA formation seat {index} changed guardian code"
                            )));
                        }
                        *code = Some(guardian_code);
                        let slot = slot.clone();
                        dbtx.insert_entry(&SlotKey { index }, &slot).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }
}

/// The creation `formation_id` names, while its purchase runs.
pub(super) async fn running_purchase(
    dbtx: &mut DatabaseTransaction<'_>,
    formation_id: &FormationId,
) -> FaResult<Creation> {
    read_any_creation(dbtx)
        .await?
        .filter(|creation| {
            &creation.stored.formation_id == formation_id && creation.pending().next().is_some()
        })
        .ok_or_else(|| {
            FaError::Storage("the running FA seat purchase changed during an operation".to_owned())
        })
}
