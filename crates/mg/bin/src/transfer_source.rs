use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    FmResult, Locator, MANIFOLD_GUARDIAN_ALPN, ManifoldGuardianServiceClient, SeatTransferGrant,
    SignedRequest, TransferSeatResponse,
};
use mg_core::transfer_source::{TransferSource, TransferSourceUnavailable};

pub(crate) struct IrohTransferSource {
    endpoint: iroh::Endpoint,
    local_e2e: bool,
}

impl IrohTransferSource {
    pub(crate) fn new(endpoint: iroh::Endpoint, local_e2e: bool) -> Self {
        Self {
            endpoint,
            local_e2e,
        }
    }
}

#[async_trait::async_trait]
impl TransferSource for IrohTransferSource {
    async fn transfer(
        &self,
        source: &Locator,
        grant: SignedRequest<SeatTransferGrant>,
    ) -> Result<FmResult<TransferSeatResponse>, TransferSourceUnavailable> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let address = source.endpoint_addr.clone();
            let connect_timeout = Duration::from_secs(30);
            // Verified discovery intentionally advertises endpoint IDs, not
            // local socket addresses. Isolated native tests have no public
            // discovery service. Enrich only routing, never the signed grant or
            // its service identity. Only the development/regtest local harness
            // can read this override.
            let (address, connect_timeout) = if let Some(path) = self
                .local_e2e
                .then(|| std::env::var_os("MG_E2E_TRANSFER_SERVICE_ROUTES"))
                .flatten()
            {
                let bytes = tokio::fs::read(path)
                    .await
                    .map_err(|_| TransferSourceUnavailable)?;
                let routes: Vec<Locator> =
                    serde_json::from_slice(&bytes).map_err(|_| TransferSourceUnavailable)?;
                let address = routes
                    .into_iter()
                    .find(|route| route.endpoint_addr.id == address.id)
                    .map_or(address, |route| route.endpoint_addr);
                // A restarted local source gets a new transport endpoint. Do
                // not spend the production connection deadline dialing its
                // stale route before retrying the refreshed test route. Keep
                // the RPC deadline unchanged: retirement still does real I/O.
                (address, Duration::from_secs(2))
            } else {
                (address, connect_timeout)
            };
            let connection = tokio::time::timeout(
                connect_timeout,
                self.endpoint.connect(address, MANIFOLD_GUARDIAN_ALPN),
            )
            .await
            .map_err(|_| TransferSourceUnavailable)?
            .map_err(|_| TransferSourceUnavailable)?;
            ManifoldGuardianServiceClient::new(connection)
                .transport()
                .transfer_seat(grant)
                .await
                .map_err(|_| TransferSourceUnavailable)
        })
        .await
        .map_err(|_| TransferSourceUnavailable)?
    }
}
