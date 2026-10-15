//! Storage written by older releases, and its migration to the current
//! schema. Opening an FA runs this once; nothing else reads old formats.

use super::*;

/// Schemas before 11 predate the production compatibility baseline, and
/// opening such an FA resets its storage. Keep this fixed when advancing the
/// schema written by current code.
const RESET_BEFORE_SCHEMA_VERSION: u16 = 11;

#[derive(Debug, Decodable, Encodable)]
pub(super) struct RestoredBackupKey;
impl_db_record!(
    key = RestoredBackupKey,
    value = RestoredBackupState,
    db_prefix = FaDbPrefix::RestoredBackup
);

/// Legacy: an authenticated backup imported but not yet reconciled against
/// consensus. `backup_eligible` marks one that was reconciled before the
/// federation record existed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct RestoredBackupState {
    #[serde(rename = "fi_id")]
    pub(super) fa_id: FaId,
    pub(super) payload: crate::backup::FaBackupPayload,
    pub(super) formation_id: FormationId,
    pub(super) backup_eligible: bool,
    pub(super) federation_name: Option<crate::FederationName>,
}

json_database_value!(RestoredBackupState);

/// The schema 11 formation row: one federation creation and its purchase.
#[derive(serde::Deserialize)]
struct Schema11Formation {
    schema_version: u16,
    #[serde(rename = "fi_id")]
    fa_id: FaId,
    formation_id: FormationId,
    phase: Schema11Phase,
    intent: Schema11Intent,
    seat_count: u16,
    creation_mode: serde_json::Value,
    #[serde(default)]
    invite_code: Option<InviteCode>,
    #[serde(default)]
    formation_meta_target: Option<FormationMetaTarget>,
    #[serde(default)]
    dkg_completion_callback: Option<DkgCompletionCallback>,
}

#[derive(serde::Deserialize)]
struct Schema11Intent {
    federation_name: FederationName,
    federation_size: FederationSize,
    plan: crate::PlanPreference,
    fedimintd_versions: crate::FedimintdVersionRange,
    fedimintd_dkg_version: FedimintdDkgVersion,
}

#[derive(PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Schema11Phase {
    Initialized,
    SeatsPartiallyCreated,
    SeatsCreated,
    DkgComplete,
    PublishingSeatBindings,
    Formed,
}

/// The key of a schema 11 seat row.
#[derive(Debug, Decodable, Encodable)]
struct Schema11SeatKey {
    formation_id: String,
    index: u16,
}

impl Schema11SeatKey {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![FaDbPrefix::LegacySeat as u8];
        bytes.extend(self.consensus_encode_to_vec());
        bytes
    }
}

/// One schema 11 seat row.
#[derive(serde::Deserialize)]
struct Schema11Seat {
    index: u16,
    locator: Locator,
    admission: Schema11Admission,
    signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    seat_id: Option<SeatId>,
    #[serde(default)]
    guardian_fee_account: Option<Account>,
    #[serde(default)]
    guardian_code: Option<GuardianCode>,
    /// Set while the seat waited for a guardian replacement.
    #[serde(default)]
    replacement_for: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Schema11Admission {
    PeerBadge {
        #[serde(rename = "fman_id")]
        mg_id: PublicKey,
        #[serde(default)]
        holder: Option<PublicKey>,
        /// `"fresh"` until the seat's quote was bound to its payment or
        /// presentation.
        state: serde_json::Value,
    },
}

impl FaStore {
    /// Bring storage written by older releases to the current schema.
    pub(crate) async fn migrate_storage(&self) -> FaResult<()> {
        self.reset_before_schema_11().await?;
        self.split_schema_11_formation().await?;
        self.adopt_schema_11_restored_backup().await?;
        self.remove_driver_lease().await
    }

