//! Outbound source RPC capability. The binary owns the shared Iroh endpoint;
//! core owns retry, validation and acceptance.

use fedi_decentralized_service_manifold_guardian::{
    FmResult, Locator, MgSignedRequest, TransferSeatRequest, TransferSeatResponse,
};

/// Transport failure is deliberately separate from a signed-grant refusal.
/// Neither the transport error chain nor remote diagnostic text is logged here.
#[derive(Debug, thiserror::Error)]
#[error("guardian transfer source is unavailable")]
pub struct TransferSourceUnavailable;

#[async_trait::async_trait]
pub trait TransferSource: Send + Sync {
    async fn transfer(
        &self,
        source: &Locator,
        grant: MgSignedRequest<TransferSeatRequest>,
    ) -> Result<FmResult<TransferSeatResponse>, TransferSourceUnavailable>;
}

#[cfg(test)]
pub(crate) struct UnavailableTransferSource;

#[cfg(test)]
#[async_trait::async_trait]
impl TransferSource for UnavailableTransferSource {
    async fn transfer(
        &self,
        _: &Locator,
        _: MgSignedRequest<TransferSeatRequest>,
    ) -> Result<FmResult<TransferSeatResponse>, TransferSourceUnavailable> {
        Err(TransferSourceUnavailable)
    }
}
