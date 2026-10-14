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
            Schema11Fixture::Formed | Schema11Fixture::FormedWithoutFederationRecord => {
                SCHEMA_11_FORMED
            }
        };
        let rows: Vec<(String, serde_json::Value)> =
            serde_json::from_str(rows).expect("schema 11 fixture parses");
        let mut dbtx = self.database.begin_transaction().await;
        for (key, value) in rows {
            let key = hex::decode(key).expect("fixture key is hex");
            if matches!(fixture, Schema11Fixture::FormedWithoutFederationRecord)
                && key[0] == FaDbPrefix::Federation as u8
            {
                continue;
            }
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
        dbtx.raw_insert_bytes(&DatabaseKeyPrefix::to_bytes(&PurchaseKey), bytes)
            .await
            .expect("test raw write succeeds");
        dbtx.raw_insert_bytes(&[FaDbPrefix::PurchaseSeat as u8, 0xff], b"sentinel")
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

    pub(crate) async fn set_schema_version_for_test(&self, schema_version: u16) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&PurchaseKey)
            .await
            .expect("test formation exists");
        formation.schema_version = schema_version;
        dbtx.insert_entry(&PurchaseKey, &formation).await;
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
}
