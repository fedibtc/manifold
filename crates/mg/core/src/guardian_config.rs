//! What MG reads from a seat's installed `fedimintd` config: which guardian
//! it is and the invite that dials it.
//!
//! The config is the one home of both facts. `formed_seats` records only the
//! [`FederationPeer`] the config names, and every invite MG hands out is built
//! here from the files the child boots, the way `fedimintd` builds its own:
//! its own API endpoint, its local identity and the federation id. A seat
//! without its config (data loss) therefore still knows its federation but
//! has no invite to give.

use std::path::Path;

use fedimint_core::config::FederationId;
use fedimint_core::encoding::Encodable as _;
use fedimint_core::invite_code::InviteCode;
use fedimint_server::config::{ServerConfigConsensus, ServerConfigLocal};

use crate::backup::{CONSENSUS_CONFIG_FILE, LOCAL_CONFIG_FILE, read_optional};
use crate::facts::FederationPeer;

/// The guardian an installed config is, and the invite it hands out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledGuardian {
    pub(crate) peer: FederationPeer,
    pub(crate) invite: InviteCode,
}

impl InstalledGuardian {
    /// `fedimintd`'s own construction (`ServerConfig::calculate_federation_id`
    /// and the invite it writes beside its config), from the two public parts
    /// of the config. No API secret: MG never configures one.
    pub(crate) fn from_config(
        consensus: &ServerConfigConsensus,
        local: &ServerConfigLocal,
    ) -> anyhow::Result<Self> {
        let endpoints = consensus.api_endpoints();
        let federation_id = FederationId(endpoints.consensus_hash());
        let own = endpoints.get(&local.identity).ok_or_else(|| {
            anyhow::anyhow!("installed config has no API endpoint for its own identity")
        })?;
        Ok(Self {
            peer: FederationPeer {
                federation_id,
                peer_id: local.identity,
            },
            invite: InviteCode::new(own.url.clone(), local.identity, federation_id, None),
        })
    }

    fn parse(consensus_json: &str, local_json: &str) -> anyhow::Result<Self> {
        Self::from_config(
            &serde_json::from_str(consensus_json)?,
            &serde_json::from_str(local_json)?,
        )
    }

    /// Read the config installed in a seat's data directory. `None` when it
    /// is not there: before the ceremony installs it, or after data loss.
    pub(crate) async fn read(seat_data_dir: &Path) -> anyhow::Result<Option<Self>> {
        let (Some(consensus), Some(local)) = (
            read_optional(seat_data_dir.join(CONSENSUS_CONFIG_FILE)).await?,
            read_optional(seat_data_dir.join(LOCAL_CONFIG_FILE)).await?,
        ) else {
            return Ok(None);
        };
        Self::parse(&consensus, &local)
            .map(Some)
            .map_err(|error| error.context("read the seat's installed fedimintd config"))
    }

    /// Read the installed config of a seat formed as `formed`. A config that
    /// names another guardian than the durable formed fact is corruption,
    /// never a reason to hand out its invite.
    pub(crate) async fn read_formed(
        seat_data_dir: &Path,
        formed: &FederationPeer,
    ) -> anyhow::Result<Option<Self>> {
        let installed = Self::read(seat_data_dir).await?;
        if let Some(installed) = &installed {
            anyhow::ensure!(
                installed.peer == *formed,
                "installed fedimintd config names another guardian than the formed seat"
            );
        }
        Ok(installed)
    }
}

#[cfg(test)]
#[path = "../tests/guardian_config.rs"]
mod tests;
