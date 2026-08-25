use fedi_iroh_rpc::{IrohProtocol, RpcError, service};
use iroh::{Endpoint, endpoint::presets, protocol::Router};
use serde::{Deserialize, Serialize};

const ALPN: &[u8] = b"fedi/example/ping/1";

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Deserialize, Serialize, thiserror::Error)]
pub enum Error {
    #[error("transport: {0}")]
    Transport(String),
}

impl From<RpcError> for Error {
    fn from(error: RpcError) -> Self {
        Self::Transport(error.to_string())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PingRequest {
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PingResponse {
    message: String,
}

#[service]
pub trait PingService {
    async fn ping(&self, request: PingRequest) -> Result<PingResponse>;
}

#[derive(Debug, Clone)]
struct PingImpl;

impl PingService for PingImpl {
    async fn ping(&self, request: PingRequest) -> Result<PingResponse> {
        Ok(PingResponse {
            message: format!("pong: {}", request.message),
        })
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let server_endpoint = Endpoint::bind(presets::N0).await?;
    let server = PingServiceServer::new(PingImpl);
    let router = Router::builder(server_endpoint)
        .accept(ALPN, IrohProtocol::new(server))
        .spawn();
    router.endpoint().online().await;

    let client_endpoint = Endpoint::bind(presets::N0).await?;
    let connection = client_endpoint
        .connect(router.endpoint().addr(), ALPN)
        .await?;
    let client = PingServiceClient::new(connection);

    let response = client
        .ping(PingRequest {
            message: "hello".to_owned(),
        })
        .await?;
    println!("{}", response.message);

    router.shutdown().await?;
    client_endpoint.close().await;

    Ok(())
}
