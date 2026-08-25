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

/// Third-party targets whose `info` is per-operation chatter rather than
/// deployment state.
///
/// `nostr_relay_pool` logs a connect and a disconnect for every relay
/// operation, and FLIP touches its relays on a timer, so at the default level
/// that one crate wrote more lines than the whole of FLIP.
///
/// An operator who names the same target in `RUST_LOG` wins: `add_directive`
/// replaces a directive for a target it already has, so the ones below are
/// skipped when the environment already speaks about that target.
const QUIET_DEPENDENCY_DIRECTIVES: &[&str] = &["nostr_relay_pool=warn"];

fn init_tracing() {
    let requested = std::env::var("RUST_LOG").unwrap_or_default();
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    for directive in QUIET_DEPENDENCY_DIRECTIVES {
        let target = directive.split('=').next().unwrap_or(directive);
        if requested.contains(target) {
            continue;
        }
        match directive.parse() {
            Ok(parsed) => filter = filter.add_directive(parsed),
            // A malformed constant is a bug in this list, not a reason to boot
            // without logging at all.
            Err(error) => eprintln!("ignoring malformed log directive {directive}: {error}"),
        }
    }
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
