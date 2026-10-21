use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    FmResult, Locator, MANIFOLD_GUARDIAN_ALPN, ManifoldGuardianServiceClient, MgSignedRequest,
    TransferSeatRequest, TransferSeatResponse,
};
use mg_core::transfer_source::{TransferSource, TransferSourceUnavailable};

/// Pull a retired guardian over the MG's own service endpoint. Routing is the
/// endpoint's concern (production discovery, or the isolated harness's memory
/// lookup); this client never enriches the signed grant or its service identity.
pub(crate) struct IrohTransferSource {
    endpoint: iroh::Endpoint,
}

impl IrohTransferSource {
    pub(crate) fn new(endpoint: iroh::Endpoint) -> Self {
        Self { endpoint }
    }
}

#[async_trait::async_trait]
impl TransferSource for IrohTransferSource {
    async fn transfer(
        &self,
        source: &Locator,
        request: MgSignedRequest<TransferSeatRequest>,
    ) -> Result<FmResult<TransferSeatResponse>, TransferSourceUnavailable> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let connection = tokio::time::timeout(
                Duration::from_secs(30),
                self.endpoint
                    .connect(source.endpoint_addr.clone(), MANIFOLD_GUARDIAN_ALPN),
            )
            .await
            .map_err(|_| TransferSourceUnavailable)?
            .map_err(|_| TransferSourceUnavailable)?;
            ManifoldGuardianServiceClient::new(connection)
                .transport()
                .transfer_seat(request)
                .await
                .map_err(|_| TransferSourceUnavailable)
        })
        .await
        .map_err(|_| TransferSourceUnavailable)?
    }
}
