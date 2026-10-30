use std::sync::OnceLock;
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    FmResult, Locator, MANIFOLD_GUARDIAN_ALPN, ManifoldGuardianServiceClient, MgSignedRequest,
    TransferSeatCall, TransferSeatRequest, TransferSeatResponse,
};
use mg_core::transfer_source::{TransferSource, TransferSourceUnavailable};

#[cfg(test)]
mod tests;

/// Pull a retired guardian over the MG's own service endpoint. Routing is the
/// endpoint's concern (production discovery, or the isolated harness's memory
/// lookup); this client never enriches the signed grant or its service identity.
pub(crate) struct IrohTransferSource {
    endpoint: iroh::Endpoint,
    /// Bound after startup; resumed workers fail closed until it exists.
    proof_source: OnceLock<mg_nostr::ManifoldGuardianNostr>,
}

impl IrohTransferSource {
    pub(crate) fn new(endpoint: iroh::Endpoint) -> Self {
        Self {
            endpoint,
            proof_source: OnceLock::new(),
        }
    }

    /// Bind the owner of the current enrollment and badge-subject signing key.
    pub(crate) fn bind_proof_source(&self, source: mg_nostr::ManifoldGuardianNostr) {
        assert!(
            self.proof_source.set(source).is_ok(),
            "proof source already bound"
        );
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
            let proof = self
                .proof_source
                .get()
                .ok_or(TransferSourceUnavailable)?
                .transfer_admission_proof(&request)
                .map_err(|_| TransferSourceUnavailable)?;
            let call = TransferSeatCall::new(request, Some(proof));
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
                .transfer_seat(call)
                .await
                .map_err(|_| TransferSourceUnavailable)
        })
        .await
        .map_err(|_| TransferSourceUnavailable)?
    }
}
