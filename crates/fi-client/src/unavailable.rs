//! Deliberately unavailable adapters for consumer integration scaffolding.

use std::time::Duration;

use fedi_decentralized_nostr_clients::{FiNostrClient, NostrClientError, NostrClientResult};
use fedi_decentralized_service_fleet_manager::*;
use nostr_sdk::{Event, PublicKey};

use crate::{
    FederationConsensusError, FederationConsensusSnapshot, FiPaymentError, FleetManagerCallError,
    FleetManagerConnectorError, GuardianFeeAccount, PaymentReservationRecovery,
    PreparedSeatPayment, SeatPaymentRecovery, SettledSeatRefund,
};
use crate::{
    FederationConsensusReader, FiFeeAccountError, FiFeeAccountProvider, FiPayments,
    FleetManagerConnector,
};

/// FI fee-account capability that cannot resolve any formed federation.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableFiFeeAccountProvider;

impl FiFeeAccountProvider for UnavailableFiFeeAccountProvider {
    fn formed_federation_fee_account(
        &self,
        _federation_id: &crate::FedimintFederationId,
    ) -> Result<GuardianFeeAccount, FiFeeAccountError> {
        Err(FiFeeAccountError::new(
            "formed-federation FI fee account capability unavailable",
        ))
    }
}

/// Payment adapter that performs no value-moving operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailablePayments;

impl FiPayments for UnavailablePayments {
    type RefundContext = ();
    type PaymentReservation = ();
    type TerminalReleaseProof = ();

    async fn prepare_quote_refund(
        &self,
        _federation_id: &FederationId,
        _plan: &Plan,
    ) -> Result<RefundIssuance, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn payable_federations(
        &self,
        _admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn recover_payment_reservation(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<PaymentReservationRecovery<Self::PaymentReservation>, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn reserve_payment_requirements(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<Self::PaymentReservation, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn release_payment_reservation(
        &self,
        _reservation: Self::PaymentReservation,
    ) -> Result<(), FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn release_seat_payment_reservation(
        &self,
        _proof: Self::TerminalReleaseProof,
    ) -> Result<(), FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn recover_seat_payment(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        _quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext, Self::TerminalReleaseProof>, FiPaymentError>
    {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn create_seat_payment(
        &self,
        _reservation: &Self::PaymentReservation,
        _quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }

    async fn settle_seat_refund(
        &self,
        _context: Self::RefundContext,
        _refund: RefundTransaction,
    ) -> Result<SettledSeatRefund<Self::TerminalReleaseProof>, FiPaymentError> {
        Err(FiPaymentError::new("payment capability unavailable"))
    }
}

/// Registry adapter that performs no relay queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableRegistry;

impl FiNostrClient for UnavailableRegistry {
    async fn fetch_fman_advertisement(
        &self,
        _fman_pubkey: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Event> {
        Err(NostrClientError::MissingEvent {
            context: "FI registry capability unavailable",
        })
    }

    async fn fetch_setup_payment_federations(
        &self,
        _publisher: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        Err(NostrClientError::MissingEvent {
            context: "FI setup-payment registry capability unavailable",
        })
    }

    async fn fetch_fman_advertisements(&self, _timeout: Duration) -> NostrClientResult<Vec<Event>> {
        Err(NostrClientError::MissingEvent {
            context: "FI registry capability unavailable",
        })
    }
}

/// FMan connector that performs no network connection.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableFleetManagerConnector;

impl FleetManagerConnector for UnavailableFleetManagerConnector {
    type Client = UnavailableFleetManagerClient;

    async fn connect(
        &self,
        _locator: &Locator,
    ) -> Result<Self::Client, FleetManagerConnectorError> {
        Err(FleetManagerConnectorError::new(
            "FMan transport capability unavailable",
        ))
    }

    async fn get_availability(
        &self,
        _client: &Self::Client,
        _request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, FleetManagerCallError> {
        Err(FleetManagerCallError::new(
            "FMan transport capability unavailable",
        ))
    }

    async fn get_quote(
        &self,
        _client: &Self::Client,
        _request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, FleetManagerCallError> {
        Err(FleetManagerCallError::new(
            "FMan transport capability unavailable",
        ))
    }
}

/// Unreachable associated client required by [`FleetManagerConnector`].
#[derive(Clone, Copy, Debug)]
pub struct UnavailableFleetManagerClient;

impl FleetManagerService for UnavailableFleetManagerClient {
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

    async fn get_federation_trust_material(
        &self,
        _request: GetFederationTrustMaterialRequest,
    ) -> FmResult<GetFederationTrustMaterialResponse> {
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

fn unavailable() -> FleetManagerError {
    FleetManagerError::Other("FMan transport capability unavailable".to_owned())
}
