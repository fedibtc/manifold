//! The formation row: the invite and metadata target of a full pool after
//! DKG completed, until the federation forms.

use super::*;

/// The formation of a full pool after its DKG completed. It ends with the
/// creation.
#[derive(Debug, Decodable, Encodable)]
pub(super) struct FormationKey;

impl_db_record!(
    key = FormationKey,
    value = StoredFormation,
    db_prefix = FaDbPrefix::Formation,
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct StoredFormation {
    /// Federation invite the MGs agreed on when DKG completed.
    pub(super) invite_code: InviteCode,
    /// Exact formation metadata target, persisted before its first vote.
    pub(super) meta_target: Option<FormationMetaTarget>,
}

/// Complete semantic target replayed until formation metadata is confirmed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormationMetaTarget {
    /// FA's canonical prediction, used only for consensus readback.
    pub(crate) seat_bindings: String,
    /// Exact structural request payload replayed until readback succeeds.
    pub(crate) binding_entries: Vec<FormationSeatBinding>,
    #[serde(rename = "fi_fee_account")]
    pub(crate) fa_fee_account: GuardianFeeAccount,
    pub(crate) guardian_verification_fee_account: GuardianFeeAccount,
    pub(crate) send_ppm: u64,
    pub(crate) recipients: String,
}

json_database_value!(StoredFormation);

impl FaStore {
    /// Change the formation progress in one transaction.
    async fn update_formation(
        &self,
        formation_id: &FormationId,
        update: impl Fn(&mut Creation) -> FaResult<()> + Send + Sync,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let update = &update;
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let (mut creation, _) = active_formation(dbtx, &formation_id).await?;
                        update(&mut creation)?;
                        active_formation_seats(&creation)?;
                        dbtx.insert_entry(&CreationKey, &creation.stored).await;
                        if let Some(formation) = &creation.formation {
                            dbtx.insert_entry(&FormationKey, formation).await;
                        }
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Save completed DKG and its invite.
    pub(crate) async fn record_dkg_complete(
        &self,
        formation_id: &FormationId,
        invite_code: InviteCode,
    ) -> FaResult<()> {
        self.update_formation(formation_id, |creation| {
            if creation.formation.is_some() {
                return Err(FaError::Storage(
                    "the FA formation already completed DKG".to_owned(),
                ));
            }
            creation.formation = Some(StoredFormation {
                invite_code: invite_code.clone(),
                meta_target: None,
            });
            // Every MG now owns durable delivery retry. Drop the FA-side
            // bearer with the DKG checkpoint, retaining it through every
            // earlier crash and restart.
            creation.stored.dkg_completion_callback = None;
            Ok(())
        })
        .await
    }

    /// Persist the complete formation metadata target before it is submitted.
    pub(crate) async fn record_formation_meta_target(
        &self,
        formation_id: &FormationId,
        target: FormationMetaTarget,
    ) -> FaResult<()> {
        self.update_formation(formation_id, |creation| {
            let formation = creation.formation.as_mut().ok_or_else(|| {
                FaError::Storage(
                    "the FA formation has no invite for its metadata target".to_owned(),
                )
            })?;
            formation.meta_target = Some(target.clone());
            Ok(())
        })
        .await
    }

    /// Checkpoint exact consensus readback of the persisted formation target:
    /// record the federation it formed, and end the creation and its slots.
    pub(crate) async fn finish_formation(&self, formation_id: &FormationId) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let (creation, seats) = active_formation(dbtx, &formation_id).await?;
                        let Some(StoredFormation {
                            invite_code,
                            meta_target: Some(_),
                        }) = &creation.formation
                        else {
                            return Err(FaError::Storage(
                                "cannot complete formation before publishing metadata".to_owned(),
                            ));
                        };
                        insert_formed_federation(dbtx, &creation.stored, invite_code, &seats).await;
                        // Ended slots go with the creation.
                        for (index, _) in &creation.slots {
                            dbtx.remove_entry(&SlotKey { index: *index }).await;
                        }
                        dbtx.remove_entry(&FormationKey).await;
                        dbtx.remove_entry(&CreationKey).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }
}

/// One accepted seat of the formation and its cached guardian code.
pub(super) struct FormationSeat {
    pub(super) index: u16,
    pub(super) mg_id: PublicKey,
    pub(super) locator: Locator,
    pub(super) seat_id: SeatId,
    pub(super) guardian_code: Option<GuardianCode>,
}

/// The seats of the creation's formation in slot order, with their cached
/// guardian codes. The first seat's code carries the federation name. The
/// formation runs once every slot holds an accepted seat.
pub(super) fn formation_seats(creation: &Creation) -> FaResult<Option<Vec<FormationSeat>>> {
    let accepted = creation.accepted().collect::<Vec<_>>();
    if accepted.len() < usize::from(creation.stored.federation_size.0) {
        if creation.formation.is_some() {
            return Err(FaError::Storage(
                "persisted FA formation lacks accepted seats".to_owned(),
            ));
        }
        return Ok(None);
    }
    let mut seats = Vec::with_capacity(accepted.len());
    for (index, seat) in accepted {
        seats.push(FormationSeat {
            index,
            mg_id: seat.slot.mg_id,
            locator: seat.slot.locator.clone(),
            seat_id: seat.seat_id.clone(),
            guardian_code: seat.slot.state.guardian_code().cloned(),
        });
    }
    validate_formation(creation, &seats)?;
    Ok(Some(seats))
}

/// The seats of the running formation.
pub(super) fn active_formation_seats(creation: &Creation) -> FaResult<Vec<FormationSeat>> {
    formation_seats(creation)?
        .ok_or_else(|| FaError::Storage("the FA formation is missing".to_owned()))
}

/// The creation whose formation `formation_id` names, and its seats.
pub(super) async fn active_formation(
    dbtx: &mut DatabaseTransaction<'_>,
    formation_id: &FormationId,
) -> FaResult<(Creation, Vec<FormationSeat>)> {
    let creation = read_any_creation(dbtx)
        .await?
        .filter(|creation| &creation.stored.formation_id == formation_id)
        .ok_or_else(|| {
            FaError::Storage("active FA formation changed during an operation".to_owned())
        })?;
    let seats = active_formation_seats(&creation)?;
    Ok((creation, seats))
}

/// Reject formation progress that disagrees with its seats.
pub(super) fn validate_formation(creation: &Creation, seats: &[FormationSeat]) -> FaResult<()> {
    if !(crate::MIN_FEDERATION_SIZE..crate::MAX_FEDERATION_SIZE_EXCLUSIVE)
        .contains(&creation.stored.federation_size.0)
    {
        return Err(FaError::Storage(
            "persisted FA formation has an unsupported seat count".to_owned(),
        ));
    }
    if creation.formation.is_some()
        && let Some(seat) = seats.iter().find(|seat| seat.guardian_code.is_none())
    {
        return Err(FaError::Storage(format!(
            "persisted formed FA formation lacks guardian recovery facts for slot {}",
            seat.index
        )));
    }
    Ok(())
}
