//! Deliberately unavailable adapters for consumer integration scaffolding.

use std::time::Duration;

use fedi_decentralized_nostr_clients::{FaNostrClient, NostrClientError, NostrClientResult};
use fedi_decentralized_service_manifold_guardian::*;
use nostr_sdk::{Event, PublicKey};

use crate::{
    FaFeeAccountError, FaFeeAccountProvider, FaPayments, FederationConsensusReader,
    ManifoldGuardianConnector,
};
use crate::{
    FaPaymentError, FederationConsensusError, FederationConsensusSnapshot, GuardianFeeAccount,
    ManifoldGuardianCallError, ManifoldGuardianConnectorError, PaymentReservationRecovery,
    PreparedSeatPayment, SeatPaymentRecovery, SettledSeatRefund,
};

/// FA fee-account capability that cannot resolve any formed federation.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableFaFeeAccountProvider;

impl FaFeeAccountProvider for UnavailableFaFeeAccountProvider {
    fn formed_federation_fee_account(
        &self,
        _federation_id: &crate::FedimintFederationId,
    ) -> Result<GuardianFeeAccount, FaFeeAccountError> {
        Err(FaFeeAccountError::new(
            "formed-federation FA fee account capability unavailable",
        ))
    }
}

/// Payment adapter that performs no value-moving operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailablePayments;

impl FaPayments for UnavailablePayments {
    type RefundContext = ();
    type PaymentReservation = ();
    type TerminalReleaseProof = ();

