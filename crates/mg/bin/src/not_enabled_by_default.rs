//! Module initializers that the bundled `fedimintd` registers as available
//! without enabling them in newly formed federations by default.

use std::collections::{BTreeMap, BTreeSet};

use fedimint_core::core::{Decoder, ModuleInstanceId, ModuleKind};
use fedimint_core::db::{DatabaseTransaction, DatabaseVersion};
use fedimint_core::module::{DynCommonModuleInit, IDynCommonModuleInit};
use fedimint_core::{NumPeers, PeerId};
use fedimint_server_core::bitcoin_rpc::ServerBitcoinRpcMonitor;
use fedimint_server_core::config::PeerHandleOps;
use fedimint_server_core::migration::DynServerDbMigrationFn;
use fedimint_server_core::{
    ConfigGenModuleArgs, DynServerModule, EnvVarDoc, IServerModuleInit, ServerModuleInit,
};

/// A server module initializer that stays out of fedimintd's default module
/// set.
///
/// fedimintd falls back to the registry's default set when no setup code
/// carries an explicit module list, and its setup UI preselects it. A module
/// wrapped here is still available to a formation that names it, but never
/// joins a federation that did not ask for it. Every other behavior is the
/// wrapped initializer's.
#[derive(Debug)]
pub(crate) struct NotEnabledByDefault<T>(pub(crate) T);

#[async_trait::async_trait]
impl<T> IDynCommonModuleInit for NotEnabledByDefault<T>
where
    T: ServerModuleInit + Sync + 'static,
{
    fn decoder(&self) -> Decoder {
        IDynCommonModuleInit::decoder(&self.0)
    }

    fn module_kind(&self) -> ModuleKind {
        IDynCommonModuleInit::module_kind(&self.0)
    }

    fn to_dyn_common(&self) -> DynCommonModuleInit {
        IDynCommonModuleInit::to_dyn_common(&self.0)
    }

    async fn dump_database(
        &self,
        dbtx: &mut DatabaseTransaction<'_>,
        prefix_names: Vec<String>,
    ) -> Box<dyn Iterator<Item = (String, Box<dyn erased_serde::Serialize + Send>)> + '_> {
        IDynCommonModuleInit::dump_database(&self.0, dbtx, prefix_names).await
    }
}

#[async_trait::async_trait]
impl<T> IServerModuleInit for NotEnabledByDefault<T>
where
    T: ServerModuleInit + Sync + 'static,
{
    fn as_common(&self) -> &(dyn IDynCommonModuleInit + Send + Sync + 'static) {
        self
    }

    async fn init(
        &self,
        peer_num: NumPeers,
        cfg: fedimint_core::config::ServerModuleConfig,
        db: fedimint_core::db::Database,
        task_group: &fedimint_core::task::TaskGroup,
        our_peer_id: PeerId,
        module_api: fedimint_api_client::api::DynModuleApi,
        server_bitcoin_rpc_monitor: ServerBitcoinRpcMonitor,
    ) -> anyhow::Result<DynServerModule> {
        IServerModuleInit::init(
            &self.0,
            peer_num,
            cfg,
            db,
            task_group,
            our_peer_id,
            module_api,
            server_bitcoin_rpc_monitor,
        )
        .await
    }

    fn trusted_dealer_gen(
        &self,
        peers: &[PeerId],
        args: &ConfigGenModuleArgs,
    ) -> BTreeMap<PeerId, fedimint_core::config::ServerModuleConfig> {
        IServerModuleInit::trusted_dealer_gen(&self.0, peers, args)
    }

    async fn distributed_gen(
        &self,
        peers: &(dyn PeerHandleOps + Send + Sync),
        args: &ConfigGenModuleArgs,
    ) -> anyhow::Result<fedimint_core::config::ServerModuleConfig> {
        IServerModuleInit::distributed_gen(&self.0, peers, args).await
    }

    fn validate_config(
        &self,
        identity: &PeerId,
        config: fedimint_core::config::ServerModuleConfig,
    ) -> anyhow::Result<()> {
        IServerModuleInit::validate_config(&self.0, identity, config)
    }

    fn get_client_config(
        &self,
        module_instance_id: ModuleInstanceId,
        config: &fedimint_core::config::ServerModuleConsensusConfig,
    ) -> anyhow::Result<fedimint_core::config::ClientModuleConfig> {
        IServerModuleInit::get_client_config(&self.0, module_instance_id, config)
    }

    fn get_database_migrations(&self) -> BTreeMap<DatabaseVersion, DynServerDbMigrationFn> {
        IServerModuleInit::get_database_migrations(&self.0)
    }

    fn used_db_prefixes(&self) -> Option<BTreeSet<u8>> {
        IServerModuleInit::used_db_prefixes(&self.0)
    }

    fn is_enabled_by_default(&self) -> bool {
        false
    }

    fn get_documented_env_vars(&self) -> Vec<EnvVarDoc> {
        IServerModuleInit::get_documented_env_vars(&self.0)
    }
}
