//! Imported guardians must satisfy the same native module checks as locally
//! formed guardians, plus the deployment's supported-network policy.

use anyhow::ensure;
use fedimint_server::config::ServerConfig;
use mg_core::transfer_native::GuardianConfigValidator;

pub(crate) struct NativeGuardianConfigValidator;

impl GuardianConfigValidator for NativeGuardianConfigValidator {
    fn validate(&self, config: &ServerConfig, network: bitcoin::Network) -> anyhow::Result<()> {
        let mut has_wallet = false;
        for (id, kind) in config.iter_module_instances() {
            match kind.as_str() {
                "wallet" => {
                    let module = config
                        .get_module_config_typed::<fedimint_wallet_common::config::WalletConfig>(
                            id,
                        )?;
                    ensure!(module.consensus.network.0 == network);
                    has_wallet = true;
                }
                "walletv2" => {
                    let module = config
                        .get_module_config_typed::<fedimint_walletv2_common::config::WalletConfig>(
                            id,
                        )?;
                    ensure!(module.consensus.network == network);
                    has_wallet = true;
                }
                "ln" => {
                    let module = config
                        .get_module_config_typed::<fedimint_ln_common::config::LightningConfig>(
                            id,
                        )?;
                    ensure!(module.consensus.network.0 == network);
                }
                "lnv2" => {
                    let module = config
                        .get_module_config_typed::<fedimint_lnv2_common::config::LightningConfig>(
                            id,
                        )?;
                    ensure!(module.consensus.network == network);
                }
                // Native mint validators index these maps. Check their shape
                // before invoking them, including in panic=abort builds.
                "mint" => {
                    let module = config
                        .get_module_config_typed::<fedimint_mint_common::config::MintConfig>(id)?;
                    ensure!(
                        module
                            .consensus
                            .peer_tbs_pks
                            .contains_key(&config.local.identity)
                    );
                }
                "mintv2" => {
                    let module = config
                        .get_module_config_typed::<fedimint_mintv2_common::config::MintConfig>(
                            id,
                        )?;
                    for denomination in fedimint_mintv2_common::config::consensus_denominations() {
                        ensure!(module.private.tbs_sks.contains_key(&denomination));
                        ensure!(
                            module
                                .consensus
                                .tbs_pks
                                .get(&denomination)
                                .is_some_and(|peers| peers.contains_key(&config.local.identity))
                        );
                    }
                }
                _ => {}
            }
        }
        ensure!(has_wallet, "guardian has no supported wallet module");
        config.validate_config(&config.local.identity, &crate::manifold_modules())
    }
}

#[cfg(test)]
mod tests;
