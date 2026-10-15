use super::migration::{RestoredBackupKey, RestoredBackupState};
use super::*;
use fedimint_core::db::IDatabaseTransactionOpsCore as _;

// Captured from the schema-9 persisted JSON shape before durable Fedimint
// compatibility ranges and DKG cohort selection were added.
const SCHEMA_9_FORMATION: &[u8] = br#"{"schema_version":9,"fi_id":"02f66ab99ef6fb5f248cd0cdb7d2fcda9f12600be32b0bc13c8b556a4b6de8ff2e","formation_id":"legacy-schema-9","phase":"initialized","intent":{"federation_name":"Legacy Federation","federation_size":4,"plan":"infinite_best_effort","fedimintd_version":"0.11.1-fedi15"},"seat_count":0,"creation_mode":"pinned","payment_authorization":null,"payment_reservation_id":null,"payment_reservation_release_intended":false,"payment_authorization_recorded":false,"payment_outputs_started":false,"invite_code":null,"formation_meta_target":null}"#;

/// Raw storage captured from schema 11 code with the selected formation flow.
const SCHEMA_11_PURCHASE: &str = include_str!("fixtures/schema_11_purchase.json");
const SCHEMA_11_DKG: &str = include_str!("fixtures/schema_11_dkg.json");
const SCHEMA_11_FORMED: &str = include_str!("fixtures/schema_11_formed.json");

/// The schema 11 fixture states.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Schema11Fixture {
    /// Payment outputs started and three of seven seats exist.
    Purchase,
    /// Every seat exists and six of seven guardian codes arrived.
    Dkg,
    /// DKG produced the invite; formation metadata is not yet proposed.
    DkgComplete,
    /// The formation metadata target was saved and is being proposed.
    PublishingSeatBindings,
    /// Formed, with the federation record that #159 storage adds.
    Formed,
    /// Formed, as stored before the federation record existed.
    FormedWithoutFederationRecord,
}

/// The metadata consensus the formed schema 11 fixture confirmed.
pub(crate) fn schema_11_formed_meta_consensus_for_test() -> Vec<u8> {
    let rows: Vec<(String, serde_json::Value)> =
        serde_json::from_str(SCHEMA_11_FORMED).expect("schema 11 fixture parses");
    let target = &rows
        .iter()
        .find(|(key, _)| key == "00")
        .expect("fixture has a formation row")
        .1["formation_meta_target"];
    serde_json::to_vec(&serde_json::json!({
        fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY: target["seat_bindings"],
        "fedi:guardian_fee_send_ppm": target["send_ppm"].to_string(),
        "fedi:guardian_fee_remittance_account": target["recipients"],
    }))
    .expect("test meta object serializes")
}

impl FaStore {
    pub(crate) async fn install_schema_11_fixture_for_test(&self, fixture: Schema11Fixture) {
        let rows = match fixture {
            Schema11Fixture::Purchase => SCHEMA_11_PURCHASE,
            Schema11Fixture::Dkg => SCHEMA_11_DKG,
            Schema11Fixture::DkgComplete
            | Schema11Fixture::PublishingSeatBindings
            | Schema11Fixture::Formed
            | Schema11Fixture::FormedWithoutFederationRecord => SCHEMA_11_FORMED,
        };
        let mut rows: Vec<(String, serde_json::Value)> =
            serde_json::from_str(rows).expect("schema 11 fixture parses");
        // The earlier phases of the formed fixture differ only in its header.
        for (key, value) in &mut rows {
            if key != "00" {
                continue;
            }
            match fixture {
                Schema11Fixture::DkgComplete => {
                    value["phase"] = "dkg_complete".into();
                    value["formation_meta_target"] = serde_json::Value::Null;
                }
                Schema11Fixture::PublishingSeatBindings => {
                    value["phase"] = "publishing_seat_bindings".into();
                }
                _ => {}
            }
        }
        let mut dbtx = self.database.begin_transaction().await;
        for (key, value) in rows {
            let key = hex::decode(key).expect("fixture key is hex");
            if !matches!(fixture, Schema11Fixture::Formed) && key[0] == FaDbPrefix::Federation as u8
            {
                continue;
            }
            dbtx.raw_insert_bytes(&key, &serde_json::to_vec(&value).expect("fixture value"))
                .await
                .expect("test raw write succeeds");
        }
        dbtx.commit_tx().await;
    }

    /// Install the schema 11 purchase fixture with seat `index` ended after
    /// payment, as the removed guardian replacement flow stored it: waiting
    /// for a replacement, or with one applied to the MG in `replaced_by`.
    pub(crate) async fn install_schema_11_replacement_for_test(
        &self,
        index: u16,
        replaced_by: Option<(Locator, PublicKey)>,
    ) {
        let mut rows: Vec<(String, serde_json::Value)> =
            serde_json::from_str(SCHEMA_11_PURCHASE).expect("schema 11 fixture parses");
        for (key, value) in &mut rows {
            if key == "00" {
                // Marking a seat for replacement invalidated the aggregate.
                value["payment_authorization"] = serde_json::Value::Null;
                value["payment_reservation_id"] = serde_json::Value::Null;
            } else if key.starts_with("01") && value["index"] == index {
                let badge = value["admission"]["peer_badge"].clone();
                value["replacement_for"] = badge["state"]["effect_authorized"]["quote_id"].clone();
                value["replacement_previous_fman_id"] = badge["fman_id"].clone();
                value["replacement_previous_locator"] = value["locator"].clone();
                value["signed_quote"] = serde_json::Value::Null;
                if let Some((locator, mg_id)) = &replaced_by {
                    value["replacement_approved"] = true.into();
                    value["locator"] = serde_json::to_value(locator).expect("locator serializes");
                    let badge = &mut value["admission"]["peer_badge"];
                    badge["fman_id"] = mg_id.to_hex().into();
                    badge["holder"] = mg_id.to_hex().into();
                    badge["state"] = "fresh".into();
                }
            }
        }
        let mut dbtx = self.database.begin_transaction().await;
        for (key, value) in rows {
            let key = hex::decode(key).expect("fixture key is hex");
            dbtx.raw_insert_bytes(&key, &serde_json::to_vec(&value).expect("fixture value"))
                .await
                .expect("test raw write succeeds");
        }
        dbtx.commit_tx().await;
    }

