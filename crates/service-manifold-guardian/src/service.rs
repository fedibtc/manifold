//! Manifold Guardian service trait.

use fedi_iroh_rpc::service;

use crate::status::{GetFedimintStatsRequest, GetFedimintStatsResponse};
use crate::{
    CreateSeatRequest, CreateSeatResponse, DecommissionSeatRequest, DecommissionSeatResponse,
    GetAvailabilityRequest, GetAvailabilityResponse, GetDkgCodeRequest, GetDkgCodeResponse,
    GetInviteCodeRequest, GetInviteCodeResponse, GetMgTrustMaterialRequest,
    GetMgTrustMaterialResponse, GetPeerAttestationRequest, GetPeerAttestationResponse,
    GetQuoteRequest, GetQuoteResponse, GetStatusRequest, GetStatusResponse, ManifoldGuardianError,
    MgSignedRequest, ProposeFormationMetaRequest, ProposeFormationMetaResponse, PullSeatRequest,
    PullSeatResponse, RegisterGatewayRequest, RegisterGatewayResponse, RemoveSeatRequest,
    RemoveSeatResponse, ReplaceSeatBindingRequest, ReplaceSeatBindingResponse, RestartDkgRequest,
    RestartDkgResponse, SetMetaFieldRequest, SetMetaFieldResponse, SignedRequest, SignedResponse,
    StartDkgRequest, StartDkgResponse, TransferSeatRequest, TransferSeatResponse,
};

/// Result type for Manifold Guardian protocol calls.
pub type FmResult<T> = Result<T, ManifoldGuardianError>;

/// Fedi App ↔ Manifold Guardian protocol.
///
/// FA-authenticated requests travel as [`SignedRequest`] envelopes and MG
/// commitment responses as [`SignedResponse`] envelopes (see
/// [`crate::signing`]); read/status responses stay unsigned.
#[service]
pub trait ManifoldGuardianService {
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

    /// Get the MG-signed peer attestation for a running seat.
    ///
    /// This FA/seat-scoped read remains useful for diagnostics and FA
    /// backups. It is not the FLIP trust-material discovery path.
    async fn get_peer_attestation(
        &self,
        request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse>;

    /// Get this MG's public signed current trust material.
    ///
    /// This unauthenticated read-only API is the FLIP/external-verifier source
    /// for holder authorizations and backing trust badges after consensus
    /// `fedi:fman_seat_bindings` metadata identifies this MG as an operator.
    ///
    /// The method name is the RPC wire name, so it keeps the old `fman` form.
    async fn get_fman_trust_material(
        &self,
        request: GetMgTrustMaterialRequest,
    ) -> FmResult<GetMgTrustMaterialResponse>;

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

    /// Decommission the caller's own seat (development and staging only).
    ///
    /// Terminal and idempotent, with exactly the effect of an operator
    /// decommission. Production daemons refuse it with `UnsupportedVerb`; see
    /// [`crate::DecommissionSeatRequest`] for why it exists at all.
    async fn decommission_seat(
        &self,
        request: SignedRequest<DecommissionSeatRequest>,
    ) -> FmResult<DecommissionSeatResponse>;

    /// Make a bought `New` seat pull a guardian from the grant's source MG.
    async fn pull_seat(
        &self,
        request: SignedRequest<PullSeatRequest>,
    ) -> FmResult<PullSeatResponse>;

    /// Stop this MG's seat named by the grant and hand its guardian over to
    /// the grant's destination, which signs this call.
    async fn transfer_seat(
        &self,
        request: MgSignedRequest<TransferSeatRequest>,
    ) -> FmResult<TransferSeatResponse>;

    /// Vote to replace one moved peer's directory entry and fee recipient.
    async fn replace_seat_binding(
        &self,
        request: SignedRequest<ReplaceSeatBindingRequest>,
    ) -> FmResult<ReplaceSeatBindingResponse>;

    /// Delete a transferred seat's guardian data and backup.
    async fn remove_seat(
        &self,
        request: SignedRequest<RemoveSeatRequest>,
    ) -> FmResult<RemoveSeatResponse>;
}

#[cfg(test)]
mod tests;
