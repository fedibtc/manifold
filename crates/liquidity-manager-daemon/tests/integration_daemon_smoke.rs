mod test_support;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, ensure};
use fedi_decentralized_liquidity_manager_daemon::Database;
use fedi_decentralized_service_liquidity_manager::{
    AllocationItemTarget, GatewayId, GatewayName, ItemId, Sats,
};
use flate2::read::GzDecoder;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use test_support::{ADMIN_TOKEN, DaemonProcess, TestDataDir, TestPorts, wait_for_admin_ready};
use tracing_subscriber::EnvFilter;

const GATEWAY_SECRET: &str = "gateway-secret";
const BITCOIND_SECRET: &str = "bitcoind-secret";

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("fedi_decentralized_liquidity_manager_daemon=debug,info")
        }))
        .with_test_writer()
        .try_init();
}

/// Restore mode starts nothing that belongs to a generation.
///
/// The process is handed a public bind address and must leave it unbound.
/// Binding it is the first thing `public_rpc::serve` does, and that same call is
/// what later spawns the advertisement publisher, so an unbound address excludes
/// both. It must serve its own five API routes and none of the normal router's.
///
/// Note what this does not discriminate: a normal daemon with no provider
/// identity installed
/// also leaves the public address unbound, because `public_rpc::serve` waits for
/// an identity first. The assertion catches a restore-mode regression that starts
/// the transport; it is not evidence of an asymmetry between the two modes.
#[tokio::test(flavor = "multi_thread")]
async fn restore_mode_serves_no_normal_route_and_binds_no_public_transport() -> anyhow::Result<()> {
    init_logging();

    let data_dir = TestDataDir::new("integration-restore-mode-confinement")?;
    let ports = TestPorts::allocate()?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();

    let mut daemon = DaemonProcess::start_restore_mode(data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;

    // The public transport is a UDP/QUIC Iroh endpoint, so binding it here is
    // what proves it free. A refused TCP connect would prove nothing about it.
    let probe = std::net::UdpSocket::bind(ports.public_bind_address)
        .context("restore mode occupied the public bind address")?;
    drop(probe);

    // Restore-mode routes exist. `inspect_backup` is sent a request it will
    // reject; the assertion is that it is routed at all, not that it succeeds.
    for route in ["/admin/v1/get_health", "/admin/v1/inspect_backup"] {
        let status = client
            .post(format!("{admin_url}{route}"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&json!({ "archive": "/nonexistent/archive.tar.gz" }))
            .send()
            .await
            .with_context(|| format!("restore-mode request to {route}"))?
            .status();
        ensure!(
            status != StatusCode::NOT_FOUND,
            "restore mode should serve {route}, got {status}"
        );
    }

    // Normal-router verbs reach the restore router's wildcard rather than their
    // own handlers. The request carries the admin token, so the refusal is a
    // statement about the served surface and not about authentication. The
    // wildcard answers `unavailable` rather than 404 deliberately: the verb is
    // not missing from the daemon, the mode does not offer it, and that
    // condition ends. `restore_routes_require_auth_and_limit_surface` covers the
    // error body; what this asserts is that no normal handler runs.
    for route in [
        "/admin/v1/get_setup_state",
        "/admin/v1/apply_setup_config",
        "/admin/v1/get_provider_config",
        "/admin/v1/get_funds",
        "/admin/v1/create_deposit_address",
        "/admin/v1/request_withdrawal",
        "/admin/v1/create_backup",
    ] {
        let status = client
            .post(format!("{admin_url}{route}"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&json!({}))
            .send()
            .await
            .with_context(|| format!("normal-route probe to {route}"))?
            .status();
        ensure!(
            status == StatusCode::SERVICE_UNAVAILABLE,
            "restore mode should refuse {route} as unavailable, got {status}"
        );
    }

    daemon.stop()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_smoke_preserves_setup_state_across_restart() -> anyhow::Result<()> {
    init_logging();

    let ports = TestPorts::allocate()?;
    let data_dir = TestDataDir::new("integration-daemon-smoke")?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();

    let mut daemon = DaemonProcess::start(data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;

    let unauth_status = client
        .post(format!("{admin_url}/admin/v1/get_setup_state"))
        .send()
        .await?
        .status();
    assert_eq!(unauth_status, StatusCode::UNAUTHORIZED);

    let auth_status = client
        .post(format!("{admin_url}/admin/v1/get_setup_state"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .status();
    assert_eq!(auth_status, StatusCode::OK);

    store_setup_secrets(&client, &admin_url).await?;

    let apply_response: Value = client
        .post(format!("{admin_url}/admin/v1/apply_setup_config"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&setup_config(&admin_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(apply_response["status"], "pending_validation");

    let setup_response = get_setup_state(&client, &admin_url).await?;
    assert_setup_redacted(&setup_response);
    assert_no_plaintext_sqlite_secrets(data_dir.path())?;
    assert_nonempty_file(data_dir.path().join("secret-store.key"))?;

    let backup_response = create_backup(&client, &admin_url).await?;
    // Version 2 was the first format carrying per-file checksums; version 3 is
    // the first carrying a common recovery point. An archive written at an
    // older version lacks one or both, so restore refuses it by version rather
    // than reporting the missing part.
    assert_eq!(backup_response["manifest"]["version"], 3);
    assert!(
        backup_response["manifest"]["state_groups"]
            .as_array()
            .context("backup state_groups should be an array")?
            .iter()
            .any(|group| group == "database")
    );
    // The manifest must name the instant both stores were captured at. This is
    // the end-to-end check that the daemon really writes it: the unit tests
    // build the manifest in-process.
    assert!(
        backup_response["manifest"]["recovery_point"]["quiesced_at"]
            .as_u64()
            .is_some_and(|quiesced_at| quiesced_at > 0),
        "the manifest must record when the quiescence barrier was taken: {}",
        backup_response["manifest"]
    );
    assert_eq!(
        backup_response["manifest"]["recovery_point"]["stores"],
        serde_json::json!(["sqlite", "data_directory"])
    );
    let archive_path = PathBuf::from(
        backup_response["archive"]
            .as_str()
            .context("backup archive should be a string")?,
    );
    ensure!(
        archive_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".tar.gz")),
        "backup archive should use .tar.gz extension: {}",
        archive_path.display()
    );
    assert_nonempty_file(&archive_path)?;
    assert_no_plaintext_archive_secrets(&archive_path)?;

    let inspect_response = inspect_backup(&client, &admin_url, &archive_path).await?;
    assert_eq!(
        inspect_response["manifest"], backup_response["manifest"],
        "inspect should return the archive manifest"
    );

    // Restoring onto a running daemon is covered on its own by
    // `live_restore_rebuilds_the_runtime_without_restarting_the_process`; doing
    // it here would replace the state the rest of this test is asserting on.

    daemon.stop()?;

    let mut restarted = DaemonProcess::start(data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut restarted).await?;

    let restarted_setup_response = get_setup_state(&client, &admin_url).await?;
    assert_setup_redacted(&restarted_setup_response);
    assert_no_plaintext_sqlite_secrets(data_dir.path())?;
    assert_nonempty_file(data_dir.path().join("secret-store.key"))?;

    restarted.stop()?;

    let restored_data_dir = TestDataDir::new("integration-daemon-restore")?;
    let mut restore_daemon = DaemonProcess::start_restore_mode(restored_data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut restore_daemon).await?;

    // A verb restore mode does not serve answers as a typed service error, not
    // as a bodiless 404: a client cannot tell an empty 404 from an unreachable
    // daemon, and this mode exists for the one operation where that matters.
    let restore_mode_setup = client
        .post(format!("{admin_url}/admin/v1/get_setup_state"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?;
    assert_eq!(restore_mode_setup.status(), StatusCode::SERVICE_UNAVAILABLE);
    let restore_mode_error: Value = restore_mode_setup.json().await?;
    assert_eq!(restore_mode_error["code"], "unavailable");
    assert!(
        restore_mode_error["message"]
            .as_str()
            .is_some_and(|message| message.contains("restore-only mode")),
        "the error should name the mode, got: {restore_mode_error}"
    );

    let restore_response = restore_backup(&client, &admin_url, &archive_path).await?;
    assert_eq!(restore_response["status"], "pending_validation");
    assert!(
        restore_response["restored_state_groups"]
            .as_array()
            .context("restored_state_groups should be an array")?
            .iter()
            .any(|group| group == "database")
    );

    restore_daemon.stop()?;

    let mut restored_normal = DaemonProcess::start(restored_data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut restored_normal).await?;

    let restored_setup_response = get_setup_state(&client, &admin_url).await?;
    assert_setup_redacted(&restored_setup_response);
    assert_no_plaintext_sqlite_secrets(restored_data_dir.path())?;
    assert_nonempty_file(restored_data_dir.path().join("secret-store.key"))?;

    restored_normal.stop()?;
    Ok(())
}

/// The Stage 2 acceptance test: a backup is restored onto a running daemon and
/// the process never exits.
///
/// The load-bearing assertion is not that the restored state is served — it is
/// that the same process serves it. If the daemon restarted, the restore would
/// be the old stop/start procedure wearing a new route.
#[tokio::test(flavor = "multi_thread")]
async fn live_restore_rebuilds_the_runtime_without_restarting_the_process() -> anyhow::Result<()> {
    init_logging();

    let ports = TestPorts::allocate()?;
    // A live restore moves the displaced state to a *sibling* of the data dir,
    // so the data dir gets its own root here. Test roots share a parent, and
    // counting pre-restore dirs in a shared parent would see other tests' work.
    let root = TestDataDir::new("integration-daemon-live-restore")?;
    let data_dir = root.path().join("data");
    fs::create_dir_all(&data_dir).context("create live-restore data dir")?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();

    let mut daemon = DaemonProcess::start(&data_dir, &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;
    let original_pid = daemon.child.id();

    store_setup_secrets(&client, &admin_url).await?;

    // State A, captured into an archive.
    client
        .post(format!("{admin_url}/admin/v1/apply_setup_config"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&setup_config_named(&admin_url, "primary"))
        .send()
        .await?
        .error_for_status()?;
    let archive_path = PathBuf::from(
        create_backup(&client, &admin_url).await?["archive"]
            .as_str()
            .context("backup archive should be a string")?,
    );

    // State B, which the restore has to undo.
    client
        .post(format!("{admin_url}/admin/v1/apply_setup_config"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&setup_config_named(&admin_url, "replaced-after-backup"))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(
        get_setup_state(&client, &admin_url).await?["config"]["gateway"]["gateway_name"],
        "replaced-after-backup",
        "the daemon should be serving state B before the restore"
    );

    let restore_response = restore_backup(&client, &admin_url, &archive_path).await?;
    assert!(
        restore_response["restored_state_groups"]
            .as_array()
            .context("restored_state_groups should be an array")?
            .iter()
            .any(|group| group == "database"),
        "a live restore should report the state groups it restored"
    );

    // The swap happens after the response, so the restored state appears once
    // the new generation is installed.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = daemon.child.try_wait()? {
            anyhow::bail!("daemon exited during the live restore: {status}");
        }
        if let Ok(state) = get_setup_state(&client, &admin_url).await
            && state["config"]["gateway"]["gateway_name"] == "primary"
        {
            break;
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "restored state never became visible"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The whole point: same process, start to finish.
    ensure!(
        daemon.child.try_wait()?.is_none(),
        "daemon process exited across the restore"
    );
    assert_eq!(
        daemon.child.id(),
        original_pid,
        "the restore must not have replaced the process"
    );

    // The state the restore displaced is kept rather than deleted.
    let aside_dirs = fs::read_dir(root.path())?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".pre-restore-")
        })
        .count();
    ensure!(
        aside_dirs == 1,
        "a live restore should leave exactly one pre-restore directory, found {aside_dirs}"
    );

    // The daemon is still a working daemon, not just a working health endpoint.
    assert_setup_redacted(&get_setup_state(&client, &admin_url).await?);
    assert_nonempty_file(data_dir.join("secret-store.key"))?;

    daemon.stop()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_reports_startup_recovery_counts_for_seeded_work() -> anyhow::Result<()> {
    init_logging();

    let ports = TestPorts::allocate()?;
    let data_dir = TestDataDir::new("integration-daemon-recovery")?;
    seed_recovery_fixture(data_dir.path()).await?;

    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();
    let mut daemon = DaemonProcess::start(data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;

    let health = get_health(&client, &admin_url).await?;
    let encoded = health.to_string();
    ensure!(
        encoded.contains("recovery_complete=true"),
        "health did not report completed recovery: {encoded}"
    );
    for expected in ["active_allocation_items=1", "active_wallet_operations=1"] {
        ensure!(
            encoded.contains(expected),
            "health did not include {expected}: {encoded}"
        );
    }

    daemon.stop()?;
    Ok(())
}

async fn get_setup_state(client: &Client, admin_url: &str) -> anyhow::Result<Value> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/get_setup_state"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn get_health(client: &Client, admin_url: &str) -> anyhow::Result<Value> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/get_health"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn create_backup(client: &Client, admin_url: &str) -> anyhow::Result<Value> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/create_backup"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn inspect_backup(
    client: &Client,
    admin_url: &str,
    archive_path: &Path,
) -> anyhow::Result<Value> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/inspect_backup"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({ "archive": archive_path.to_string_lossy() }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn restore_backup(
    client: &Client,
    admin_url: &str,
    archive_path: &Path,
) -> anyhow::Result<Value> {
    // Keep the body on failure. A restore refusal carries its reason in the
    // ServiceError envelope, and `error_for_status` throws that away — leaving
    // a bare 500 that says nothing about which check refused the archive.
    let response = client
        .post(format!("{admin_url}/admin/v1/restore_backup"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({ "archive": archive_path.to_string_lossy() }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "restore_backup failed: {status} {body}"
    );
    Ok(serde_json::from_str(&body)?)
}

fn assert_setup_redacted(setup_response: &Value) {
    assert_eq!(setup_response["status"], "pending_validation");
    assert_eq!(
        setup_response["config"]["gateway"]["has_admin_credential"],
        true
    );
    assert_eq!(
        setup_response["config"]["chain_observer"]["backend"]["has_password"],
        true
    );

    let encoded = setup_response.to_string();
    assert!(!encoded.contains(GATEWAY_SECRET));
    assert!(!encoded.contains(BITCOIND_SECRET));
}

fn assert_no_plaintext_archive_secrets(archive_path: &Path) -> anyhow::Result<()> {
    let file =
        fs::File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .with_context(|| format!("decompress {}", archive_path.display()))?;
    ensure!(
        !contains_bytes(&bytes, GATEWAY_SECRET.as_bytes()),
        "{} contains plaintext gateway secret",
        archive_path.display()
    );
    ensure!(
        !contains_bytes(&bytes, BITCOIND_SECRET.as_bytes()),
        "{} contains plaintext bitcoind secret",
        archive_path.display()
    );
    Ok(())
}

fn assert_no_plaintext_sqlite_secrets(data_dir: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(data_dir)? {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.starts_with("flip.sqlite") {
            continue;
        }

        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        ensure!(
            !contains_bytes(&bytes, GATEWAY_SECRET.as_bytes()),
            "{} contains plaintext gateway secret",
            path.display()
        );
        ensure!(
            !contains_bytes(&bytes, BITCOIND_SECRET.as_bytes()),
            "{} contains plaintext bitcoind secret",
            path.display()
        );
    }

    Ok(())
}

fn assert_nonempty_file(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    ensure!(
        metadata.len() > 0,
        "{} should exist and be non-empty",
        path.display()
    );
    Ok(())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

async fn seed_recovery_fixture(data_dir: &Path) -> anyhow::Result<()> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let item_json = serde_json::to_string(&AllocationItemTarget::Gateway {
        item_id: ItemId("item-active".to_owned()),
        gateway_id: GatewayId("gateway-1".to_owned()),
        gateway_name: GatewayName("gateway".to_owned()),
        amount: Sats(10_000),
    })?;
    sqlx::query(
        "INSERT INTO allocations \
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
          request_json, verification_json, target_json, \
          committed_amount_sats, reserved_amount_sats) \
         VALUES ('federation-active', 'requester-1', 'provider-1', 'regtest', \
                 x'01', '{}', '{}', '{}', 10000, 10000)",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO allocation_items \
         (item_id, federation_id, source_type, status, committed_amount_sats, \
          reserved_amount_sats, item_json, created_at, updated_at) \
         VALUES ('item-active', 'federation-active', 'gateway', 'running', 10000, 10000, \
                 ?, unixepoch(), unixepoch())",
    )
    .bind(item_json)
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO wallet_operations \
         (operation_id, operation_type, status, federation_id, item_id, amount_sats, \
          created_at, updated_at) \
         VALUES ('wallet-active', 'gateway_funding', 'pending', 'federation-active', 'item-active', \
                 10000, unixepoch(), unixepoch())",
    )
    .execute(database.pool())
    .await?;
    database.pool().close().await;
    Ok(())
}

/// Stores the two named secrets a setup config does not carry.
///
/// Secrets are written by name, one at a time, so a config write can neither
/// store nor remove one — which is what stops a blank field from meaning
/// anything. `apply_setup_config` requires the gateway credential to be present
/// before it will accept a config at all.
async fn store_setup_secrets(client: &Client, admin_url: &str) -> anyhow::Result<()> {
    for (secret, value) in [
        ("gateway_admin_credential", GATEWAY_SECRET),
        ("chain_observer_password", BITCOIND_SECRET),
    ] {
        client
            .post(format!("{admin_url}/admin/v1/set_config_secret"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&json!({ "secret": secret, "update": { "action": "set", "value": value } }))
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}

fn setup_config(admin_url: &str) -> Value {
    setup_config_named(admin_url, "primary")
}

/// The gateway name is the marker the live-restore test uses to tell which
/// generation's state it is looking at.
fn setup_config_named(admin_url: &str, gateway_name: &str) -> Value {
    json!({
        "config": {
            "network": "regtest",
            "gateway": {
                "gateway_id": "gateway-1",
                "gateway_name": gateway_name,
                "admin_url": admin_url,
                "identity_metadata": []
            },
            "chain_observer": {
                "backend": {
                    "type": "bitcoind",
                    "url": admin_url,
                    "username": "bitcoin"
                }
            },
            "relays": [],
            "capacity": {
                "mode": "explicit_cap",
                "explicit_cap": 10000,
                "supported_sources": ["gateway", "stability_pool"]
            },
            "funding_policy": {
                "fee_reserve": 0,
                "confirmations": 1,
                "stability_pool_min_fee_rate_ppb": 0
            },
            "replenishment": {
                "warning_threshold": 1000,
                "critical_threshold": 500
            },
            "advertised_endpoint": {
                "endpoint_id": null,
                "transport": "iroh",
                "address": "iroh-node-id",
                "discovery_hints": [],
                "rpc_protocol_name": "fedi/flip/public-liquidity/1"
            },
            "advertisement": {
                "republish_interval": 600,
                "ready_advertisement_enabled": false
            },
            "provider_display": null,
            "policy": {
                "accepted_attester_policies": [],
                "supported_networks": ["regtest"]
            }
        }
    })
}