    /// Install the schema 11 purchase fixture with seat `index` never bound
    /// to an effect.
    pub(crate) async fn install_schema_11_unbound_seat_for_test(&self, index: u16) {
        let mut rows: Vec<(String, serde_json::Value)> =
            serde_json::from_str(SCHEMA_11_PURCHASE).expect("schema 11 fixture parses");
        for (key, value) in &mut rows {
            if key.starts_with("01") && value["index"] == index {
                value["admission"]["peer_badge"]["state"] = "fresh".into();
            }
        }
        let mut dbtx = self.database.begin_transaction().await;
        for (key, value) in rows {
            let key = hex::decode(key).expect("fixture key is hex");
            dbtx.raw_insert_bytes(&key, &serde_json::to_vec(&value).expect("fixture value"))
                .await
                .expect("test raw write succeeds");
        }
        dbtx.commit_tx().await;
    }

    pub(crate) async fn install_schema_9_fixture_for_test(&self) {
        self.install_raw_formation_for_test(SCHEMA_9_FORMATION)
            .await;
    }

    pub(crate) async fn install_raw_formation_for_test(&self, bytes: &[u8]) {
        let mut dbtx = self.database.begin_transaction().await;
        dbtx.raw_insert_bytes(&[FaDbPrefix::LegacyFormation as u8], bytes)
            .await
            .expect("test raw write succeeds");
        dbtx.raw_insert_bytes(&[FaDbPrefix::LegacySeat as u8, 0xff], b"sentinel")
            .await
            .expect("test raw write succeeds");
        dbtx.commit_tx().await;
    }

    pub(crate) async fn raw_namespace_is_empty_for_test(&self) -> bool {
        self.database
            .begin_transaction_nc()
            .await
            .raw_find_by_prefix(&[])
            .await
            .expect("test raw scan succeeds")
            .next()
            .await
            .is_none()
    }

    /// Whether any legacy formation, creation or slot row remains.
    pub(crate) async fn holds_creation_rows_for_test(&self) -> bool {
        let mut dbtx = self.database.begin_transaction_nc().await;
        for prefix in [
            FaDbPrefix::LegacyFormation,
            FaDbPrefix::LegacySeat,
            FaDbPrefix::Slot,
            FaDbPrefix::Creation,
        ] {
            if dbtx
                .raw_find_by_prefix(&[prefix as u8])
                .await
                .expect("test raw scan succeeds")
                .next()
                .await
                .is_some()
            {
                return true;
            }
        }
        false
    }

    pub(crate) async fn set_schema_version_for_test(&self, schema_version: u16) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut creation = dbtx
            .get_value(&CreationKey)
            .await
            .expect("test creation exists");
        creation.schema_version = schema_version;
        dbtx.insert_entry(&CreationKey, &creation).await;
        dbtx.commit_tx().await;
    }

    /// Rewrite the federation record into the storage that predates it. A
    /// reconciled import was a restored-backup row marked eligible, and this
    /// one has not published a backup yet.
    pub(crate) async fn write_legacy_federation_storage_for_test(
        &self,
        imported: Option<crate::backup::FaBackupPayload>,
    ) {
        let mut dbtx = self.database.begin_transaction().await;
        let federation = dbtx
            .get_value(&FederationKey)
            .await
            .expect("test federation exists");
        dbtx.remove_entry(&FederationKey).await;
        if let Some(payload) = imported {
            dbtx.remove_entry(&BackupGenerationKey).await;
            dbtx.insert_entry(
                &RestoredBackupKey,
                &RestoredBackupState {
                    fa_id: federation.fa_id,
                    payload,
                    formation_id: federation.formation_id,
                    backup_eligible: true,
                    federation_name: federation.federation_name,
                },
            )
            .await;
        }
        dbtx.commit_tx().await;
    }

    /// Write an import that waits for reconciliation, as schema 11 did.
    pub(crate) async fn write_pending_import_for_test(
        &self,
        fa_id: FaId,
        payload: crate::backup::FaBackupPayload,
    ) {
        let formation_id =
            crate::formation::restored_formation_id(fa_id, &payload.federation_invite).unwrap();
        let mut dbtx = self.database.begin_transaction().await;
        dbtx.insert_entry(
            &RestoredBackupKey,
            &RestoredBackupState {
                fa_id,
                payload,
                formation_id,
                backup_eligible: false,
                federation_name: None,
            },
        )
        .await;
        dbtx.commit_tx().await;
    }
}
