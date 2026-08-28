//! `liquidity-manager-daemon` entry point: tracing, CLI parsing, and the choice
//! between normal and restore-only boot.

use fedi_decentralized_liquidity_manager_daemon::{
    CliCommand, DaemonMode, parse_cli, run_daemon, run_restore_daemon,
};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    init_tracing();

    match parse_cli() {
        CliCommand::RunDaemon(args) => {
            match args.mode {
                DaemonMode::Restore => return run_restore_daemon(args).await,
                DaemonMode::Normal => {}
            }
            let peer_badge_verifier =
                PeerBadgeVerifier::try_from_profile(&args.manifold_environment.profile()?)?;
            run_daemon(args, peer_badge_verifier).await?
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
