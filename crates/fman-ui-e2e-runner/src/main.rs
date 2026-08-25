//! Bridges the operator-UI Playwright suite to a real, defe-provisioned Fleet
//! Manager so the browser tests run against the genuine operator HTTP API
//! (`crates/fleet-manager/specs/SPEC-operator-http.md`).
//!
//! Run this under a `defe` server (either `defe exec fman-ui-e2e-runner ...` or
//! against a persistent `just defe-serve`), which sets `DEV_DEFE_SOCKET_PATH`.
//! The runner:
//!   1. acquires the FMan's launch dependencies (a regtest bitcoind and a Nostr
//!      relay), which defe requires to start a manager,
//!   2. asks defe for an exclusive Fleet Manager and reads its operator API URL
//!      and password,
//!   3. runs `pnpm test:e2e:fman` with `E2E_TARGET=daemon`, pointing the Vite
//!      proxy at that daemon and handing the password to the sign-in helper,
//!   4. lets every slot be torn down when this process exits (defe releases on
//!      client disconnect).
//!
//! Any extra CLI args are forwarded to `playwright test`, e.g.
//!   fman-ui-e2e-runner -- --headed -g "overview"

use anyhow::{Context, Result, bail};
use defe_client::{AsyncDefeClient, FmanRequest, ResourceDescriptor, SharingMode};

// Repo-relative path to the operator-UI workspace, resolved from this crate.
const OPERATOR_UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../operator-ui");

// The seat port grid this manager may use. The UI tracer never forms a
// federation, so the grid is only reserved, never bound.
const FIRST_PORT_BASE: u16 = 34_000;

#[tokio::main]
async fn main() -> Result<()> {
    let forwarded: Vec<String> = std::env::args().skip(1).collect();

    // 1. Acquire the manager's launch dependencies. These are non-owning: the
    //    leases must outlive the FMan, so hold them for the whole run.
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe — run under `defe exec` or a live `just defe-serve`")?;
    let bitcoind_lease = client
        .request_bitcoind(SharingMode::Shared)
        .await
        .context("request a regtest bitcoind from defe")?;
    let ResourceDescriptor::Bitcoind(bitcoind) = bitcoind_lease.descriptor.clone() else {
        bail!(
            "defe returned a non-bitcoind descriptor: {:?}",
            bitcoind_lease.descriptor
        );
    };
    let relay_lease = client
        .request_nostr_relay(SharingMode::Shared)
        .await
        .context("request a Nostr relay from defe")?;
    let ResourceDescriptor::NostrRelay(relay) = relay_lease.descriptor.clone() else {
        bail!(
            "defe returned a non-relay descriptor: {:?}",
            relay_lease.descriptor
        );
    };

    // 2. Acquire the Fleet Manager itself.
    let fman_lease = client
        .request_fman(FmanRequest {
            sharing: SharingMode::Exclusive,
            bitcoind,
            nostr_relay_url: relay.url.clone(),
            first_port_base: FIRST_PORT_BASE,
            // No federation is formed here, so no direct routes are needed.
            iroh_connect_overrides: String::new(),
        })
        .await
        .context("request an exclusive Fleet Manager from defe")?;

    let ResourceDescriptor::Fman(fman) = &fman_lease.descriptor else {
        bail!(
            "defe returned a non-FMan descriptor: {:?}",
            fman_lease.descriptor
        );
    };

    // Log the address and data dir, never the password.
    eprintln!(
        "fman-ui-e2e-runner: FMan operator API at {} (data dir {})",
        fman.admin_url,
        fman.data_dir.display()
    );

    // 3. Run Playwright against the real daemon. E2E_TARGET=daemon makes the
    //    config boot Vite only; the proxy target and operator password reach
    //    the browser via the Vite dev proxy and the sign-in helper respectively.
    let status = tokio::process::Command::new("pnpm")
        .arg("--dir")
        .arg(OPERATOR_UI_DIR)
        .arg("test:e2e:fman")
        .args(&forwarded)
        .env("E2E_TARGET", "daemon")
        .env("FMAN_ADMIN_PROXY_TARGET", &fman.admin_url)
        .env("FMAN_ADMIN_PASSWORD", &fman.admin_password)
        .status()
        .await
        .context("run `pnpm test:e2e:fman`")?;

    // 4. Release the FMan and its dependencies before exiting.
    drop(client);

    std::process::exit(status.code().unwrap_or(1));
}
