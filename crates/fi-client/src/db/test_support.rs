use super::*;
use fedimint_core::db::IDatabaseTransactionOpsCore as _;

// Captured from the schema-9 persisted JSON shape before durable Fedimint
// compatibility ranges and DKG cohort selection were added.
const SCHEMA_9_FORMATION: &[u8] = br#"{"schema_version":9,"fi_id":"02f66ab99ef6fb5f248cd0cdb7d2fcda9f12600be32b0bc13c8b556a4b6de8ff2e","formation_id":"legacy-schema-9","phase":"initialized","intent":{"federation_name":"Legacy Federation","federation_size":4,"plan":"infinite_best_effort","fedimintd_version":"0.11.1-fedi15"},"seat_count":0,"creation_mode":"pinned","payment_authorization":null,"payment_reservation_id":null,"payment_reservation_release_intended":false,"payment_authorization_recorded":false,"payment_outputs_started":false,"invite_code":null,"formation_meta_target":null}"#;

impl FiStore {
    pub(crate) async fn install_schema_9_fixture_for_test(&self) {
        self.install_raw_formation_for_test(SCHEMA_9_FORMATION)
            .await;
    }

    pub(crate) async fn install_raw_formation_for_test(&self, bytes: &[u8]) {
        let mut dbtx = self.database.begin_transaction().await;
        dbtx.raw_insert_bytes(&DatabaseKeyPrefix::to_bytes(&ActiveFormationKey), bytes)
            .await
            .expect("test raw write succeeds");
        dbtx.raw_insert_bytes(&[FiDbPrefix::Seat as u8, 0xff], b"sentinel")
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

    async fn mutate_active_formation_json_for_test(
        &self,
        mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) {
        let mut dbtx = self.database.begin_transaction().await;
        let key = <ActiveFormationKey as fedimint_core::db::DatabaseKeyPrefix>::to_bytes(
            &ActiveFormationKey,
        );
        let bytes = dbtx
            .raw_get_bytes(&key)
            .await
            .expect("test raw read succeeds")
            .expect("test formation exists");
        let mut formation: serde_json::Value =
            serde_json::from_slice(&bytes).expect("stored formation is JSON");
        mutate(
            formation
                .as_object_mut()
                .expect("stored formation is an object"),
        );
        dbtx.raw_insert_bytes(
            &key,
            &serde_json::to_vec(&formation).expect("test fixture serializes"),
        )
        .await
        .expect("test raw write succeeds");
        dbtx.commit_tx().await;
    }

    pub(crate) async fn remove_callback_field_for_test(&self) {
        self.mutate_active_formation_json_for_test(|formation| {
            formation.remove("dkg_completion_callback");
        })
        .await;
    }

    pub(crate) async fn set_schema_version_for_test(&self, schema_version: u16) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        formation.schema_version = schema_version;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
        dbtx.commit_tx().await;
    }
}
