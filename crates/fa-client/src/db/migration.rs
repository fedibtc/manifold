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

/// The formation fields of a schema 11 formation row that schema 12 moved
/// out of the purchase.
#[derive(serde::Deserialize)]
struct Schema11Formation {
    phase: Schema11Phase,
    intent: Schema11Intent,
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

impl FaStore {
    /// Bring storage written by older releases to the current schema.
    pub(crate) async fn migrate_storage(&self) -> FaResult<()> {
        self.reset_before_schema_11().await?;
        self.split_schema_11_formation().await?;
        self.adopt_schema_11_restored_backup().await
    }

    async fn reset_before_schema_11(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(bytes) = dbtx
            .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        else {
            return Ok(());
        };
        let envelope: StoredPurchaseEnvelope = serde_json::from_slice(&bytes).map_err(|_| {
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

    /// Schema 11 kept a formation and its seat purchase in one row. A
    /// purchase that has not created every seat stays a purchase. From
    /// `seats_created` on, the seats move into the pool and the formation
    /// progress into the creation record.
    async fn split_schema_11_formation(&self) -> FaResult<()> {
        let malformed =
            || FaError::Storage("persisted schema 11 FA formation is malformed".to_owned());
        let mut dbtx = self.database.begin_transaction().await;
        let Some(bytes) = dbtx
            .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        else {
            return Ok(());
        };
        let mut header: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if header["schema_version"] != 11 {
            return Ok(());
        }
        let legacy: Schema11Formation =
            serde_json::from_value(header.clone()).map_err(|_| malformed())?;
        let object = header.as_object_mut().ok_or_else(malformed)?;
        for field in [
            "invite_code",
            "formation_meta_target",
            "dkg_completion_callback",
        ] {
            object.remove(field);
        }
        object.insert("schema_version".to_owned(), STORAGE_SCHEMA_VERSION.into());
        // Schema 12 keeps only the selected payer. Pinned formations were a
        // development path with no authenticated MG identities.
        let payment_federation_id = match object.remove("creation_mode") {
            Some(serde_json::Value::Object(mut mode)) => mode
                .remove("selected")
                .ok_or_else(|| {
                    FaError::Storage(
                        "a schema 11 pinned FA formation cannot be migrated; reset this FA \
                         namespace"
                            .to_owned(),
                    )
                })?
                .get_mut("payment_federation_id")
                .map(serde_json::Value::take),
            Some(serde_json::Value::String(mode)) if mode == "pinned" => {
                return Err(FaError::Storage(
                    "a schema 11 pinned FA formation cannot be migrated; reset this FA namespace"
                        .to_owned(),
                ));
            }
            _ => return Err(malformed()),
        };
        if let Some(payment_federation_id) = payment_federation_id {
            object.insert("payment_federation_id".to_owned(), payment_federation_id);
        }
        let in_purchase = matches!(
            legacy.phase,
            Schema11Phase::Initialized | Schema11Phase::SeatsPartiallyCreated
        );
        if !in_purchase {
            object.insert("phase".to_owned(), "seats_created".into());
        }
        let mut purchase: StoredPurchase =
            serde_json::from_value(header).map_err(|_| malformed())?;
        let mut seats = Vec::with_capacity(usize::from(purchase.seat_count));
        for index in 0..purchase.seat_count {
            let key = PurchaseSeatKey {
                formation_id: purchase.formation_id.0.clone(),
                index,
            };
            let bytes = dbtx
                .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&key))
                .await
                .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
                .ok_or_else(|| {
                    FaError::Storage(format!("missing persisted FA seat row {index}"))
                })?;
            let mut seat: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| malformed())?;
            let seat_object = seat.as_object_mut().ok_or_else(malformed)?;
            let admission = seat_object
                .get_mut("admission")
                .and_then(|admission| admission.get_mut("peer_badge"))
                .ok_or_else(malformed)?
                .take();
            seat_object.insert("admission".to_owned(), admission);
            let guardian_code: Option<GuardianCode> = seat_object
                .remove("guardian_code")
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| malformed())?
                .flatten();
            // Schema 12 has no guardian replacement. A seat that waited for
            // one ended after payment, so it leaves the purchase. An applied
            // replacement has no effect yet, so it leaves too. While the
            // wallet reservation may hold its payment, it stays until the run
            // releases that reservation.
            let replacement = !seat_object
                .get("replacement_for")
                .unwrap_or(&serde_json::Value::Null)
                .is_null();
            let applied =
                seat_object.get("replacement_approved") == Some(&serde_json::Value::Bool(true));
            let seat: StoredPurchaseSeat = serde_json::from_value(seat).map_err(|_| malformed())?;
            let reserved = applied
                && purchase.payment_reservation_id.is_some()
                && purchase
                    .payment_authorization
                    .as_ref()
                    .is_some_and(|authorization| {
                        authorization
                            .quotes
                            .iter()
                            .any(|quote| quote.index == seat.index)
                    });
            if reserved {
                purchase.payment_reservation_release_intended = true;
            } else if applied && purchase.payment_reservation_id.is_none() {
                purchase.payment_authorization = None;
            }
            if replacement && !reserved {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                continue;
            }
            seats.push((key, seat, guardian_code));
        }
        // Schema 12 keeps the user's creation inputs and formation progress
        // in their own record until the federation forms.
        let creation = StoredCreation {
            fa_id: purchase.fa_id.ok_or_else(malformed)?,
            federation_name: legacy.intent.federation_name,
            federation_size: purchase.intent.federation_size,
            fedimintd_versions: purchase.intent.fedimintd_versions.clone(),
            plan: purchase.intent.plan,
            dkg_completion_callback: legacy.dkg_completion_callback,
            formation_id: purchase.formation_id.clone(),
            invite_code: legacy.invite_code,
            formation_meta_target: legacy.formation_meta_target,
            // Schema 11 named the guardian code of the seat at index 0.
            named_seat: if in_purchase {
                None
            } else {
                seats.first().and_then(|(_, seat, _)| seat.seat_id.clone())
            },
        };
        if in_purchase {
            purchase.seat_count =
                u16::try_from(seats.len()).expect("kept seats fit the stored count");
            purchase.phase = purchase_phase(
                &seats
                    .iter()
                    .map(|(_, seat, _)| seat.clone())
                    .collect::<Vec<_>>(),
            );
            // The schema 11 rows no longer decode, so they are replaced
            // without decoding.
            dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
                .await
                .map_err(|_| FaError::Storage("removing a schema 11 purchase failed".to_owned()))?;
            dbtx.insert_new_entry(&PurchaseKey, &purchase).await;
            for (key, seat, _) in seats {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                dbtx.insert_new_entry(&key, &seat).await;
            }
            dbtx.insert_new_entry(&CreationKey, &creation).await;
        } else {
            let mut formation_seats = Vec::with_capacity(seats.len());
            for (key, seat, guardian_code) in seats {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                formation_seats.push((pool_seat(seat)?, guardian_code));
            }
            // The schema 11 row may hold a phase the purchase type cannot
            // decode, so it is removed without decoding.
            dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
                .await
                .map_err(|_| {
                    FaError::Storage("removing a schema 11 formation failed".to_owned())
                })?;
            validate_formation(&creation, &formation_seats)?;
            if legacy.phase == Schema11Phase::Formed {
                // A formed federation keeps only its federation record.
                if dbtx.get_value(&FederationKey).await.is_none() {
                    insert_formed_federation(&mut dbtx.to_ref_nc(), &creation, &formation_seats)
                        .await?;
                }
            } else {
                for (seat, guardian_code) in formation_seats {
                    let seat_id = seat.seat_id.to_string();
                    if let Some(guardian_code) = guardian_code {
                        dbtx.insert_new_entry(
                            &GuardianCodeKey {
                                seat_id: seat_id.clone(),
                            },
                            &StoredGuardianCode(guardian_code),
                        )
                        .await;
                    }
                    dbtx.insert_new_entry(&PoolSeatKey { seat_id }, &seat).await;
                }
                dbtx.insert_new_entry(&CreationKey, &creation).await;
            }
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
