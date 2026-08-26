//! Bridges the operator-UI Playwright suite to a real, defe-provisioned FLIP
//! daemon so the browser tests run against genuine backend endpoints.
//!
//! Run this under a `defe` server (either `defe exec flip-ui-e2e-runner ...` or
//! against a persistent `just defe-serve`), which sets `DEV_DEFE_SOCKET_PATH`.
//! The runner:
//!   1. asks defe for an exclusive FLIP daemon and reads its admin URL + token,
//!   2. runs `pnpm test:e2e` with `E2E_TARGET=daemon`, pointing the Vite proxy
//!      at that daemon and handing the bootstrap token to the auth gate,
//!   3. lets the daemon be torn down when this process exits (defe releases the
//!      slot on client disconnect).
//!
//! Any extra CLI args are forwarded to `playwright test`, e.g.
//!   flip-ui-e2e-runner -- --headed -g "degraded"

use anyhow::{Context, Result, bail};
use defe_client::{AsyncDefeClient, FlipRequest, ResourceDescriptor, SharingMode};

// Repo-relative path to the operator-UI workspace, resolved from this crate.
const OPERATOR_UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../operator-ui");

#[tokio::main]
async fn main() -> Result<()> {
    let forwarded: Vec<String> = std::env::args().skip(1).collect();

    // 1. Acquire a real FLIP daemon from the defe server we run under. Keeping
    //    `client` alive for the whole run holds the lease; dropping the
    //    connection on exit tells the server to release the slot.
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe — run under `defe exec` or a live `just defe-serve`")?;
    let lease = client
        .request_flip(FlipRequest {
            sharing: SharingMode::Exclusive,
            iroh_connect_overrides: None,
            holder_authorization_relay_url: None,
        })
        .await
        .context("request an exclusive FLIP daemon from defe")?;

    let ResourceDescriptor::Flip(flip) = &lease.descriptor else {
        bail!(
            "defe returned a non-FLIP descriptor: {:?}",
            lease.descriptor
        );
    };

    // Log the address and data dir, never the token.
    eprintln!(
        "flip-ui-e2e-runner: FLIP admin at {} (data dir {})",
        flip.admin_url,
        flip.data_dir.display()
    );

    // 2. Run Playwright against the real daemon. E2E_TARGET=daemon makes the
    //    config boot Vite only; the proxy target and bootstrap token reach the
    //    browser via the Vite dev proxy and the auth-gate helper respectively.
    let status = tokio::process::Command::new("pnpm")
        .arg("--dir")
        .arg(OPERATOR_UI_DIR)
        .arg("test:e2e")
        .args(&forwarded)
        .env("E2E_TARGET", "daemon")
        .env("FLIP_ADMIN_PROXY_TARGET", &flip.admin_url)
        .env("FLIP_ADMIN_TOKEN", &flip.admin_token)
        .status()
        .await
        .context("run `pnpm test:e2e`")?;

    // 3. Release the FLIP slot before exiting.
    drop(client);

    std::process::exit(status.code().unwrap_or(1));
}
