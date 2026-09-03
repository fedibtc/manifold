//! Fleet Manager service trait.

use fedi_iroh_rpc::service;

use crate::status::{GetFedimintStatsRequest, GetFedimintStatsResponse};
use crate::{
    CreateSeatRequest, CreateSeatResponse, FleetManagerError, GetAvailabilityRequest,
    GetAvailabilityResponse, GetDkgCodeRequest, GetDkgCodeResponse, GetFmanTrustMaterialRequest,
    GetFmanTrustMaterialResponse, GetInviteCodeRequest, GetInviteCodeResponse,
    GetPeerAttestationRequest, GetPeerAttestationResponse, GetQuoteRequest, GetQuoteResponse,
    GetStatusRequest, GetStatusResponse, ProposeFormationMetaRequest, ProposeFormationMetaResponse,
    RegisterGatewayRequest, RegisterGatewayResponse, RestartDkgRequest, RestartDkgResponse,
    SetMetaFieldRequest, SetMetaFieldResponse, SignedRequest, SignedResponse, StartDkgRequest,
    StartDkgResponse,
};

/// Result type for Fleet Manager protocol calls.
pub type FmResult<T> = Result<T, FleetManagerError>;

/// Fedi App ↔ Fleet Manager protocol.
///
/// FI-authenticated requests travel as [`SignedRequest`] envelopes and FMan
/// commitment responses as [`SignedResponse`] envelopes (see
/// [`crate::signing`]); read/status responses stay unsigned.
#[service]
pub trait FleetManagerService {
    /// Public endpoint for clients checking if this FM has capacity.
    async fn get_availability(
        &self,
        request: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse>;

    /// Return a stateless signed key-locked ecash quote.
    async fn get_quote(
        &self,
        request: GetQuoteRequest,
    ) -> FmResult<SignedResponse<GetQuoteResponse>>;

    /// Verify a quote payment offline and accept or refund it.
    async fn create_seat(
        &self,
        request: SignedRequest<CreateSeatRequest>,
    ) -> FmResult<SignedResponse<CreateSeatResponse>>;

    /// Get this guardian's DKG code.
    async fn get_dkg_code(
        &self,
        request: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse>;

    /// Start DKG after all guardian codes are available.
    async fn start_dkg(
        &self,
        request: SignedRequest<StartDkgRequest>,
    ) -> FmResult<StartDkgResponse>;

    /// Replace the current child and start DKG on its fresh session.
    async fn restart_dkg(
        &self,
        request: SignedRequest<RestartDkgRequest>,
    ) -> FmResult<RestartDkgResponse>;

    /// Get current DKG or running-federation status.
    async fn get_status(
        &self,
        request: SignedRequest<GetStatusRequest>,
    ) -> FmResult<GetStatusResponse>;

    /// Get the federation invite code for a running seat.
    async fn get_invite_code(
        &self,
        request: SignedRequest<GetInviteCodeRequest>,
    ) -> FmResult<GetInviteCodeResponse>;

    /// Get the FMan-signed peer attestation for a running seat.
    ///
    /// This FI/seat-scoped read remains useful for diagnostics and FI
    /// backups. It is not the FLIP trust-material discovery path.
    async fn get_peer_attestation(
        &self,
        request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse>;

    /// Get this FMan's public signed current trust material.
    ///
    /// This unauthenticated read-only API is the FLIP/external-verifier source
    /// for holder authorizations and backing trust badges after consensus
    /// `fedi:fman_seat_bindings` metadata identifies this FMan as an operator.
    async fn get_fman_trust_material(
        &self,
        request: GetFmanTrustMaterialRequest,
    ) -> FmResult<GetFmanTrustMaterialResponse>;

    /// Set a metadata field on the running federation.
    async fn set_meta_field(
        &self,
        request: SignedRequest<SetMetaFieldRequest>,
    ) -> FmResult<SetMetaFieldResponse>;

    /// Propose the directory and fee policy as one formation-only metadata vote.
    async fn propose_formation_meta(
        &self,
        request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FmResult<ProposeFormationMetaResponse>;

    /// Store a client-reachable gateway URL in this guardian's LNv2 module.
    async fn register_gateway(
        &self,
        request: SignedRequest<RegisterGatewayRequest>,
    ) -> FmResult<RegisterGatewayResponse>;

    /// Get this seat's `fedimintd` stats.
    async fn get_fedimint_stats(
        &self,
        request: SignedRequest<GetFedimintStatsRequest>,
    ) -> FmResult<GetFedimintStatsResponse>;
}

#[cfg(test)]
mod tests;