    /// Schema 11 kept a database-wide driver lease. One client per database
    /// owns all work now, so a remaining lease row is stale.
    async fn remove_driver_lease(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let key = [FaDbPrefix::DriverLease as u8];
        if dbtx
            .raw_get_bytes(&key)
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
            .is_none()
        {
            return Ok(());
        }
        dbtx.raw_remove_entry(&key)
            .await
            .map_err(|_| FaError::Storage("removing the FA driver lease failed".to_owned()))?;
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("committing the FA driver lease removal failed".to_owned())
        })
    }

    async fn reset_before_schema_11(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(bytes) = dbtx
            .raw_get_bytes(&[FaDbPrefix::LegacyFormation as u8])
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        else {
            return Ok(());
        };
        let envelope: StoredCreationEnvelope = serde_json::from_slice(&bytes).map_err(|_| {
            FaError::Storage("persisted FA purchase header is malformed".to_owned())
        })?;
        if envelope.schema_version >= RESET_BEFORE_SCHEMA_VERSION {
            return Ok(());
        }
        dbtx.raw_remove_by_prefix(&[])
            .await
            .map_err(|_| FaError::Storage("resetting legacy FA storage failed".to_owned()))?;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("committing legacy FA reset failed".to_owned()))
    }

    /// Schema 11 kept a federation creation, its purchase and its formation
    /// in one row, with one row per seat. Each seat becomes a slot at its
    /// index, so the seat at index 0 still carries the federation name. A
    /// formed federation keeps only its federation record.
    async fn split_schema_11_formation(&self) -> FaResult<()> {
        let malformed =
            || FaError::Storage("persisted schema 11 FA formation is malformed".to_owned());
        let mut dbtx = self.database.begin_transaction().await;
        let Some(bytes) = dbtx
            .raw_get_bytes(&[FaDbPrefix::LegacyFormation as u8])
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        else {
            return Ok(());
        };
        let legacy: Schema11Formation = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if legacy.schema_version != 11 {
            return Err(malformed());
        }
        // Pinned formations were a development path with no authenticated
        // MG identities.
        let payment_federation_id = match &legacy.creation_mode {
            serde_json::Value::Object(mode) if mode.contains_key("selected") => mode["selected"]
                .get("payment_federation_id")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| malformed())?
                .flatten(),
            serde_json::Value::String(mode) if mode == "pinned" => {
                return Err(FaError::Storage(
                    "a schema 11 pinned FA formation cannot be migrated; reset this FA namespace"
                        .to_owned(),
                ));
            }
            _ => return Err(malformed()),
        };
        let mut reauthorization = None;
        let mut slots = Vec::with_capacity(usize::from(legacy.seat_count));
        let mut guardian_codes = Vec::new();
        for index in 0..legacy.seat_count {
            let key = Schema11SeatKey {
                formation_id: legacy.formation_id.0.clone(),
                index,
            };
            let bytes = dbtx
                .raw_get_bytes(&key.to_bytes())
                .await
                .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
                .ok_or_else(|| {
                    FaError::Storage(format!("missing persisted FA seat row {index}"))
                })?;
            let seat: Schema11Seat = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
            if seat.index != index {
                return Err(malformed());
            }
            // Schema 12 has no guardian replacement. A seat that waited for
            // one ended after payment, and an applied replacement has no
            // effect yet, so both leave.
            if seat
                .replacement_for
                .as_ref()
                .is_some_and(|replacement| !replacement.is_null())
            {
                continue;
            }
            let Schema11Admission::PeerBadge {
                mg_id,
                holder,
                state,
            } = seat.admission;
            let state = match (seat.seat_id, seat.signed_quote) {
                (Some(seat_id), Some(quote)) => {
                    let quote = quote
                        .verify(&seat.locator.service_pubkey)
                        .map_err(|_| malformed())?;
                    if let Some(code) = seat.guardian_code {
                        guardian_codes.push((seat_id.clone(), code));
                    }
                    SlotState::Accepted {
                        seat_id,
                        guardian_fee_account: seat.guardian_fee_account.ok_or_else(malformed)?,
                        fedimintd_version: quote.terms.request.fedimintd_version.clone(),
                    }
                }
                (Some(_), None) => return Err(malformed()),
                // A seat whose quote was bound may hold a started payment.
                (None, Some(quote)) if state != "fresh" => SlotState::Bound { quote },
                // Schema 11 did not record the previewed plan, so an unbound
                // seat has no approved price to request.
                (None, _) => {
                    reauthorization
                        .get_or_insert(crate::SelectionReauthorizationReason::SeatUnavailable);
                    continue;
                }
            };
            slots.push((
                index,
                StoredSlot {
                    locator: seat.locator,
                    mg_id,
                    holder,
                    state,
                },
            ));
        }
        let mut creation = Creation {
            stored: StoredCreation {
                schema_version: STORAGE_SCHEMA_VERSION,
                fa_id: legacy.fa_id,
                federation_name: legacy.intent.federation_name,
                federation_size: legacy.intent.federation_size,
                fedimintd_versions: legacy.intent.fedimintd_versions,
                plan: legacy.intent.plan,
                dkg_completion_callback: legacy.dkg_completion_callback,
                formation_id: legacy.formation_id.clone(),
                invite_code: legacy.invite_code,
                formation_meta_target: legacy.formation_meta_target,
                reauthorization,
                purchase: None,
            },
            slots,
        };
        // The kept unaccepted seats continue the purchase.
        if creation.pending().next().is_some() {
            creation.stored.purchase = Some(StoredPurchase {
                fedimintd_dkg_version: legacy.intent.fedimintd_dkg_version,
                payment_federation_id,
            });
        }
        validate_creation(&creation)?;
        dbtx.raw_remove_entry(&[FaDbPrefix::LegacyFormation as u8])
            .await
            .map_err(|_| FaError::Storage("removing a schema 11 formation failed".to_owned()))?;
        dbtx.raw_remove_by_prefix(&[FaDbPrefix::LegacySeat as u8])
            .await
            .map_err(|_| FaError::Storage("removing schema 11 seats failed".to_owned()))?;
        if legacy.phase == Schema11Phase::Formed {
            // A formed federation keeps only its federation record.
            let mut seats = Vec::with_capacity(creation.slots.len());
            for (index, slot) in &creation.slots {
                let accepted = slot.accepted().ok_or_else(malformed)?;
                seats.push(FormationSeat {
                    index: *index,
                    mg_id: slot.mg_id,
                    locator: slot.locator.clone(),
                    seat_id: accepted.seat_id.clone(),
                    guardian_code: None,
                });
            }
            if seats.len() != usize::from(creation.stored.federation_size.0) {
                return Err(malformed());
            }
            if dbtx.get_value(&FederationKey).await.is_none() {
                insert_formed_federation(&mut dbtx.to_ref_nc(), &creation.stored, &seats).await?;
            }
        } else {
            for (seat_id, code) in guardian_codes {
                dbtx.insert_new_entry(
                    &GuardianCodeKey {
                        seat_id: seat_id.to_string(),
                    },
                    &StoredGuardianCode(code),
                )
                .await;
            }
            for (index, slot) in &creation.slots {
                dbtx.insert_new_entry(&SlotKey { index: *index }, slot)
                    .await;
            }
            dbtx.insert_new_entry(&CreationKey, &creation.stored).await;
            // Reject cached codes that disagree with the formation.
            formation_seats(&mut dbtx.to_ref_nc(), &creation).await?;
        }
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("migrating schema 11 FA formation failed".to_owned()))
    }

    /// Schema 11 kept an imported backup in its own row until consensus
    /// verified it.
    async fn adopt_schema_11_restored_backup(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(restored) = dbtx.get_value(&RestoredBackupKey).await else {
            return Ok(());
        };
        dbtx.remove_entry(&RestoredBackupKey).await;
        // A reconciled legacy import becomes the record. An unreconciled one
        // is dropped, and the next restored open looks it up again.
        if restored.backup_eligible {
            adopt_backup(
                &mut dbtx.to_ref_nc(),
                restored.fa_id,
                restored.formation_id,
                restored.payload,
                restored.federation_name,
            )
            .await?;
        } else {
            dbtx.remove_by_prefix(&RecoveryCompletedKeyPrefix).await;
        }
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("migrating restored FA backup failed".to_owned()))
    }
}
