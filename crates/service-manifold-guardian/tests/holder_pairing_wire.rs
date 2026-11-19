//! Golden stream payloads for independent implementations. The holder event and
//! bearer are inert examples, not an authorization that any guardian can accept.
use std::{net::SocketAddr, time::Duration};

use fedi_decentralized_service_manifold_guardian::{LinkSecret, holder_pairing::*};
use fedi_decentralized_services::domain::ProtocolV1;
use fedi_iroh_rpc::IrohProtocol;
use iroh::{Endpoint, EndpointAddr, endpoint::presets, protocol::Router};
use nostr::EventId;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const EVENT: &str = r#"{"fixture":"not-a-signed-event"}"#;

struct FixtureService;
impl HolderPairingService for FixtureService {
    async fn submit_holder_authorization(
        &self,
        req: SubmitHolderAuthorization,
    ) -> PairingResult<HolderPairingReceipt> {
        assert_eq!(req.secret.as_bytes(), &[0x22; 32]);
        assert_eq!(req.event_json, EVENT);
        Ok(HolderPairingReceipt {
            event_id: EventId::from_byte_array([0x33; 32]),
        })
    }
}

async fn endpoint() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .relay_mode(iroh::RelayMode::Disabled)
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .net_report_config(iroh::NetReportConfig::minimal())
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))?
        .alpns(vec![HOLDER_PAIRING_ALPN.to_vec()])
        .bind()
        .await?)
}

fn addr(ep: &Endpoint) -> EndpointAddr {
    EndpointAddr::new(ep.id()).with_ip_addr(
        ep.bound_sockets()
            .into_iter()
            .find(|addr| addr.ip().is_loopback())
            .expect("loopback socket"),
    )
}

#[tokio::test]
async fn pairing_frames_match_golden_bytes() -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), exchange()).await?
}

async fn exchange() -> TestResult {
    // Forward one stream unchanged between the generated client and server so
    // the test observes both production encoders, not a second frame definition.
    let router = Router::builder(endpoint().await?)
        .accept(
            HOLDER_PAIRING_ALPN,
            IrohProtocol::new(HolderPairingServiceServer::new(FixtureService)),
        )
        .spawn();
    let tap = endpoint().await?;
    let caller = endpoint().await?;
    let send_request = async {
        let client =
            HolderPairingServiceClient::new(caller.connect(addr(&tap), HOLDER_PAIRING_ALPN).await?);
        let receipt = client
            .transport()
            .submit_holder_authorization(SubmitHolderAuthorization {
                version: ProtocolV1,
                secret: LinkSecret::from_bytes([0x22; 32]),
                event_json: EVENT.to_owned(),
            })
            .await??;
        assert_eq!(receipt.event_id, EventId::from_byte_array([0x33; 32]));
        TestResult::Ok(())
    };
    let capture = async {
        let incoming = tap.accept().await.expect("incoming test client").await?;
        let (mut reply, mut request) = incoming.accept_bi().await?;
        let request = request.read_to_end(4096).await?;
        let outgoing = tap
            .connect(addr(router.endpoint()), HOLDER_PAIRING_ALPN)
            .await?;
        let (mut send, mut recv) = outgoing.open_bi().await?;
        send.write_all(&request).await?;
        send.finish()?;
        let response = recv.read_to_end(4096).await?;
        assert_eq!(
            request.as_slice(),
            include_bytes!("fixtures/holder-pairing-request.cbor")
        );
        assert_eq!(
            response.as_slice(),
            include_bytes!("fixtures/holder-pairing-response.cbor")
        );
        reply.write_all(&response).await?;
        reply.finish()?;
        reply.stopped().await?;
        TestResult::Ok(())
    };
    tokio::try_join!(send_request, capture)?;
    router.shutdown().await?;
    Ok(())
}
