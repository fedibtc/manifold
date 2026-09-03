use bitcoin::secp256k1::{PublicKey, SecretKey};
use fedi_iroh_rpc::IrohProtocol;
use iroh::Endpoint;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use secp256k1::{Keypair, SECP256K1};
use stability_pool_common::{Account, AccountType};

use super::*;
use crate::*;

const ALPN: &[u8] = b"fedi/fleet-manager/service-test/0.1";

#[derive(Debug, Clone)]
struct TestFleetManager;

fn unsupported<T>() -> FmResult<T> {
    Err(FleetManagerError::UnsupportedVerb {
        verb: "test".to_owned(),
    })
}

fn manager_key() -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[2; 32]).unwrap()
}

fn guardian_fee_account() -> GuardianFeeAccount {
    Account::single(
        PublicKey::from_secret_key(
            bitcoin::secp256k1::SECP256K1,
            &SecretKey::from_slice(&[3; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
    .try_into()
    .unwrap()
}

impl FleetManagerService for TestFleetManager {
    async fn get_availability(
        &self,
        _: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse> {
        unsupported()
    }

    async fn get_quote(
        &self,
        request: GetQuoteRequest,
    ) -> FmResult<SignedResponse<GetQuoteResponse>> {
        SignedResponse::create(
            &GetQuoteResponse {
                terms: QuoteTerms {
                    quote_nonce: [4; 32],
                    offer_epoch: OfferEpoch::from_bytes([0; 32]),
                    request,
                    price_msats: 0,
                    payment: None,
                },
            },
            &manager_key(),
        )
        .map_err(Into::into)
    }

    async fn create_seat(
        &self,
        request: SignedRequest<CreateSeatRequest>,
    ) -> FmResult<SignedResponse<CreateSeatResponse>> {
        let request = request.verify(Timestamp(1_000))?.into_inner();
        let quote = request.quote.verify(&manager_key().x_only_public_key().0)?;
        SignedResponse::create(
            &CreateSeatResponse {
                quote_id: quote.quote_id(),
                outcome: CreateSeatOutcome::Accepted {
                    seat_id: SeatId::from(QuoteId([0x0a; 32])),
                    guardian_fee_account: guardian_fee_account(),
                },
            },
            &manager_key(),
        )
        .map_err(Into::into)
    }

    async fn get_dkg_code(
        &self,
        _: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse> {
        unsupported()
    }
    async fn start_dkg(&self, _: SignedRequest<StartDkgRequest>) -> FmResult<StartDkgResponse> {
        unsupported()
    }
    async fn restart_dkg(
        &self,
        _: SignedRequest<RestartDkgRequest>,
    ) -> FmResult<RestartDkgResponse> {
        unsupported()
    }
    async fn get_status(&self, _: SignedRequest<GetStatusRequest>) -> FmResult<GetStatusResponse> {
        unsupported()
    }
    async fn get_invite_code(
        &self,
        _: SignedRequest<GetInviteCodeRequest>,
    ) -> FmResult<GetInviteCodeResponse> {
        unsupported()
    }
    async fn get_peer_attestation(
        &self,
        _: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse> {
        unsupported()
    }
    async fn get_fman_trust_material(
        &self,
        _: GetFmanTrustMaterialRequest,
    ) -> FmResult<GetFmanTrustMaterialResponse> {
        unsupported()
    }
    async fn set_meta_field(
        &self,
        _: SignedRequest<SetMetaFieldRequest>,
    ) -> FmResult<SetMetaFieldResponse> {
        unsupported()
    }
    async fn propose_formation_meta(
        &self,
        _request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FmResult<ProposeFormationMetaResponse> {
        Err(FleetManagerError::UnsupportedVerb {
            verb: "propose_formation_meta".to_owned(),
        })
    }

    async fn register_gateway(
        &self,
        _: SignedRequest<RegisterGatewayRequest>,
    ) -> FmResult<RegisterGatewayResponse> {
        unsupported()
    }
    async fn get_fedimint_stats(
        &self,
        _: SignedRequest<GetFedimintStatsRequest>,
    ) -> FmResult<GetFedimintStatsResponse> {
        unsupported()
    }
}

#[tokio::test]
async fn generated_client_round_trips_get_quote_and_create_seat() {
    let server_endpoint = Endpoint::bind(presets::N0DisableRelay).await.unwrap();
    let router = Router::builder(server_endpoint)
        .accept(
            ALPN,
            IrohProtocol::new(FleetManagerServiceServer::new(TestFleetManager)),
        )
        .spawn();
    let client_endpoint = Endpoint::bind(presets::N0DisableRelay).await.unwrap();
    let connection = client_endpoint
        .connect(router.endpoint().addr(), ALPN)
        .await
        .unwrap();
    let client = FleetManagerServiceClient::new(connection);
    let unsupported = client
        .transport()
        .get_availability(GetAvailabilityRequest)
        .await
        .expect("RPC transport succeeds")
        .expect_err("remote service refusal remains the inner result");
    assert!(matches!(
        unsupported,
        FleetManagerError::UnsupportedVerb { .. }
    ));
    let fi_key = Keypair::from_seckey_slice(SECP256K1, &[1; 32]).unwrap();
    let fi_id = FiId(fi_key.x_only_public_key().0);
    let quote = client
        .transport()
        .get_quote(GetQuoteRequest {
            fi_id,
            fedimintd_version: "0.0.0-test".parse().expect("valid test version"),
            federation_size: FederationSize(7),
            plan: Plan::InfiniteBestEffort {
                price_msats: 250000,
            },
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .expect("RPC transport succeeds")
        .unwrap();
    // The quote's identity is the hash of the signed bytes: stable across
    // the wire, computable by both sides without a canonicalization scheme.
    let quote_id = quote
        .verify(&manager_key().x_only_public_key().0)
        .unwrap()
        .quote_id();

    let response = client
        .create_seat(
            SignedRequest::create(
                &CreateSeatRequest {
                    ts: Timestamp(1_000),
                    fi_id,
                    quote,
                    payment_signatures: Vec::new(),
                },
                &fi_key,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .verify(&manager_key().x_only_public_key().0)
        .unwrap();
    assert_eq!(response.quote_id, quote_id);
    assert!(matches!(
        response.outcome,
        CreateSeatOutcome::Accepted { .. }
    ));

    router.shutdown().await.unwrap();
    client_endpoint.close().await;
}
