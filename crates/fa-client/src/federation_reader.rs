//! The federation reads every consumer needs, over Fedimint's own API.

use fedi_decentralized_domain::{GatewayApiUrl, GuardianReports};
use fedi_decentralized_federation_preview::{ConnectorRegistry, FederationPreviewError};

use crate::{
    FederationConsensusError, FederationConsensusReader, FederationConsensusSnapshot, InviteCode,
};

/// [`FederationConsensusReader`] that reads each federation live through
/// `fedi-decentralized-federation-preview`, over the consumer's connectors.
#[derive(Clone)]
pub struct ConnectorsConsensusReader {
    connectors: ConnectorRegistry,
}

impl ConnectorsConsensusReader {
    #[must_use]
    pub fn new(connectors: ConnectorRegistry) -> Self {
        Self { connectors }
    }
}

/// A fixed message: a read error can name the invite and the guardians'
/// endpoints, and consumers log and show this one.
fn read_error(
    message: &'static str,
) -> impl FnOnce(FederationPreviewError) -> FederationConsensusError {
    move |_| FederationConsensusError::new(message)
}

impl FederationConsensusReader for ConnectorsConsensusReader {
    async fn read_guardian_reports(
        &self,
        invite_code: &InviteCode,
    ) -> Result<GuardianReports, FederationConsensusError> {
        fedi_decentralized_federation_preview::read_guardian_reports(&self.connectors, invite_code)
            .await
            .map_err(read_error("guardian reports read failed"))
    }

    async fn read_consensus(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        let snapshot =
            fedi_decentralized_federation_preview::read_consensus(&self.connectors, invite_code)
                .await
                .map_err(read_error("federation consensus read failed"))?;
        Ok(FederationConsensusSnapshot {
            config: snapshot.config,
            meta_value: snapshot.meta_value,
            meta_revision: snapshot.meta_revision,
            network: snapshot.network,
        })
    }

    async fn read_lnv2_gateways(
        &self,
        invite_code: &InviteCode,
    ) -> Result<Vec<GatewayApiUrl>, FederationConsensusError> {
        fedi_decentralized_federation_preview::read_lnv2_gateways(&self.connectors, invite_code)
            .await
            .map_err(read_error("federation gateway consensus read failed"))
    }

    async fn read_fedimintd_versions(
        &self,
        invite_code: &InviteCode,
    ) -> Result<Vec<String>, FederationConsensusError> {
        fedi_decentralized_federation_preview::read_fedimintd_versions(
            &self.connectors,
            invite_code,
        )
        .await
        .map_err(read_error("guardian fedimintd version read failed"))
    }
}
