use clap::Parser as _;
use fedi_decentralized_cloud_fman_telemetry::{Args, init_logging, serve};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    init_logging();
    serve(Args::parse()).await
}
