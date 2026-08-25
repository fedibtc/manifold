use super::*;
use fedimint_core::db::IDatabaseTransactionOpsCore as _;

impl FiStore {
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

    pub(crate) async fn install_schema_9_fixture_for_test(&self) {
        self.mutate_active_formation_json_for_test(|formation| {
            formation.insert("schema_version".to_owned(), serde_json::json!(9));
            formation.remove("dkg_completion_callback");
        })
        .await;
    }

    pub(crate) async fn stored_schema_and_callback_field_for_test(&self) -> (u64, bool) {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let bytes = dbtx
            .raw_get_bytes(
                &<ActiveFormationKey as fedimint_core::db::DatabaseKeyPrefix>::to_bytes(
                    &ActiveFormationKey,
                ),
            )
            .await
            .expect("test raw read succeeds")
            .expect("test formation exists");
        let formation: serde_json::Value =
            serde_json::from_slice(&bytes).expect("stored formation is JSON");
        (
            formation["schema_version"]
                .as_u64()
                .expect("schema version is numeric"),
            formation
                .as_object()
                .expect("stored formation is an object")
                .contains_key("dkg_completion_callback"),
        )
    }
}