    async fn prepare_quote_refund(
        &self,
        _federation_id: &FederationId,
        _plan: &Plan,
    ) -> Result<RefundIssuance, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn payable_federations(
        &self,
        _admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn recover_payment_reservation(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<PaymentReservationRecovery<Self::PaymentReservation>, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn reserve_payment_requirements(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<Self::PaymentReservation, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn release_payment_reservation(
        &self,
        _reservation: Self::PaymentReservation,
    ) -> Result<(), FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn release_seat_payment_reservation(
        &self,
        _proof: Self::TerminalReleaseProof,
    ) -> Result<(), FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn recover_seat_payment(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext, Self::TerminalReleaseProof>, FaPaymentError>
    {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn create_seat_payment(
        &self,
        _reservation: &Self::PaymentReservation,
        _quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }

    async fn settle_seat_refund(
        &self,
        _context: Self::RefundContext,
        _refund: RefundTransaction,
    ) -> Result<SettledSeatRefund<Self::TerminalReleaseProof>, FaPaymentError> {
        Err(FaPaymentError::new("payment capability unavailable"))
    }
}

/// Registry adapter that performs no relay queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableRegistry;

impl FaNostrClient for UnavailableRegistry {
    async fn fetch_mg_advertisement(
        &self,
        _mg_pubkey: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Event> {
        Err(NostrClientError::MissingEvent {
            context: "FA registry capability unavailable",
        })
    }

    async fn fetch_setup_payment_federations(
        &self,
        _publisher: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        Err(NostrClientError::MissingEvent {
            context: "FA setup-payment registry capability unavailable",
        })
    }

    async fn fetch_mg_advertisements(&self, _timeout: Duration) -> NostrClientResult<Vec<Event>> {
        Err(NostrClientError::MissingEvent {
            context: "FA registry capability unavailable",
        })
    }
}

/// MG connector that performs no network connection.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableManifoldGuardianConnector;

impl ManifoldGuardianConnector for UnavailableManifoldGuardianConnector {
    type Client = UnavailableManifoldGuardianClient;

    async fn connect(
        &self,
        _locator: &Locator,
    ) -> Result<Self::Client, ManifoldGuardianConnectorError> {
        Err(ManifoldGuardianConnectorError::new(
            "MG transport capability unavailable",
        ))
    }

    async fn get_availability(
        &self,
        _client: &Self::Client,
        _request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, ManifoldGuardianCallError> {
        Err(ManifoldGuardianCallError::new(
            "MG transport capability unavailable",
        ))
    }

    async fn get_quote(
        &self,
        _client: &Self::Client,
        _request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, ManifoldGuardianCallError> {
        Err(ManifoldGuardianCallError::new(
            "MG transport capability unavailable",
        ))
    }
}

/// Unreachable associated client required by [`ManifoldGuardianConnector`].
#[derive(Clone, Copy, Debug)]
pub struct UnavailableManifoldGuardianClient;

impl ManifoldGuardianService for UnavailableManifoldGuardianClient {
    async fn get_availability(
        &self,
        _request: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse> {
        Err(unavailable())
    }

    async fn get_quote(
        &self,
        _request: GetQuoteRequest,
    ) -> FmResult<SignedResponse<GetQuoteResponse>> {
        Err(unavailable())
    }

    async fn create_seat(
        &self,
        _request: SignedRequest<CreateSeatRequest>,
    ) -> FmResult<SignedResponse<CreateSeatResponse>> {
        Err(unavailable())
    }

    async fn get_dkg_code(
        &self,
        _request: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse> {
        Err(unavailable())
    }

    async fn start_dkg(
        &self,
        _request: SignedRequest<StartDkgRequest>,
    ) -> FmResult<StartDkgResponse> {
        Err(unavailable())
    }

    async fn restart_dkg(
        &self,
        _request: SignedRequest<RestartDkgRequest>,
    ) -> FmResult<RestartDkgResponse> {
        Err(unavailable())
    }

    async fn get_status(
        &self,
        _request: SignedRequest<GetStatusRequest>,
    ) -> FmResult<GetStatusResponse> {
        Err(unavailable())
    }

    async fn get_invite_code(
        &self,
        _request: SignedRequest<GetInviteCodeRequest>,
    ) -> FmResult<GetInviteCodeResponse> {
        Err(unavailable())
    }

    async fn get_peer_attestation(
        &self,
        _request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse> {
        Err(unavailable())
    }

    async fn get_fman_trust_material(
        &self,
        _request: GetMgTrustMaterialRequest,
    ) -> FmResult<GetMgTrustMaterialResponse> {
        Err(unavailable())
    }

    async fn set_meta_field(
        &self,
        _request: SignedRequest<SetMetaFieldRequest>,
    ) -> FmResult<SetMetaFieldResponse> {
        Err(unavailable())
    }

    async fn propose_formation_meta(
        &self,
        _request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FmResult<ProposeFormationMetaResponse> {
        Err(unavailable())
    }

    async fn register_gateway(
        &self,
        _request: SignedRequest<RegisterGatewayRequest>,
    ) -> FmResult<RegisterGatewayResponse> {
        Err(unavailable())
    }

    async fn get_fedimint_stats(
        &self,
        _request: SignedRequest<GetFedimintStatsRequest>,
    ) -> FmResult<GetFedimintStatsResponse> {
        Err(unavailable())
    }

    async fn decommission_seat(
        &self,
        _request: SignedRequest<DecommissionSeatRequest>,
    ) -> FmResult<DecommissionSeatResponse> {
        Err(unavailable())
    }

    async fn pull_seat(
        &self,
        _request: SignedRequest<PullSeatRequest>,
    ) -> FmResult<PullSeatResponse> {
        Err(unavailable())
    }

    async fn transfer_seat(
        &self,
        _request: SignedRequest<SeatTransferGrant>,
    ) -> FmResult<TransferSeatResponse> {
        Err(unavailable())
    }

    async fn replace_seat_binding(
        &self,
        _request: SignedRequest<ReplaceSeatBindingRequest>,
    ) -> FmResult<ReplaceSeatBindingResponse> {
        Err(unavailable())
    }

    async fn remove_seat(
        &self,
        _request: SignedRequest<RemoveSeatRequest>,
    ) -> FmResult<RemoveSeatResponse> {
        Err(unavailable())
    }
}

/// Consensus reader that performs no federation query.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableFederationConsensusReader;

impl FederationConsensusReader for UnavailableFederationConsensusReader {
    async fn read_consensus(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        Err(FederationConsensusError::new(
            "federation consensus read capability unavailable",
        ))
    }

    async fn read_lnv2_gateways(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<Vec<GatewayApiUrl>, FederationConsensusError> {
        Err(FederationConsensusError::new(
            "federation consensus read capability unavailable",
        ))
    }
}

fn unavailable() -> ManifoldGuardianError {
    ManifoldGuardianError::Other("MG transport capability unavailable".to_owned())
}
