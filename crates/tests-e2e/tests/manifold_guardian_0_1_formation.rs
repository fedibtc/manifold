use std::collections::BTreeSet;
use std::env;
use std::io::Write as _;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use defe_api::{GatewaydRequest, ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_decentralized_domain::{MG_SEAT_BINDINGS_META_FIELD_KEY, MgSeatBindings};
use fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_EVENT_KIND;
use fedi_decentralized_nostr::setup_payment_federations::{
    SETUP_PAYMENT_FEDERATIONS_D_TAG, SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedi_decentralized_service_manifold_guardian::{
    FaId, FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GUARDIAN_TELEMETRY_ALPN,
    GatewayApiUrl, GuardianTelemetryApi as _, GuardianTelemetryApiClient,
    ListGuardianTelemetrySeatsRequest, ListSafeEventJournalsRequest, Locator,
    MANIFOLD_GUARDIAN_ALPN, ManifoldGuardianError, ManifoldGuardianServiceClient,
    RegisterGatewayRequest, RegisterGatewayResponse, SafeEventJournal,
    ScrapeGuardianMetricsRequest, SeatId, ServiceErrorCode, SignedRequest, TelemetryCapability,
    Timestamp,
};
use fedi_iroh_rpc::iroh::{Endpoint, endpoint::presets};
use fedimint_derive_secret::{ChildId, DerivableSecret};
use futures_util::future::join_all;
use iroh_base_035::ticket::NodeTicket;
use iroh_base_035::{NodeAddr, NodeId, SecretKey};
use mg_fedimint::{Wallet as MgWallet, WalletSecret};
use nostr_sdk::{EventBuilder, Keys as NostrKeys, Kind, Tag};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

#[path = "manifold_guardian_0_1_formation/guardian_transfer.rs"]
mod guardian_transfer;
#[path = "manifold_guardian_0_1_formation/restart_dkg.rs"]
mod restart_dkg;

const OPT_IN_ENV: &str = "MG_E2E";
const MANIFOLD_GUARDIAN_BIN_ENV: &str = "MG_E2E_MANIFOLD_GUARDIAN_BIN";
const MG_CLI_BIN_ENV: &str = "MG_E2E_MG_CLI_BIN";
const FA_CLI_BIN_ENV: &str = "MG_E2E_FA_CLI_BIN";
const FEDIMINT_CLI_BIN_ENV: &str = "MG_E2E_FEDIMINT_CLI_BIN";
const BITCOIN_CLI_BIN_ENV: &str = "MG_E2E_BITCOIN_CLI_BIN";
const REPLAY_INVITE_ENV: &str = "MG_E2E_REPLAY_INVITE";
const REPLAY_TOKEN_FILE_ENV: &str = "MG_E2E_REPLAY_TOKEN_FILE";
const REPLAY_WALLET_DIR_ENV: &str = "MG_E2E_REPLAY_WALLET_DIR";
const GATEWAY_CLI_BIN_ENV: &str = "FLIP_E2E_GATEWAY_CLI_BIN";
/// fa-cli's local-E2E routes from advertised MG endpoint ids to loopback
/// addresses; see [`mg_endpoint_addrs`].
const MG_ENDPOINT_ADDRS_ENV: &str = "MG_E2E_MG_ENDPOINT_ADDRS";
const PAYOUT_CRASH_SEAM_ENABLE: &str = "enable-payout-crash-seam";
const PAYOUT_CRASH_SEAM_REACHED: &str = "payout-crash-seam-reached";
const GUARDIAN_COUNT: usize = 7;
/// Ports between consecutive MGs of one test's port grid: room for an MG's
/// seat blocks and, in the transfer tests, its pinned endpoint.
const MG_PORT_STRIDE: u16 = 100;
const PAID_GUARDIAN_COUNT: usize = 7;
const LOCATOR_TIMEOUT: Duration = Duration::from_secs(5);
const FA_CLI_TIMEOUT: Duration = Duration::from_secs(60);
const FEDIMINT_CLI_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
/// One consensus meta read against an already-joined federation.
const FEDIMINT_CLI_META_TIMEOUT: Duration = Duration::from_secs(15);
/// Loaded CI can need several consensus epochs before the initial metadata is visible.
const FEDIMINT_META_CONSENSUS_TIMEOUT: Duration = Duration::from_secs(60);
/// WalletV2's initial valid-address search is CPU-heavy in unoptimized CI builds.
const FEDIMINT_CLI_WALLETV2_TIMEOUT: Duration = Duration::from_secs(120);
/// Covers startup, formation, client join, metadata readiness, and clean shutdown.
const FORMATION_TIMEOUT: Duration = Duration::from_secs(180);
/// A killed 15-second FA invocation retains its lease for at most another 60
/// seconds. Guardians now form without waiting for FA resume, so callback
/// retries may already be in exponential backoff when the test unblocks them.
const FA_CRASH_RECOVERY_TIMEOUT: Duration = Duration::from_secs(320);
/// One real formation, child replacement, daemon restart, data-loss
/// projection, and terminal decommission.
const SEAT_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(420);
const POST_FORMATION_TIMEOUT: Duration = Duration::from_secs(600);
/// Formation, confirmed Nostr archive publication, mnemonic restore, and
/// restored guardian catch-up against the six surviving peers.
const FLEET_RESTORE_TIMEOUT: Duration = Duration::from_secs(420);
/// Bound individual regtest wallet and confirmation-mining commands.
const BITCOIN_CLI_TIMEOUT: Duration = Duration::from_secs(60);
/// Defe pre-funds this wallet before handing out an exclusive Bitcoin lease.
const MINER_WALLET: &str = "default";
const MINT_V2_REPLAY_TIMEOUT: Duration = Duration::from_secs(90);
/// The paid run adds wallet joins, an on-chain deposit past the regtest
/// finality delay, key-locked ecash payments, a second formation, and two
/// Lightning sweeps from the removed payment federation, one after an MG
/// restart and one after a mnemonic restore.
const PAID_FORMATION_TIMEOUT: Duration = Duration::from_secs(600);
/// Per-seat InfiniteBestEffort price the paid test charges: two ecash notes
/// (16384 + 4096 msat). One claimed payment leaves 19968 msat, of which the
/// MG lists 14 sats as sweepable after the Defe gateway's send fees (2000 msat
/// + 0.5% each for routing and transaction), covering
/// [`FORMER_PAYMENT_SWEEP_MSAT`].
const SEAT_PRICE_MSAT: u64 = 20_480;
/// One mint-v2 denomination large enough for every seat and transaction fee.
const FA_FUNDING_MSAT: u64 = 262_144;
/// What each MG sweeps out of the removed setup-payment federation.
const FORMER_PAYMENT_SWEEP_MSAT: u64 = 12_000;
const FA_WALLET_SECRET: [u8; 64] = [42; 64];
/// Authenticated policy locator required by kind-37707 version 1. The paid
/// formation gate does not call it; it only proves the complete event schema
/// is admitted before the MG wallets join.
const TELEMETRY_REGISTRATION_URL: &str = "https://push.fedi.example/v1/telemetry/registrations";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifold_guardian_0_1_forms_seven_guardian_federation_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping Manifold Guardian 0.1 E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }

    tokio::time::timeout(FORMATION_TIMEOUT, run_manifold_guardian_formation())
        .await
        .expect("Manifold Guardian 0.1 E2E timed out")
        .expect("Manifold Guardian 0.1 E2E failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "spawned by the paid formation test with an isolated connector environment"]
async fn mint_v2_receive_replay_child() {
    let invite = env::var(REPLAY_INVITE_ENV).expect("parent supplies the payment invite");
    let token_file =
        env::var_os(REPLAY_TOKEN_FILE_ENV).expect("parent supplies the bearer-token file");
    let wallet_dir = env::var_os(REPLAY_WALLET_DIR_ENV).expect("parent supplies the wallet dir");
    let token = std::fs::read_to_string(token_file).expect("read replay-test bearer token");
    let wallet = MgWallet::open(
        PathBuf::from(wallet_dir),
        &WalletSecret([91; 64]),
        mg_fedimint::WalletOrigin::Fresh,
    )
    .await
    .expect("open replay-test wallet");
    let federation_id = wallet
        .join(&invite.parse().expect("parse payment invite"))
        .await
        .expect("join payment federation");

    let first = wallet
        .receive_v2(federation_id, token.trim())
        .await
        .expect("initial mint-v2 receive");
    let replayed = wallet
        .receive_v2(federation_id, token.trim())
        .await
        .expect("resume mint-v2 receive from its durable operation");
    assert_eq!(first, replayed);
}

async fn run_manifold_guardian_formation() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")
        .unwrap_or_else(|err| panic!("{err}"));
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli").unwrap_or_else(|err| panic!("{err}"));
    let fedimint_cli_bin =
        locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli").unwrap_or_else(|err| panic!("{err}"));

    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe from env; run under `defe exec` or a persistent defe server");
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .expect("allocate real regtest bitcoind through defe");
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        panic!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };
    eprintln!(
        "allocated regtest bitcoind at {} with data dir {}",
        bitcoind.rpc_url,
        bitcoind.data_dir.display()
    );
    // Exclusive: fa-cli selects every MG advertised on this relay.
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .expect("allocate Nostr relay through defe");
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        panic!("expected Nostr relay descriptor");
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();

    let temp = mg_e2e_temp_dir()?;
    eprintln!("Manifold Guardian E2E data dir: {}", temp.display());
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(NostrEnv {
            relay_urls: &nostr_relay.url,
            holder_relay_url: &nostr_relay.url,
            setup_payment_publisher: &setup_payment_publisher,
        }),
        None,
    )
    .await;

    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT)
        .await
        .expect("offer the bootstrap seats");

    eprintln!("running fa-cli selection over {} MGs", locators.len());
    let invite_code = run_fa_cli(
        &fa_cli_bin,
        &nostr_relay.url,
        &mg_endpoint_addrs(&locators),
        FaCliInvocation {
            extra_args: &[],
            resume_args: None,
            wallet_secret: None,
            output: FaCliOutput::Human,
            seat_price_msats: 0,
        },
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        FA_CLI_TIMEOUT,
    )
    .await
    .expect("fa-cli completes 7-MG 0.1 formation and returns invite code");
    assert!(
        !invite_code.trim().is_empty(),
        "fa-cli returned a non-empty invite code"
    );
    assert_formation_has_v2_module_set(&temp, GUARDIAN_COUNT)
        .expect("every guardian committed the exact Manifold v2 module set");
    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fedimint-cli-client"),
        iroh_overrides: &iroh_overrides,
    };
    fedimint_cli
        .run(
            &["join-federation", invite_code.trim()],
            FEDIMINT_CLI_JOIN_TIMEOUT,
        )
        .await
        .expect("fedimint-cli joins the invite code returned by fa-cli");
    wait_for_seat_bindings_consensus(&fedimint_cli, GUARDIAN_COUNT)
        .await
        .expect(
            "the MG seat-binding directory reached consensus using the local-E2E child Iroh keys",
        );
    exercise_guardian_telemetry(&manifold_guardian_bin, &temp, &locators[0])
        .await
        .expect("the exact bundled guardian exposes every reviewed v2 metric");
    shutdown_daemons(daemons)
        .await
        .expect("Manifold Guardians shut down cleanly before journal inspection");
    assert_safe_event_journals(&temp, GUARDIAN_COUNT)
        .expect("every MG and embedded fedimintd wrote a bounded safe-event journal");
    defe.release(bitcoind_lease.handle_id)
        .await
        .expect("release bitcoind lease");
    std::fs::remove_dir_all(&temp).expect("remove Manifold Guardian E2E tempdir after success");

    Ok(())
}

/// Prove single-relay-outage liveness: with the first profile relay dead for
/// the whole test, the daemon still onboards — its Holder-authorization read
/// runs through the relay pool and the authorization only exists on the live
/// relay — and its advertisement lands on the live relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_advertises_and_onboards_with_first_relay_down_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping MG multi-relay liveness E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }

    tokio::time::timeout(Duration::from_secs(180), run_multi_relay_liveness())
        .await
        .expect("MG multi-relay liveness E2E timed out")
        .expect("MG multi-relay liveness E2E failed");
}

async fn run_multi_relay_liveness() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env; run under `defe exec` or a persistent defe server")?;
    let bitcoind_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!("expected bitcoind descriptor");
    };
    // Exclusive: the ad assertion below matches by event kind, so no other
    // test's MG may share this relay.
    let nostr_relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(live_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!("expected Nostr relay descriptor");
    };
    // Nothing listens here: the first relay in the profile list is down for
    // the whole test, which is exactly the outage the pool must survive.
    let dead_relay = "ws://127.0.0.1:9";
    let relay_urls = format!("{dead_relay} {}", live_relay.url);
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();

    let temp = mg_e2e_temp_dir()?;
    eprintln!("MG multi-relay liveness data dir: {}", temp.display());
    let port_base = reserve_mg_port_grid(1)?;
    let (daemons, _locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        None,
        1,
        Some(NostrEnv {
            relay_urls: &relay_urls,
            holder_relay_url: &live_relay.url,
            setup_payment_publisher: &setup_payment_publisher,
        }),
        None,
    )
    .await;

    // Onboarding completing inside start_daemons already proved the pooled
    // badge read. The advertisement must land on the live relay too.
    let observer = NostrRelayClient::connect(
        &live_relay.url,
        NostrKeys::generate(),
        Duration::from_secs(5),
    )
    .await
    .context("connect ad observer to the live relay")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let ads = observer
            .fetch_events_capped(
                nostr_sdk::Filter::new().kind(Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND)),
                Duration::from_secs(5),
                8,
            )
            .await
            .context("fetch advertisements from the live relay")?;
        if !ads.is_empty() {
            break;
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "no advertisement reached the live relay with the first relay down"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    shutdown_daemons(daemons).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove multi-relay liveness tempdir")?;
    Ok(())
}

/// Prove the operator's actual disaster-recovery path, not only the archive
/// codec: a formed guardian is published to a real Nostr relay, its original
/// host is stopped, and a fresh data root reconstructs and runs that guardian
/// from the mnemonic alone (SPEC-nostr-backup-restore).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_restores_a_formed_fleet_from_mnemonic_and_nostr_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping MG restore E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }

    tokio::time::timeout(FLEET_RESTORE_TIMEOUT, run_formed_fleet_restore())
        .await
        .expect("MG restore E2E timed out")
        .expect("MG restore E2E failed");
}

async fn run_formed_fleet_restore() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .context("allocate real regtest bitcoind through defe")?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .context("allocate local Nostr relay through defe")?;
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!(
            "expected Nostr relay descriptor from defe, got {:?}",
            nostr_relay_lease.descriptor
        );
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();
    let nostr = NostrEnv {
        relay_urls: &nostr_relay.url,
        holder_relay_url: &nostr_relay.url,
        setup_payment_publisher: &setup_payment_publisher,
    };

    let temp = mg_e2e_temp_dir()?;
    eprintln!("formed-fleet restore E2E data dir: {}", temp.display());
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (mut daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    let state_dir = temp.join("fi-state");
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &nostr_relay.url,
        &mg_endpoint_addrs(&locators),
        &iroh_overrides,
    )
    .await?;

    let original_dir = temp.join("mg-0");
    let original_identity =
        manifold_guardian_admin(&manifold_guardian_bin, &original_dir, &["onboarding"]).await?;
    let (seat_id, mnemonic) = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let seats =
                manifold_guardian_admin(&manifold_guardian_bin, &original_dir, &["seats", "list"])
                    .await?;
            if seats["backup_scan"]["pending_seats"].as_u64() == Some(0)
                && seats["seats"][0]["backup"]["archive_confirmed"].as_bool() == Some(true)
            {
                let seat_id = seats["seats"][0]["seat_id"]
                    .as_str()
                    .context("formed seat listing carries its id")?
                    .to_owned();
                let mnemonic = manifold_guardian_admin(
                    &manifold_guardian_bin,
                    &original_dir,
                    &["show-mnemonic"],
                )
                .await?["mnemonic"]
                    .as_str()
                    .context("show-mnemonic returns the root phrase")?
                    .to_owned();
                return Ok::<_, anyhow::Error>((seat_id, mnemonic));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("formed guardian archive was not confirmed on Nostr"))??;
    // The FA delivers one guardian's invite; this guardian serves its own.
    let seat_invite = manifold_guardian_admin(
        &manifold_guardian_bin,
        &original_dir,
        &["seats", "status", &seat_id],
    )
    .await?["report"]["invite_code"]
        .as_str()
        .context("formed seat reports its invite")?
        .to_owned();
    anyhow::ensure!(
        seat_invite
            .parse::<fedimint_core::invite_code::InviteCode>()?
            .federation_id()
            == invite
                .parse::<fedimint_core::invite_code::InviteCode>()?
                .federation_id(),
        "formed seat must join the FA's federation"
    );

    // The acknowledgement is truthful only after the original guardian is
    // down. Keep its old data root untouched: restore must need only the
    // mnemonic and relay, not a copied local artifact.
    let original = daemons.remove(0);
    shutdown_daemons(vec![original]).await?;
    let restored_dir = temp.join("restored-mg-0");
    let mnemonic_file = temp.join("restore-mnemonic");
    write_sensitive_file(&mnemonic_file, &mnemonic)?;
    let mut restored = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &restored_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base,
        Some(&iroh_overrides),
        Some(nostr),
        None,
    )?;
    let mnemonic_file_arg = mnemonic_file.display().to_string();
    let restored_answer = retry_manifold_guardian_admin(
        &manifold_guardian_bin,
        &restored_dir,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &mnemonic_file_arg,
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    anyhow::ensure!(
        restored_answer["onboarded"] == "restored"
            && restored_answer["seats"].as_u64() == Some(1)
            && restored_answer["formed"].as_u64() == Some(1),
        "restore must recover the complete formed fleet: {restored_answer}"
    );
    // A restore recovers the fleet but not the offer, so the wizard resumes at
    // the authorization stage; the relay already carries this identity's
    // authorization from its first onboarding.
    complete_onboarding_stages(&manifold_guardian_bin, &restored_dir, 1, nostr).await?;
    read_locator(&mut restored, 0).await?;

    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let status = manifold_guardian_admin(
                &manifold_guardian_bin,
                &restored_dir,
                &["seats", "status", &seat_id],
            )
            .await?;
            if status["report"]["phase"] == "running" && status["report"]["health"] == "healthy" {
                anyhow::ensure!(
                    status["report"]["invite_code"] == seat_invite,
                    "restored guardian must retain the exact formed invite: {status}"
                );
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("restored guardian did not catch up and become healthy"))??;
    let restored_mnemonic =
        manifold_guardian_admin(&manifold_guardian_bin, &restored_dir, &["show-mnemonic"]).await?;
    anyhow::ensure!(
        restored_mnemonic["mnemonic"] == mnemonic,
        "restored fleet must retain the original root identity"
    );
    let restored_identity =
        manifold_guardian_admin(&manifold_guardian_bin, &restored_dir, &["onboarding"]).await?;
    anyhow::ensure!(
        restored_identity["service_pubkey"] == original_identity["service_pubkey"]
            && restored_identity["service_nostr_pubkey"]
                == original_identity["service_nostr_pubkey"],
        "restored fleet must retain both original public service identities: original={original_identity}, restored={restored_identity}"
    );

    daemons.push(restored);
    shutdown_daemons(daemons).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove restore E2E tempdir")?;
    Ok(())
}

/// Two hosts that ran one mnemonic at once: host B is restored while host A
/// still runs, and each host then
/// forms one more guardian on its own. Both hosts number that guardian 1. A
/// later restore onto a clean host must still recover every seat of both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_restore_of_two_diverged_hosts_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping diverged-hosts restore E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }

    tokio::time::timeout(Duration::from_secs(900), run_diverged_hosts_restore())
        .await
        .expect("diverged-hosts restore E2E timed out")
        .expect("diverged-hosts restore E2E failed");
}

async fn wait_for_settled_backup(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    seats: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let listing =
                manifold_guardian_admin(manifold_guardian_bin, data_dir, &["seats", "list"])
                    .await?;
            let listed = listing["seats"].as_array().cloned().unwrap_or_default();
            if listing["backup_scan"]["pending_seats"].as_u64() == Some(0)
                && listed.len() == seats
                && listed
                    .iter()
                    .all(|seat| seat["backup"]["archive_confirmed"].as_bool() == Some(true))
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("backup of {} did not settle", data_dir.display()))?
}

async fn run_diverged_hosts_restore() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env().await?;
    let bitcoind_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!("expected bitcoind descriptor");
    };
    let nostr_relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!("expected Nostr relay descriptor");
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();
    let nostr = NostrEnv {
        relay_urls: &nostr_relay.url,
        holder_relay_url: &nostr_relay.url,
        setup_payment_publisher: &setup_payment_publisher,
    };

    let temp = mg_e2e_temp_dir()?;
    eprintln!("diverged-hosts restore E2E data dir: {}", temp.display());
    // Index GUARDIAN_COUNT is host B, GUARDIAN_COUNT + 1 is the clean host C.
    let max_seats = 3;
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT + 2)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 3, GUARDIAN_COUNT + 2);
    let (mut daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        max_seats,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    form_federation_in_state(
        &fa_cli_bin,
        &temp.join("fa-1"),
        &nostr_relay.url,
        &mg_endpoint_addrs(&locators),
        &iroh_overrides,
    )
    .await?;

    let host_a = temp.join("mg-0");
    wait_for_settled_backup(&manifold_guardian_bin, &host_a, 1).await?;
    let mnemonic = manifold_guardian_admin(&manifold_guardian_bin, &host_a, &["show-mnemonic"])
        .await?["mnemonic"]
        .as_str()
        .context("show-mnemonic returns the root phrase")?
        .to_owned();
    let mnemonic_file = temp.join("restore-mnemonic");
    write_sensitive_file(&mnemonic_file, &mnemonic)?;
    let mnemonic_file_arg = mnemonic_file.display().to_string();

    // Host B: restored while host A still runs, with a false acknowledgement.
    let host_b = temp.join(format!("mg-{GUARDIAN_COUNT}"));
    let mut b = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &host_b,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base + u16::try_from(GUARDIAN_COUNT).unwrap() * MG_PORT_STRIDE,
        Some(&iroh_overrides),
        Some(nostr),
        None,
    )?;
    let b_restore = retry_manifold_guardian_admin(
        &manifold_guardian_bin,
        &host_b,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &mnemonic_file_arg,
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    anyhow::ensure!(
        b_restore["seats"].as_u64() == Some(1),
        "host B must restore the seat of host A: {b_restore}"
    );
    complete_onboarding_stages(&manifold_guardian_bin, &host_b, max_seats, nostr).await?;
    let b_locator = read_locator(&mut b, GUARDIAN_COUNT).await?;
    retry_manifold_guardian_admin(
        &manifold_guardian_bin,
        &host_b,
        &["plans", "set", "--price-msats", "0"],
    )
    .await?;
    daemons.push(b);

    // Each host forms one more guardian on its own. Both hosts advertise
    // the same MG identity, so which one fa-cli dials is decided by the
    // address its endpoint id resolves to.
    form_federation_in_state(
        &fa_cli_bin,
        &temp.join("fa-2"),
        &nostr_relay.url,
        &mg_endpoint_addrs(&locators),
        &iroh_overrides,
    )
    .await?;
    let mut b_locators = locators.clone();
    b_locators[0] = b_locator;
    form_federation_in_state(
        &fa_cli_bin,
        &temp.join("fa-3"),
        &nostr_relay.url,
        &mg_endpoint_addrs(&b_locators),
        &iroh_overrides,
    )
    .await?;
    wait_for_settled_backup(&manifold_guardian_bin, &host_a, 2).await?;
    wait_for_settled_backup(&manifold_guardian_bin, &host_b, 2).await?;

    // Stop both hosts, then restore onto a clean host C. Each host numbered
    // its second seat 1, and C must restore both of them.
    shutdown_daemons(daemons).await?;
    let host_c = temp.join(format!("mg-{}", GUARDIAN_COUNT + 1));
    let mut c = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &host_c,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base + u16::try_from(GUARDIAN_COUNT + 1).unwrap() * MG_PORT_STRIDE,
        Some(&iroh_overrides),
        Some(nostr),
        None,
    )?;
    let c_restore = retry_manifold_guardian_admin(
        &manifold_guardian_bin,
        &host_c,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &mnemonic_file_arg,
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    anyhow::ensure!(
        c_restore["onboarded"] == "restored"
            && c_restore["seats"].as_u64() == Some(3)
            && c_restore["formed"].as_u64() == Some(3),
        "restore must recover the seats of both hosts: {c_restore}"
    );
    for seat_no in 0..3 {
        let config = host_c.join(format!("seats/{seat_no}/data/consensus.json"));
        anyhow::ensure!(config.exists(), "{} is missing", config.display());
    }
    complete_onboarding_stages(&manifold_guardian_bin, &host_c, max_seats, nostr).await?;
    read_locator(&mut c, GUARDIAN_COUNT + 1).await?;
    let c_seats =
        manifold_guardian_admin(&manifold_guardian_bin, &host_c, &["seats", "list"]).await?;
    anyhow::ensure!(
        c_seats["seats"].as_array().map(Vec::len) == Some(3),
        "the restored host must list the seats of both hosts: {c_seats}"
    );

    shutdown_daemons(vec![c]).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove diverged-hosts restore E2E tempdir")?;
    Ok(())
}

/// Exercise the adverse formed-seat lifecycle against the shipped process
/// boundary. Formation uses the real seven-guardian minimum; fault injection
/// is confined to one MG because the behavior under test is local ownership.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_recovers_a_real_child_and_terminalizes_data_loss_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping MG seat-lifecycle E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    if !cfg!(target_os = "linux") {
        eprintln!("skipping MG seat-lifecycle E2E; exact child replacement needs Linux pidfds");
        return;
    }

    tokio::time::timeout(SEAT_LIFECYCLE_TIMEOUT, run_real_seat_lifecycle())
        .await
        .expect("MG seat-lifecycle E2E timed out")
        .expect("MG seat-lifecycle E2E failed");
}

async fn run_real_seat_lifecycle() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .context("allocate real regtest bitcoind through defe")?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };

    // Exclusive: fa-cli selects every MG advertised on this relay.
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .context("allocate local Nostr relay through defe")?;
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!(
            "expected Nostr relay descriptor from defe, got {:?}",
            nostr_relay_lease.descriptor
        );
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();

    let temp = mg_e2e_temp_dir()?;
    let data_dir = temp.join("mg-0");
    let state_dir = temp.join("fi-state");
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (mut daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(NostrEnv {
            relay_urls: &nostr_relay.url,
            holder_relay_url: &nostr_relay.url,
            setup_payment_publisher: &setup_payment_publisher,
        }),
        None,
    )
    .await;
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    let mg_endpoints = mg_endpoint_addrs(&locators);
    form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &nostr_relay.url,
        &mg_endpoints,
        &iroh_overrides,
    )
    .await?;

    let seats =
        manifold_guardian_admin(&manifold_guardian_bin, &data_dir, &["seats", "list"]).await?;
    let seat_id = seats["seats"][0]["seat_id"]
        .as_str()
        .context("formed seat listing carries its id")?
        .to_owned();
    let status = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "status", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        status["report"]["phase"] == "running",
        "formed seat must report running before fault injection: {status}"
    );

    let mg_pid = daemons[0]
        .id()
        .context("Manifold Guardian exited before child fault injection")?;
    let old_pid = find_direct_child_named(mg_pid, "fedimintd")?;
    ExactProcess::open_direct_child(mg_pid, old_pid, "fedimintd")?.signal(libc::SIGKILL)?;
    let new_pid = wait_for_replacement_child(mg_pid, old_pid).await?;
    eprintln!("seat loop replaced killed fedimintd {old_pid} with {new_pid}");

    // The replacement must be serving the already-formed configuration, not
    // merely exist as a process.
    // Instrumented coverage builds can take tens of seconds to reopen the
    // configured child and complete its first authenticated API read.
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let status = manifold_guardian_admin(
                &manifold_guardian_bin,
                &data_dir,
                &["seats", "status", &seat_id],
            )
            .await?;
            let guardian_fee = &status["guardian_fee"];
            if status["report"]["phase"] == "running"
                && status["report"]["health"] == "healthy"
                && guardian_fee.get("error").is_none()
                && guardian_fee.get("policy_error").is_none()
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("replacement fedimintd did not serve a healthy guardian-fee policy read")
    })??;
    update_federation_name(
        &fa_cli_bin,
        &state_dir,
        &mg_endpoints,
        &iroh_overrides,
        "seat-lifecycle-e2e",
    )
    .await?;

    shutdown_daemons(std::mem::take(&mut daemons)).await?;
    std::fs::remove_dir_all(data_dir.join("seats/0/data"))
        .context("remove only the E2E seat's final data directory")?;

    let mut daemon = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &data_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base,
        Some(&iroh_overrides),
        None,
        None,
    )?;
    read_locator(&mut daemon, 0).await?;
    let status = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "status", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        status["report"]["phase"] == "data_loss" && status["report"]["health"] == "unavailable",
        "formed durable fact plus missing final data must report data_loss: {status}"
    );

    let first = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "decommission", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        first["decommissioned"] == true && first["already_decommissioned"] == false,
        "first decommission must terminalize the seat: {first}"
    );
    let second = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "decommission", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        second["decommissioned"] == true && second["already_decommissioned"] == true,
        "repeated decommission must be idempotent: {second}"
    );
    let status = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "status", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        status["report"]["state"] == "decommissioned",
        "terminal seat status must remain operator-visible: {status}"
    );

    shutdown_daemons(vec![daemon]).await?;
    let mut daemon = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &data_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base,
        Some(&iroh_overrides),
        None,
        None,
    )?;
    read_locator(&mut daemon, 0).await?;
    let status = manifold_guardian_admin(
        &manifold_guardian_bin,
        &data_dir,
        &["seats", "status", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        status["report"]["state"] == "decommissioned",
        "decommission must remain terminal after daemon restart: {status}"
    );
    anyhow::ensure!(
        find_direct_child_named(
            daemon
                .id()
                .context("restarted Manifold Guardian has a process id")?,
            "fedimintd"
        )
        .is_err(),
        "a decommissioned seat must not respawn fedimintd after daemon restart"
    );
    shutdown_daemons(vec![daemon]).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove seat-lifecycle E2E tempdir")?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_remits_collects_and_recovers_guardian_fee_payout_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping MG post-formation E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    tokio::time::timeout(
        POST_FORMATION_TIMEOUT,
        run_real_post_formation_operations(None),
    )
    .await
    .expect("MG post-formation E2E timed out")
    .expect("MG post-formation E2E failed");
}

/// Like Fedimint's upgrade-tests: create real state with the old executable,
/// replace binaries without replacing data, then exercise the same operations.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicit old/new release binaries and isolated defe resources"]
async fn mg_upgrades_existing_guardians_wallet_and_pending_payout_under_defe() {
    let old = env::var_os("MG_UPGRADE_FROM_BIN")
        .map(PathBuf::from)
        .expect("MG_UPGRADE_FROM_BIN must name the previous production fleet-manager");
    let new = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager").unwrap();
    assert_ne!(
        Sha256::digest(std::fs::read(&old).expect("read previous release")),
        Sha256::digest(std::fs::read(&new).expect("read candidate release")),
        "upgrade qualification must run different old and new executables"
    );
    tokio::time::timeout(
        Duration::from_secs(900),
        run_real_post_formation_operations(Some(&old)),
    )
    .await
    .expect("MG production upgrade timed out")
    .expect("MG production upgrade failed");
}

async fn run_real_post_formation_operations(upgrade_from: Option<&Path>) -> anyhow::Result<()> {
    let candidate_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let manifold_guardian_bin = upgrade_from.unwrap_or(&candidate_bin);
    let upgrade_to = upgrade_from.map(|_| candidate_bin.as_path());
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let bitcoin_cli_bin = locate_binary(BITCOIN_CLI_BIN_ENV, "bitcoin-cli")?;
    let gateway_cli_bin = locate_binary(GATEWAY_CLI_BIN_ENV, "gateway-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .context("allocate real regtest bitcoind through defe")?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };

    // Exclusive: fa-cli selects every MG advertised on this relay.
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .context("allocate local Nostr relay through defe")?;
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!(
            "expected Nostr relay descriptor from defe, got {:?}",
            nostr_relay_lease.descriptor
        );
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();

    let temp = mg_e2e_temp_dir()?;
    let bitcoin_cli = BitcoinCli::new(&bitcoin_cli_bin, bitcoind)?;
    let miner_address = bitcoin_cli.funded_miner_address().await?;
    let state_dir = temp.join("fi-state");
    #[cfg(target_os = "linux")]
    {
        let mg0 = temp.join("mg-0");
        std::fs::create_dir_all(&mg0)?;
        std::fs::write(mg0.join(PAYOUT_CRASH_SEAM_ENABLE), b"enabled\n")?;
    }
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (mut daemons, locators) = start_daemons(
        manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(NostrEnv {
            relay_urls: &nostr_relay.url,
            holder_relay_url: &nostr_relay.url,
            setup_payment_publisher: &setup_payment_publisher,
        }),
        None,
    )
    .await;
    offer_free_seats(manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    let mg_endpoints = mg_endpoint_addrs(&locators);
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &nostr_relay.url,
        &mg_endpoints,
        &iroh_overrides,
    )
    .await?;

    configure_guardian_fees(
        &fa_cli_bin,
        &state_dir,
        &mg_endpoints,
        &iroh_overrides,
        5_000,
    )
    .await?;
    // A generic metadata write after fee adoption must validate and carry the
    // complete authenticated fee policy rather than dropping or restating it.
    update_federation_name(
        &fa_cli_bin,
        &state_dir,
        &mg_endpoints,
        &iroh_overrides,
        "post-formation-e2e",
    )
    .await?;
    assert_guardian_fee_policy(manifold_guardian_bin, &temp, 5_000).await?;
    exercise_guardian_fee_wallet(manifold_guardian_bin, &temp).await?;
    if upgrade_to.is_none() {
        exercise_guardian_telemetry(manifold_guardian_bin, &temp, &locators[0]).await?;
    }
    exercise_real_guardian_fee_remittance_and_payout_recovery(
        &mut defe,
        bitcoind,
        manifold_guardian_bin,
        &fa_cli_bin,
        &gateway_cli_bin,
        &bitcoin_cli,
        &miner_address,
        &temp,
        port_base,
        &state_dir,
        &iroh_overrides,
        &invite,
        &mut daemons,
        &locators,
        upgrade_to,
    )
    .await?;

    shutdown_daemons(daemons).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove post-formation E2E tempdir")?;
    Ok(())
}

async fn exercise_guardian_fee_wallet(
    manifold_guardian_bin: &Path,
    temp: &Path,
) -> anyhow::Result<()> {
    let data_dir = temp.join("mg-0");
    let seats =
        manifold_guardian_admin(manifold_guardian_bin, &data_dir, &["seats", "list"]).await?;
    let seat_id = seats["seats"][0]["seat_id"]
        .as_str()
        .context("formed seat listing carries its id")?;

    // Formation can complete before the stability-pool module reaches its
    // first consensus cycle. Wait for that production readiness boundary
    // instead of racing the first account read.
    let first_cycle_deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let status = loop {
        match manifold_guardian_admin_with_timeout(
            manifold_guardian_bin,
            &data_dir,
            &["guardian-fees", "show", seat_id, "--limit", "1"],
            Duration::from_secs(10),
        )
        .await
        {
            Ok(status) => break status,
            Err(error) if tokio::time::Instant::now() < first_cycle_deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error.context("wait for the first guardian-fee cycle")),
        }
    };
    anyhow::ensure!(
        status["seat_id"] == seat_id
            && status["collectable_msat"].as_u64() == Some(0)
            && status["lifetime_remitted_msat"].as_u64() == Some(0)
            && status["remittances"].as_array().is_some_and(Vec::is_empty),
        "fresh guardian account must be readable and empty: {status}"
    );

    let collected = manifold_guardian_admin(
        manifold_guardian_bin,
        &data_dir,
        &["guardian-fees", "collect", seat_id],
    )
    .await?;
    anyhow::ensure!(
        collected["claimed_msat"].as_u64() == Some(0)
            && collected["awaiting_cycle_msat"].as_u64() == Some(0),
        "collecting a fresh guardian account must be a successful no-op: {collected}"
    );
    Ok(())
}

/// Drive the payer-to-operator money path through real federation modules:
/// wallet-v2 funding, a metadata-bearing stability-pool remittance, collection
/// into ecash, and an LNURL payout that survives a whole daemon restart
/// (REQ-guardian-fee-remittance).
#[expect(clippy::too_many_arguments)]
async fn exercise_real_guardian_fee_remittance_and_payout_recovery(
    defe: &mut AsyncDefeClient,
    bitcoind: &defe_api::BitcoindInfo,
    manifold_guardian_bin: &Path,
    fa_cli_bin: &Path,
    gateway_cli_bin: &Path,
    bitcoin_cli: &BitcoinCli<'_>,
    miner_address: &str,
    temp: &Path,
    port_base: u16,
    state_dir: &Path,
    iroh_overrides: &str,
    invite: &str,
    daemons: &mut Vec<Child>,
    locators: &[String],
    upgrade_to: Option<&Path>,
) -> anyhow::Result<()> {
    const REMITTANCE_MSAT: u64 = 200_000;
    const PAYOUT_MSAT: u64 = 50_000;

    let gateway_lease = defe
        .request_gatewayd(GatewaydRequest {
            sharing: SharingMode::Exclusive,
            bitcoind: bitcoind.clone(),
            iroh_connect_overrides: Some(iroh_overrides.to_owned()),
        })
        .await
        .context("allocate a real Fedimint gateway through defe")?;
    let ResourceDescriptor::Gatewayd(gateway) = &gateway_lease.descriptor else {
        anyhow::bail!(
            "expected gatewayd descriptor from defe, got {:?}",
            gateway_lease.descriptor
        );
    };
    wait_for_gateway_connect(gateway_cli_bin, gateway, invite).await?;
    register_local_gateway_for_lnv2(temp, invite, iroh_overrides, &gateway.api_url).await?;
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()?
        .federation_id()
        .to_string();
    fund_gateway_federation_wallet(
        gateway_cli_bin,
        gateway,
        bitcoin_cli,
        miner_address,
        &federation_id,
    )
    .await?;

    let mg0_dir = temp.join("mg-0");
    let seats =
        manifold_guardian_admin(manifold_guardian_bin, &mg0_dir, &["seats", "list"]).await?;
    let seat_id = seats["seats"][0]["seat_id"]
        .as_str()
        .context("formed seat listing carries its id")?
        .to_owned();
    let empty = manifold_guardian_admin(
        manifold_guardian_bin,
        &mg0_dir,
        &["guardian-fees", "show", &seat_id, "--limit", "1"],
    )
    .await?;
    let account: stability_pool_common::Account = serde_json::from_str(
        empty["remittance_account"]
            .as_str()
            .context("guardian-fee status carries its serialized account")?,
    )?;
    let account_id = account.id().to_string();
    let recipient = account
        .as_single()
        .context("guardian remittance account is single-signature")?;
    let metadata = mg_core::remittance_metadata::encrypt(
        recipient,
        &mg_core::remittance_metadata::RemittanceMetadata {
            version: 1,
            total_msats: REMITTANCE_MSAT,
            breakdown: vec![mg_core::remittance_metadata::RemittanceBreakdownItem {
                module: "mint".to_owned(),
                direction: "send".to_owned(),
                amount_msats: REMITTANCE_MSAT,
            }],
            remitted_at_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        },
    )?;
    let metadata_file = temp.join("guardian-remittance-metadata");
    write_sensitive_file(&metadata_file, std::str::from_utf8(&metadata)?)?;

    let wallet_dir = temp.join("guardian-remittance-payer");
    let wallet_secret_file = temp.join("guardian-remittance-wallet-secret");
    write_sensitive_file(&wallet_secret_file, &hex::encode([71_u8; 64]))?;
    run_fa_payment_wallet(
        fa_cli_bin,
        &wallet_dir,
        &wallet_secret_file,
        &["join", "--payment-invite-code", invite],
        iroh_overrides,
        Duration::from_secs(60),
    )
    .await?;
    let deposit = run_fa_payment_wallet(
        fa_cli_bin,
        &wallet_dir,
        &wallet_secret_file,
        &[
            "deposit-address",
            "--payment-federation-id",
            &invite
                .parse::<fedimint_core::invite_code::InviteCode>()?
                .federation_id()
                .to_string(),
            "--timeout-secs",
            "120",
        ],
        iroh_overrides,
        Duration::from_secs(130),
    )
    .await?;
    let deposit_address = deposit["address"]
        .as_str()
        .context("payment-wallet deposit-address returns an address")?;
    bitcoin_cli
        .run(
            Some(MINER_WALLET),
            &["sendtoaddress", deposit_address, "0.001"],
        )
        .await?;
    bitcoin_cli
        .run(None, &["generatetoaddress", "7", miner_address])
        .await?;
    run_fa_payment_wallet(
        fa_cli_bin,
        &wallet_dir,
        &wallet_secret_file,
        &[
            "wait-balance",
            "--payment-federation-id",
            &federation_id,
            "--minimum-sats",
            "1000",
            "--timeout-secs",
            "180",
        ],
        iroh_overrides,
        Duration::from_secs(190),
    )
    .await?;
    run_fa_payment_wallet(
        fa_cli_bin,
        &wallet_dir,
        &wallet_secret_file,
        &[
            "remit-guardian-fee",
            "--payment-federation-id",
            &federation_id,
            "--account-id",
            &account_id,
            "--amount-msats",
            &REMITTANCE_MSAT.to_string(),
            "--metadata-file",
            &metadata_file.display().to_string(),
        ],
        iroh_overrides,
        Duration::from_secs(60),
    )
    .await?;

    let remitted = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let status = manifold_guardian_admin(
                manifold_guardian_bin,
                &mg0_dir,
                &["guardian-fees", "show", &seat_id, "--limit", "1"],
            )
            .await?;
            if status["lifetime_remitted_msat"].as_u64() == Some(REMITTANCE_MSAT) {
                return Ok::<_, anyhow::Error>(status);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("real guardian-fee remittance did not become visible"))??;
    anyhow::ensure!(
        remitted["remittances"][0]["total_msat"].as_u64() == Some(REMITTANCE_MSAT)
            && remitted["remittances"][0]["breakdown"][0]["module"] == "mint",
        "guardian must decrypt the payer's accounting metadata: {remitted}"
    );

    let collected = manifold_guardian_admin(
        manifold_guardian_bin,
        &mg0_dir,
        &["guardian-fees", "collect", &seat_id],
    )
    .await?;
    anyhow::ensure!(
        collected["claimed_msat"].as_u64().unwrap_or_default() >= PAYOUT_MSAT,
        "collection must turn real remittance into sweepable ecash: {collected}"
    );

    // connect-fed returns before the LNv2 announcement and public routing
    // endpoint necessarily converge. Invoice selection failure is explicitly
    // pre-operation, so this readiness retry cannot duplicate a receive.
    let invoice_args = [
        "invoice",
        "--payment-federation-id",
        &federation_id,
        "--amount-sats",
        &(PAYOUT_MSAT / 1_000).to_string(),
    ];
    let invoice = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match run_fa_payment_wallet(
                fa_cli_bin,
                &wallet_dir,
                &wallet_secret_file,
                &invoice_args,
                iroh_overrides,
                Duration::from_secs(15),
            )
            .await
            {
                Ok(invoice) => return Ok::<_, anyhow::Error>(invoice),
                Err(error) => {
                    let _ = error;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("LNv2 gateway did not become ready for invoice creation"))??;
    let invoice_operation = invoice["operationId"]
        .as_str()
        .context("payment-wallet invoice returns operation id")?
        .to_owned();
    let invoice = invoice["invoice"]
        .as_str()
        .context("payment-wallet invoice returns BOLT11")?
        .to_owned();
    let lnurl = LnurlPayServer::start(invoice, PAYOUT_MSAT).await?;
    manifold_guardian_admin(
        manifold_guardian_bin,
        &mg0_dir,
        &["payout", "set", lnurl.destination()],
    )
    .await?;
    let mut upgrade_identities = Vec::new();
    if upgrade_to.is_some() {
        for index in 0..GUARDIAN_COUNT {
            let dir = temp.join(format!("mg-{index}"));
            let identity =
                manifold_guardian_admin(manifold_guardian_bin, &dir, &["show-mnemonic"]).await?;
            let seats =
                manifold_guardian_admin(manifold_guardian_bin, &dir, &["seats", "list"]).await?;
            upgrade_identities.push((identity, seats));
        }
    }
    let request_id = "guardian-fee-payout-restart-e2e";
    #[cfg(target_os = "linux")]
    let (expected_operation_id, restarted_locators) = {
        let sweep_manifold_guardian_bin = manifold_guardian_bin.to_owned();
        let sweep_dir = mg0_dir.clone();
        let sweep_seat_id = seat_id.clone();
        let sweep = tokio::spawn(async move {
            manifold_guardian_admin(
                &sweep_manifold_guardian_bin,
                &sweep_dir,
                &[
                    "guardian-fees",
                    "sweep",
                    &sweep_seat_id,
                    "--request-id",
                    request_id,
                ],
            )
            .await
        });
        let crash_seam = mg0_dir.join(PAYOUT_CRASH_SEAM_REACHED);
        tokio::time::timeout(Duration::from_secs(30), async {
            while !crash_seam.is_file() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("payout did not reach the native-commit crash seam"))?;
        let mut crashed = daemons.remove(0);
        crashed.start_kill().context("SIGKILL MG during payout")?;
        let status = crashed.wait().await.context("reap payout-crashed MG")?;
        anyhow::ensure!(
            status.signal() == Some(libc::SIGKILL),
            "payout fault injection exited unexpectedly with {status}"
        );
        anyhow::ensure!(
            sweep
                .await
                .context("join interrupted payout admin call")?
                .is_err(),
            "the crash-hidden payout start must not return a linked operation"
        );
        std::fs::remove_file(mg0_dir.join(PAYOUT_CRASH_SEAM_ENABLE))?;
        std::fs::remove_file(crash_seam)?;
        if let Some(candidate) = upgrade_to {
            // Stop the other old guardians before switching the whole federation,
            // just as devimint restart_all_with_bin does. Never re-onboard.
            shutdown_daemons(std::mem::take(daemons)).await?;
            let mut restarted_locators = Vec::new();
            for (index, (old_identity, old_seats)) in upgrade_identities.iter().enumerate() {
                let dir = temp.join(format!("mg-{index}"));
                let mut daemon = spawn_manifold_guardian(
                    candidate,
                    &dir,
                    &bitcoind.rpc_url,
                    &bitcoind.rpc_username,
                    &bitcoind.rpc_password,
                    port_base + u16::try_from(index)? * MG_PORT_STRIDE,
                    Some(iroh_overrides),
                    None,
                    None,
                )?;
                let locator = read_locator(&mut daemon, index).await?;
                restarted_locators.push(locator);
                daemons.push(daemon);
                let identity = manifold_guardian_admin(candidate, &dir, &["show-mnemonic"]).await?;
                anyhow::ensure!(
                    identity == *old_identity,
                    "upgrade changed MG {index} identity"
                );
                let seats = manifold_guardian_admin(candidate, &dir, &["seats", "list"]).await?;
                let before = old_seats["seats"].as_array().context("old release seats")?;
                let after = seats["seats"].as_array().context("upgraded seats")?;
                anyhow::ensure!(
                    before.len() == 1 && after.len() == 1,
                    "upgrade changed MG {index} seat count"
                );
                for field in [
                    "seat_id",
                    "fi_id",
                    "plan",
                    "created_at_ms",
                    "decommissioned",
                    "payment_claim",
                ] {
                    anyhow::ensure!(
                        before[0].get(field).is_some() && before[0][field] == after[0][field],
                        "upgrade changed MG {index} seat field {field}"
                    );
                }
            }
            (None::<String>, restarted_locators)
        } else {
            let mut restarted = spawn_manifold_guardian(
                manifold_guardian_bin,
                &mg0_dir,
                &bitcoind.rpc_url,
                &bitcoind.rpc_username,
                &bitcoind.rpc_password,
                port_base,
                Some(iroh_overrides),
                None,
                None,
            )?;
            let mut restarted_locators = locators.to_vec();
            restarted_locators[0] = read_locator(&mut restarted, 0).await?;
            daemons.insert(0, restarted);
            (None::<String>, restarted_locators)
        }
    };
    #[cfg(not(target_os = "linux"))]
    let (expected_operation_id, restarted_locators) = {
        let started = manifold_guardian_admin(
            manifold_guardian_bin,
            &mg0_dir,
            &[
                "guardian-fees",
                "sweep",
                &seat_id,
                "--request-id",
                request_id,
            ],
        )
        .await?;
        (
            Some(
                started["operation"]["operation_id"]
                    .as_str()
                    .context("payout start commits a native operation")?
                    .to_owned(),
            ),
            locators.to_vec(),
        )
    };
    let manifold_guardian_bin = upgrade_to.unwrap_or(manifold_guardian_bin);
    let terminal = manifold_guardian_admin_with_timeout(
        manifold_guardian_bin,
        &mg0_dir,
        &["payout", "await", request_id],
        Duration::from_secs(120),
    )
    .await?;
    let operation_id = terminal["job"]["operation"]["operation_id"]
        .as_str()
        .context("recovery links the crash-hidden native payout operation")?
        .to_owned();
    anyhow::ensure!(
        terminal["payout"]["state"] == "succeeded"
            && terminal["payout"]["recipient_amount_msat"].as_u64() == Some(PAYOUT_MSAT),
        "restarted MG must finish the exact committed payout: {terminal}"
    );
    if let Some(expected) = expected_operation_id {
        anyhow::ensure!(
            operation_id == expected,
            "payout operation changed after await"
        );
    }
    let replay = manifold_guardian_admin(
        manifold_guardian_bin,
        &mg0_dir,
        &[
            "guardian-fees",
            "sweep",
            &seat_id,
            "--request-id",
            request_id,
        ],
    )
    .await?;
    anyhow::ensure!(
        replay["operation"]["operation_id"] == operation_id,
        "replaying the payout request must return the original operation: {replay}"
    );
    let received = run_fa_payment_wallet(
        fa_cli_bin,
        &wallet_dir,
        &wallet_secret_file,
        &[
            "await-invoice",
            "--payment-federation-id",
            &federation_id,
            "--operation-id",
            &invoice_operation,
        ],
        iroh_overrides,
        Duration::from_secs(60),
    )
    .await?;
    anyhow::ensure!(
        received["state"] == "claimed",
        "real payout recipient must claim the invoice: {received}"
    );
    anyhow::ensure!(
        lnurl.callback_count() == 1,
        "payout recovery must request exactly one invoice, got {} callbacks",
        lnurl.callback_count()
    );
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let status = manifold_guardian_admin(
                manifold_guardian_bin,
                &mg0_dir,
                &["seats", "status", &seat_id],
            )
            .await?;
            if status["report"]["phase"] == "running" && status["report"]["health"] == "healthy" {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("restarted guardian did not become healthy"))??;
    if upgrade_to.is_some() {
        for (index, (_, old_seats)) in upgrade_identities.iter().enumerate() {
            let dir = temp.join(format!("mg-{index}"));
            let seat = old_seats["seats"][0]["seat_id"]
                .as_str()
                .context("old release seat id")?;
            tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let status = manifold_guardian_admin(
                        manifold_guardian_bin,
                        &dir,
                        &["seats", "status", seat],
                    )
                    .await?;
                    if status["report"]["phase"] == "running"
                        && status["report"]["health"] == "healthy"
                    {
                        return Ok::<_, anyhow::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
            .await
            .context("upgraded guardian did not recover consensus")??;
        }
        assert_guardian_fee_policy(manifold_guardian_bin, temp, 5_000).await?;
        let status = manifold_guardian_admin(
            manifold_guardian_bin,
            &mg0_dir,
            &["guardian-fees", "show", &seat_id, "--limit", "1"],
        )
        .await?;
        anyhow::ensure!(
            status["remittance_account"] == empty["remittance_account"]
                && status["lifetime_remitted_msat"] == remitted["lifetime_remitted_msat"],
            "upgrade lost the fee wallet identity or remittance history"
        );
        exercise_guardian_telemetry(
            manifold_guardian_bin,
            temp,
            restarted_locators
                .first()
                .map(String::as_str)
                .context("upgraded MG locator")?,
        )
        .await?;
    }
    // Retain the pre-existing signed MG gateway-registration coverage only
    // after the real payout has finished: its deliberately unreachable
    // fixture URL is itself the consensus LNv2 gateway setting.
    register_gateway_with_every_guardian(state_dir, &restarted_locators).await?;
    defe.release(gateway_lease.handle_id).await?;
    Ok(())
}

async fn exercise_guardian_telemetry(
    manifold_guardian_bin: &Path,
    temp: &Path,
    locator: &str,
) -> anyhow::Result<()> {
    let data_dir = temp.join("mg-0");
    let mnemonic =
        manifold_guardian_admin(manifold_guardian_bin, &data_dir, &["show-mnemonic"]).await?;
    let identity = mg_core::identity::RootMnemonic::parse(
        mnemonic["mnemonic"]
            .as_str()
            .context("show-mnemonic returns the root phrase")?,
    )?;
    let original = identity.derive_telemetry_capability(0);
    let locator: Locator = serde_json::from_str(locator).context("decode MG locator")?;
    let endpoint = Endpoint::bind(presets::N0DisableRelay)
        .await
        .context("bind telemetry collector endpoint")?;

    let connect = || async {
        let connection = endpoint
            .connect(locator.endpoint_addr.clone(), GUARDIAN_TELEMETRY_ALPN)
            .await
            .context("connect to guardian telemetry ALPN")?;
        Ok::<_, anyhow::Error>(GuardianTelemetryApiClient::new(connection))
    };

    let client = connect().await?;
    let unauthorized = match client
        .list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest {
            capability: TelemetryCapability::from_bytes([0xff; 32]),
        })
        .await
    {
        Ok(_) => anyhow::bail!("an unrelated telemetry bearer was accepted"),
        Err(error) => error,
    };
    anyhow::ensure!(
        unauthorized.code() == ServiceErrorCode::PermissionDenied,
        "an unrelated telemetry bearer must be refused as permission denied"
    );
    let listing = connect()
        .await?
        .list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest {
            capability: original.clone(),
        })
        .await?;
    anyhow::ensure!(
        listing.seats.len() == 1 && listing.seats[0].invite_code.is_some(),
        "formed MG telemetry must discover its one formed seat"
    );
    let seat_id = listing.seats[0].seat_id.clone();

    let metrics = connect()
        .await?
        .scrape_guardian_metrics(ScrapeGuardianMetricsRequest {
            seat_id: seat_id.clone(),
            capability: original.clone(),
        })
        .await?;
    anyhow::ensure!(
        metrics.status_code == 200 && !metrics.body.is_empty(),
        "running guardian must expose non-empty metrics"
    );
    // This request reaches the fedimintd embedded in the exact fleet-manager
    // binary selected by the CI runtime bundle. The proxy replays that live
    // response through the shipped default-deny policy before returning it.
    // One surviving sample proves each named histogram had its complete,
    // exact reviewed shape: projection drops the whole family otherwise.
    let metrics_body =
        std::str::from_utf8(&metrics.body).context("guardian metrics are valid UTF-8")?;
    anyhow::ensure!(
        metrics_body.lines().any(|line| {
            line.starts_with("fm_app_start_ts{")
                && line.contains("version=\"0.12.0\"")
                && line.contains("version_hash=\"816687149632f31363f0267e30369f1c9b49314b\"")
        }),
        "the exact bundled 0.12 guardian must expose its release marker"
    );
    for family in [
        "lnv2_funded_contract_sats",
        "lnv2_outgoing_contract_settled_total",
        "mintv2_inout_sats",
        "mintv2_inout_fees_sats",
        "mintv2_redeemed_ecash_sats",
        "mintv2_redeemed_ecash_fees_sats",
        "mintv2_issued_ecash_sats",
        "mintv2_issued_ecash_fees_sats",
        "walletv2_block_count",
        "walletv2_inout_sats",
        "walletv2_inout_fees_sats",
        "walletv2_pegin_sats",
        "walletv2_pegin_fees_sats",
        "walletv2_pegout_sats",
        "walletv2_pegout_fees_sats",
    ] {
        anyhow::ensure!(
            metrics_body
                .lines()
                .any(|line| line.starts_with(&format!("fm_{family}"))),
            "the exact bundled fedi18 guardian must expose fm_{family}"
        );
    }
    let missing_seat = SeatId::new("ff".repeat(32))?;
    let denied_missing_seat = match connect()
        .await?
        .scrape_guardian_metrics(ScrapeGuardianMetricsRequest {
            seat_id: missing_seat.clone(),
            capability: TelemetryCapability::from_bytes([0xff; 32]),
        })
        .await
    {
        Ok(_) => anyhow::bail!("an invalid bearer scraped an unknown seat"),
        Err(error) => error,
    };
    anyhow::ensure!(
        denied_missing_seat.code() == ServiceErrorCode::PermissionDenied,
        "metrics authorization must precede seat selection"
    );
    let unavailable = match connect()
        .await?
        .scrape_guardian_metrics(ScrapeGuardianMetricsRequest {
            seat_id: missing_seat.clone(),
            capability: original.clone(),
        })
        .await
    {
        Ok(_) => anyhow::bail!("an unknown seat selected a metrics port"),
        Err(error) => error,
    };
    anyhow::ensure!(
        unavailable.code() == ServiceErrorCode::Unavailable,
        "an unknown metrics seat must be reported as unavailable"
    );

    let journals = connect()
        .await?
        .list_safe_event_journals(ListSafeEventJournalsRequest {
            capability: original.clone(),
        })
        .await?;
    let [mg_journal, seat_journal] = journals.journals.as_slice() else {
        anyhow::bail!("telemetry did not enumerate exactly the MG and seat journals");
    };
    anyhow::ensure!(
        mg_journal.journal == SafeEventJournal::Mg
            && seat_journal.journal
                == (SafeEventJournal::Seat {
                    seat_id: seat_id.clone(),
                }),
        "telemetry must enumerate the MG and seat journals"
    );
    let mg_incarnation = mg_journal.incarnation.clone();
    let seat_incarnation = seat_journal.incarnation.clone();

    let denied_journals = match connect()
        .await?
        .list_safe_event_journals(ListSafeEventJournalsRequest {
            capability: TelemetryCapability::from_bytes([0xff; 32]),
        })
        .await
    {
        Ok(_) => anyhow::bail!("an invalid bearer enumerated safe-event journals"),
        Err(error) => error,
    };
    anyhow::ensure!(
        denied_journals.code() == ServiceErrorCode::PermissionDenied,
        "journal enumeration must require the MG bearer"
    );
    let batch = connect()
        .await?
        .fetch_safe_event_journal(FetchSafeEventJournalRequest {
            capability: original.clone(),
            journal: SafeEventJournal::Mg,
            incarnation: mg_incarnation.clone(),
            cursor: None,
        })
        .await?;
    let FetchSafeEventJournalResponse::Current {
        incarnation,
        continuity_gap,
        ..
    } = batch
    else {
        anyhow::bail!("fresh MG journal incarnation unexpectedly changed");
    };
    anyhow::ensure!(
        incarnation == mg_incarnation && !continuity_gap,
        "fresh MG journal read must preserve its incarnation and continuity"
    );
    let seat_batch = connect()
        .await?
        .fetch_safe_event_journal(FetchSafeEventJournalRequest {
            capability: original.clone(),
            journal: SafeEventJournal::Seat {
                seat_id: seat_id.clone(),
            },
            incarnation: seat_incarnation.clone(),
            cursor: None,
        })
        .await?;
    let FetchSafeEventJournalResponse::Current {
        incarnation,
        continuity_gap,
        ..
    } = seat_batch
    else {
        anyhow::bail!("fresh seat journal incarnation unexpectedly changed");
    };
    anyhow::ensure!(
        incarnation == seat_incarnation && !continuity_gap,
        "fresh seat journal read must preserve its incarnation and continuity"
    );
    let denied_missing_journal = match connect()
        .await?
        .fetch_safe_event_journal(FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([0xff; 32]),
            journal: SafeEventJournal::Seat {
                seat_id: missing_seat.clone(),
            },
            incarnation: seat_incarnation.clone(),
            cursor: None,
        })
        .await
    {
        Ok(_) => anyhow::bail!("an invalid bearer fetched an unknown seat journal"),
        Err(error) => error,
    };
    anyhow::ensure!(
        denied_missing_journal.code() == ServiceErrorCode::PermissionDenied,
        "journal authorization must precede path selection"
    );
    let missing_journal = match connect()
        .await?
        .fetch_safe_event_journal(FetchSafeEventJournalRequest {
            capability: original.clone(),
            journal: SafeEventJournal::Seat {
                seat_id: missing_seat,
            },
            incarnation: seat_incarnation,
            cursor: None,
        })
        .await
    {
        Ok(_) => anyhow::bail!("an unknown seat journal was returned"),
        Err(error) => error,
    };
    anyhow::ensure!(
        missing_journal.code() == ServiceErrorCode::NotFound,
        "an unknown seat journal must be reported as not found"
    );

    let rotation =
        manifold_guardian_admin(manifold_guardian_bin, &data_dir, &["reenroll-telemetry"]).await?;
    anyhow::ensure!(
        rotation["telemetry_reenrollment"] == "scheduled",
        "operator telemetry rotation must be accepted: {rotation}"
    );
    let revoked = match connect()
        .await?
        .list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest {
            capability: original,
        })
        .await
    {
        Ok(_) => anyhow::bail!("rotation left the old MG-wide bearer authorized"),
        Err(error) => error,
    };
    anyhow::ensure!(
        revoked.code() == ServiceErrorCode::PermissionDenied,
        "the revoked bearer must be refused as permission denied"
    );
    let rotated = identity.derive_telemetry_capability(1);
    let listing = connect()
        .await?
        .list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest {
            capability: rotated,
        })
        .await?;
    anyhow::ensure!(
        listing.seats.len() == 1,
        "rotated bearer must authorize discovery"
    );

    endpoint.close().await;
    Ok(())
}

async fn configure_guardian_fees(
    fa_cli_bin: &Path,
    state_dir: &Path,
    mg_endpoints: &str,
    iroh_overrides: &str,
    send_ppm: u32,
) -> anyhow::Result<()> {
    let mut command = Command::new(fa_cli_bin);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--json")
        .arg("maintenance")
        .arg("--poll-interval-secs")
        .arg("1")
        .arg("--run-timeout-secs")
        .arg("60")
        .arg("--request-timeout-secs")
        .arg("10")
        .arg("configure-guardian-fees")
        .arg("--send-ppm")
        .arg(send_ppm.to_string())
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", iroh_overrides);
    let output = run_expect_success(
        command,
        "fa-cli guardian-fee maintenance",
        Duration::from_secs(70),
    )
    .await?;
    let output: serde_json::Value = serde_json::from_str(output.trim())?;
    anyhow::ensure!(
        output["sendPpm"].as_u64() == Some(u64::from(send_ppm))
            && output["consensusReached"] == true,
        "guardian-fee maintenance must verify exact consensus readback: {output}"
    );
    Ok(())
}

async fn assert_guardian_fee_policy(
    manifold_guardian_bin: &Path,
    temp: &Path,
    send_ppm: u32,
) -> anyhow::Result<()> {
    let checks = (0..GUARDIAN_COUNT).map(|index| async move {
        let data_dir = temp.join(format!("mg-{index}"));
        let seats =
            manifold_guardian_admin(manifold_guardian_bin, &data_dir, &["seats", "list"]).await?;
        let seat_id = seats["seats"][0]["seat_id"]
            .as_str()
            .context("formed seat listing carries its id")?
            .to_owned();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let status = manifold_guardian_admin(
                    manifold_guardian_bin,
                    &data_dir,
                    &["seats", "status", &seat_id],
                )
                .await?;
                let fee = &status["guardian_fee"];
                if fee["send_ppm"].as_u64() == Some(u64::from(send_ppm))
                    && fee["share_matches_policy"] == true
                    && fee["our_weight"].as_u64() == Some(1)
                    && fee["total_weight"].as_u64() == Some(GUARDIAN_COUNT as u64 + 5)
                {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!("guardian {index} did not observe the canonical fee share")
        })??;
        Ok::<_, anyhow::Error>(())
    });
    for check in join_all(checks).await {
        check?;
    }
    Ok(())
}

async fn register_gateway_with_every_guardian(
    state_dir: &Path,
    mg_locators: &[String],
) -> anyhow::Result<()> {
    let status = fa_status(&locate_binary(FA_CLI_BIN_ENV, "fa-cli")?, state_dir).await?;
    let seats = status["formed"]["seats"]
        .as_array()
        .context("formed FA status carries seats")?;
    anyhow::ensure!(
        seats.len() == GUARDIAN_COUNT,
        "formed FA retained every seat"
    );

    let identity =
        std::fs::read(state_dir.join("fi-identity")).context("read the test FA identity")?;
    let fa_root = DerivableSecret::new_root(&identity, b"fedi-fi-cli/root/v1");
    let protocol_key = fa_root
        .child_key(ChildId(0))
        .to_secp_key(&fedimint_core::secp256k1::Secp256k1::new());
    let fa_key = secp256k1::SecretKey::from_byte_array(&protocol_key.secret_key().secret_bytes())
        .context("derive the test FA protocol identity")?;
    let keypair = secp256k1::Keypair::from_secret_key(&secp256k1::Secp256k1::new(), &fa_key);
    let fa_id = FaId(secp256k1::XOnlyPublicKey::from_keypair(&keypair).0);
    let gateway_api = GatewayApiUrl::try_from("https://gateway.example/")
        .context("construct canonical public gateway URL")?;
    let endpoint = Endpoint::bind(presets::N0DisableRelay)
        .await
        .context("bind gateway-registration FA endpoint")?;

    let mg_locators = mg_locators
        .iter()
        .map(|locator| serde_json::from_str::<Locator>(locator))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, seat) in seats.iter().enumerate() {
        // Seats record the advertised endpoint id only; dial the transport
        // address its MG printed, which changes when the MG restarts.
        let seat_locator: Locator = serde_json::from_value(seat["locator"].clone())?;
        let locator = mg_locators
            .iter()
            .find(|locator| locator.endpoint_addr.id == seat_locator.endpoint_addr.id)
            .with_context(|| format!("guardian {index} is not one of this test's MGs"))?
            .clone();
        let seat_id = SeatId::new(
            seat["seat_id"]
                .as_str()
                .with_context(|| format!("guardian {index} status carries seat id"))?,
        )?;
        let connection = endpoint
            .connect(locator.endpoint_addr, MANIFOLD_GUARDIAN_ALPN)
            .await
            .with_context(|| format!("connect to guardian {index} for gateway registration"))?;
        let client = ManifoldGuardianServiceClient::new(connection);
        let request = RegisterGatewayRequest {
            ts: Timestamp(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
            fa_id,
            seat_id,
            gateway_api: gateway_api.clone(),
        };
        let first = register_gateway_when_seat_available(
            &client,
            &request,
            &keypair,
            &format!("guardian {index} accepts gateway registration"),
        )
        .await?;
        anyhow::ensure!(first.was_added, "guardian {index} must add the new gateway");
        let replay = register_gateway_when_seat_available(
            &client,
            &request,
            &keypair,
            &format!("guardian {index} accepts idempotent gateway replay"),
        )
        .await?;
        anyhow::ensure!(
            !replay.was_added,
            "guardian {index} must report an idempotent gateway replay"
        );
    }
    endpoint.close().await;
    Ok(())
}

async fn register_gateway_when_seat_available(
    client: &ManifoldGuardianServiceClient,
    request: &RegisterGatewayRequest,
    keypair: &secp256k1::Keypair,
    context: &str,
) -> anyhow::Result<RegisterGatewayResponse> {
    for _ in 0..300 {
        let response = client
            .transport()
            .register_gateway(SignedRequest::create(request, keypair)?)
            .await
            .with_context(|| format!("{context}: RPC transport"))?;
        match response {
            Ok(response) => return Ok(response),
            Err(ManifoldGuardianError::SeatUnavailable) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => return Err(error).with_context(|| context.to_owned()),
        }
    }
    anyhow::bail!("{context}: seat remained unavailable for 60 seconds")
}

/// Vet the loopback-only Defe gateway through the same per-guardian LNv2
/// admin endpoint used by devimint. Production gateway registration is
/// exercised separately through MG's signed FA endpoint; its public-URL
/// validation intentionally cannot represent this local HTTP fixture.
async fn register_local_gateway_for_lnv2(
    temp: &Path,
    invite: &str,
    iroh_overrides: &str,
    gateway_api_url: &str,
) -> anyhow::Result<()> {
    let fedimint_cli_bin = locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli")?;
    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("guardian-payout-admin"),
        iroh_overrides,
    };
    fedimint_cli
        .run(&["join-federation", invite], FEDIMINT_CLI_JOIN_TIMEOUT)
        .await?;

    let gateway = format!("{}/v1", gateway_api_url.trim_end_matches('/'));
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    for manager in 0..GUARDIAN_COUNT {
        let mg_dir = temp.join(format!("mg-{manager}"));
        let data_dir = mg_dir.join("seats/0/data");
        let local: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(data_dir.join("local.json"))
                .with_context(|| format!("read guardian {manager} local config"))?,
        )?;
        let peer = local["identity"]
            .as_u64()
            .with_context(|| format!("guardian {manager} local config carries its peer id"))?
            .to_string();
        // The guardian's admin credential is derived, never stored: the
        // same derivation MG injects into fedimintd.
        let seat_id: SeatId = manifold_guardian_admin(
            &manifold_guardian_bin,
            &mg_dir,
            &["seats", "list"],
        )
        .await?["seats"][0]["seat_id"]
            .as_str()
            .with_context(|| format!("guardian {manager} seat listing carries its id"))?
            .parse()?;
        let mnemonic = manifold_guardian_admin(&manifold_guardian_bin, &mg_dir, &["show-mnemonic"])
            .await?["mnemonic"]
            .as_str()
            .with_context(|| format!("guardian {manager} show-mnemonic returns the root phrase"))?
            .to_owned();
        let password = mg_core::identity::RootMnemonic::parse(&mnemonic)?
            .derive_seat_keys(&seat_id)
            .api_auth;
        fedimint_cli
            .run(
                &[
                    "--our-id",
                    &peer,
                    "--password",
                    &password,
                    "module",
                    "lnv2",
                    "gateways",
                    "add",
                    &gateway,
                ],
                Duration::from_secs(30),
            )
            .await
            .with_context(|| format!("guardian peer {peer} vets the local LNv2 gateway"))?;
    }
    Ok(())
}

async fn fund_gateway_federation_wallet(
    gateway_cli_bin: &Path,
    gateway: &defe_api::GatewaydInfo,
    bitcoin_cli: &BitcoinCli<'_>,
    miner_address: &str,
    federation_id: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let balances = run_gateway_cli(
                gateway_cli_bin,
                gateway,
                &["get-balances"],
                Duration::from_secs(15),
            )
            .await?;
            if balances["ecash_balances"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|balance| balance["federation_id"] == federation_id)
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("gateway federation wallet did not become ready"))??;

    let deposit = run_gateway_cli(
        gateway_cli_bin,
        gateway,
        &["ecash", "pegin", "--federation-id", federation_id],
        Duration::from_secs(120),
    )
    .await?;
    let address = deposit["address"]
        .as_str()
        .context("gateway ecash pegin returns an address")?;
    bitcoin_cli
        .run(Some(MINER_WALLET), &["sendtoaddress", address, "0.001"])
        .await?;
    bitcoin_cli
        .run(None, &["generatetoaddress", "7", miner_address])
        .await?;

    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let balances = run_gateway_cli(
                gateway_cli_bin,
                gateway,
                &["get-balances"],
                Duration::from_secs(15),
            )
            .await?;
            let funded = balances["ecash_balances"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|balance| {
                    balance["federation_id"] == federation_id
                        && balance["ecash_balance_msats"].as_u64().unwrap_or_default() >= 100_000
                });
            if funded {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("gateway did not claim its federation pegin"))??;
    Ok(())
}

async fn update_federation_name(
    fa_cli_bin: &Path,
    state_dir: &Path,
    mg_endpoints: &str,
    iroh_overrides: &str,
    name: &str,
) -> anyhow::Result<()> {
    let mut command = Command::new(fa_cli_bin);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--json")
        .arg("maintenance")
        .arg("--poll-interval-secs")
        .arg("1")
        .arg("--run-timeout-secs")
        .arg("60")
        .arg("--request-timeout-secs")
        .arg("10")
        .arg("set-name")
        .arg("--value")
        .arg(name)
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", iroh_overrides);
    let output = run_expect_success(
        command,
        "fa-cli metadata maintenance after child replacement",
        Duration::from_secs(70),
    )
    .await?;
    let output: serde_json::Value = serde_json::from_str(output.trim())?;
    anyhow::ensure!(
        output["field"] == "federation_name"
            && output["value"] == name
            && output["consensusReached"] == true,
        "metadata maintenance must verify exact consensus readback: {output}"
    );
    Ok(())
}

/// Form a free federation through fa-cli's verified selection over the MGs
/// advertised on `relay_url`.
async fn form_federation_in_state(
    fa_cli_bin: &Path,
    state_dir: &Path,
    relay_url: &str,
    mg_endpoints: &str,
    iroh_overrides: &str,
) -> anyhow::Result<String> {
    let mut init = Command::new(fa_cli_bin);
    init.arg("--state-dir").arg(state_dir).arg("init");
    run_expect_success(init, "fa-cli init", Duration::from_secs(10)).await?;
    let fa_fee_account_file = write_fa_fee_account_fixture(state_dir)?;
    wait_for_mg_selection(fa_cli_bin, relay_url, mg_endpoints, GUARDIAN_COUNT, 0).await?;

    let mut create = Command::new(fa_cli_bin);
    create
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--json")
        .arg("create")
        .arg("--fa-spv2-account-file")
        .arg(fa_fee_account_file)
        .arg("--federation-size")
        .arg(GUARDIAN_COUNT.to_string())
        .arg("--poll-timeout-secs")
        .arg("120")
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
        .env(
            fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
            relay_url,
        )
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", iroh_overrides);
    let output =
        run_expect_success(create, "fa-cli lifecycle create", Duration::from_secs(130)).await?;
    let output: serde_json::Value = serde_json::from_str(output.trim())?;
    formed_invite(&output)
        .map(ToOwned::to_owned)
        .with_context(|| format!("lifecycle FA must finish formation: {output}"))
}

/// The invite of a formed `fa-cli --json` status, or `None` before formation
/// completes.
fn formed_invite(status: &serde_json::Value) -> Option<&str> {
    status["formed"]["federation_invite"].as_str()
}

async fn wait_for_replacement_child(parent: u32, old_pid: u32) -> anyhow::Result<u32> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(pid) = find_direct_child_named(parent, "fedimintd")
                && pid != old_pid
            {
                return Ok::<_, anyhow::Error>(pid);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("seat loop did not replace killed fedimintd {old_pid}"))?
}

/// Exercise the supported consumer restart path against real MG and
/// fedimintd processes, rather than reproducing the state machine with fake
/// ports. Three killed guardians prevent the first DKG wave from completing;
/// the FA process is then killed after the first idempotent DKG wave crosses
/// the process boundary while its durable guardian-code preparation remains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fa_client_resumes_real_dkg_after_sigkill_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping fa-client crash-recovery E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    if !cfg!(target_os = "linux") {
        eprintln!("skipping fa-client crash-recovery E2E; exact child discovery needs Linux /proc");
        return;
    }

    tokio::time::timeout(FA_CRASH_RECOVERY_TIMEOUT, run_fa_crash_recovery())
        .await
        .expect("fa-client crash-recovery E2E timed out")
        .expect("fa-client crash-recovery E2E failed");
}

async fn run_fa_crash_recovery() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .context("allocate real regtest bitcoind through defe")?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .context("allocate local Nostr relay through defe")?;
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        anyhow::bail!(
            "expected Nostr relay descriptor from defe, got {:?}",
            nostr_relay_lease.descriptor
        );
    };
    let setup_payment_publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();

    let callback_server = CallbackServer::start().await?;
    let temp = mg_e2e_temp_dir()?;
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (mut daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&iroh_overrides),
        GUARDIAN_COUNT,
        Some(NostrEnv {
            relay_urls: &nostr_relay.url,
            holder_relay_url: &nostr_relay.url,
            setup_payment_publisher: &setup_payment_publisher,
        }),
        Some(callback_server.origin()),
    )
    .await;
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;

    let state_dir = temp.join("fi-state");
    let mut init = Command::new(&fa_cli_bin);
    init.arg("--state-dir").arg(&state_dir).arg("init");
    run_expect_success(init, "fa-cli init", Duration::from_secs(10)).await?;
    let fa_fee_account_file = write_fa_fee_account_fixture(&state_dir)?;
    let callback_url_file = state_dir.join("completion-callback-url");
    write_sensitive_file(&callback_url_file, callback_server.callback_url())?;
    let mg_endpoints = mg_endpoint_addrs(&locators);
    wait_for_mg_selection(
        &fa_cli_bin,
        &nostr_relay.url,
        &mg_endpoints,
        GUARDIAN_COUNT,
        0,
    )
    .await?;

    let mut create = Command::new(&fa_cli_bin);
    create
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("create")
        .arg("--fa-spv2-account-file")
        .arg(&fa_fee_account_file)
        .arg("--federation-size")
        .arg(GUARDIAN_COUNT.to_string())
        .arg("--poll-interval-secs")
        .arg("1")
        // The durable lease horizon is this invocation timeout plus 60s.
        .arg("--poll-timeout-secs")
        .arg("15")
        .arg("--completion-callback-url-file")
        .arg(&callback_url_file)
        .arg("--completion-callback-idempotency-key")
        .arg("fa-crash-recovery")
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, &mg_endpoints)
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", &iroh_overrides)
        .env(
            fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
            &nostr_relay.url,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut fi = create.spawn().context("spawn interrupted fa-cli create")?;
    let fa_pid = fi.id().context("newly spawned fa-cli has a process id")?;
    let fa_process = ExactProcess::open_direct_child(std::process::id(), fa_pid, "fa-cli")?;

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let mut all_started = true;
            for index in 0usize..3 {
                all_started &= journal_contains(
                    &temp.join(format!("mg-{index}/safe-events/fman")),
                    "driven DKG start was observed",
                )?;
            }
            if all_started {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for three guardians to start DKG"))??;
    let mut victims = Vec::new();
    for index in 0usize..3 {
        let mg_pid = daemons[index]
            .id()
            .context("Manifold Guardian exited before DKG fault injection")?;
        let fedimintd_pid = find_direct_child_named(mg_pid, "fedimintd")?;
        victims.push((
            index,
            fedimintd_pid,
            ExactProcess::open_direct_child(mg_pid, fedimintd_pid, "fedimintd")?,
        ));
    }
    for (index, fedimintd_pid, fedimintd) in victims {
        fedimintd.signal(libc::SIGKILL)?;
        eprintln!("killed fedimintd {fedimintd_pid} for guardian {index}");
    }

    fa_process.signal(libc::SIGKILL)?;
    let status = fi.wait().await.context("reap killed fa-cli")?;
    anyhow::ensure!(
        status.signal() == Some(libc::SIGKILL),
        "interrupted fa-cli exited unexpectedly with {status}"
    );

    let status = fa_status(&fa_cli_bin, &state_dir).await?;
    anyhow::ensure!(
        status["formation"]["phase"] == "preparing_dkg"
            && status["formation"]["seats"]
                .as_array()
                .is_some_and(|seats| seats.iter().all(|seat| {
                    seat["phase"] == "guardian_code_ready" && seat["guardian_code"].is_string()
                })),
        "killed FA must reopen its durable guardian-code preparation, got {status}"
    );

    // The killed invocation cannot release its lease. The kill follows lease
    // acquisition, so waiting from here covers the full 15s + 60s horizon.
    tokio::time::sleep(Duration::from_secs(76)).await;
    let mut resume = Command::new(&fa_cli_bin);
    resume
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--json")
        .arg("resume")
        .arg("--fa-spv2-account-file")
        .arg(&fa_fee_account_file)
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, &mg_endpoints)
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", &iroh_overrides)
        .env(
            fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
            &nostr_relay.url,
        );
    let resumed = run_expect_success(resume, "fa-cli resume", Duration::from_secs(90)).await?;
    let resumed: serde_json::Value = serde_json::from_str(resumed.trim())?;
    anyhow::ensure!(
        formed_invite(&resumed).is_some(),
        "resumed FA must form and persist an invite, got {resumed}"
    );
    for index in 0..GUARDIAN_COUNT {
        let journal = temp.join(format!("mg-{index}/safe-events/fman"));
        let starts = journal_message_count(&journal, "driven DKG start was observed")?;
        let minimum_starts = if index < 3 { 2 } else { 1 };
        anyhow::ensure!(
            starts >= minimum_starts,
            "guardian {index} observed only {starts} DKG start wave(s), expected at least {minimum_starts}"
        );
    }

    wait_for_callbacks_pending_after_attempt(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    callback_server.wait_for_requests(GUARDIAN_COUNT).await?;
    anyhow::ensure!(
        callback_server
            .idempotency_keys()
            .iter()
            .all(|key| key == "fa-crash-recovery"),
        "every callback attempt must retain the FA's stable idempotency key"
    );

    // Preserve the pending row across one whole MG process restart before
    // allowing delivery, matching the manual retry/restart boundary.
    let daemon0 = daemons.remove(0);
    shutdown_daemons(vec![daemon0]).await?;
    let mut daemon0 = spawn_manifold_guardian(
        &manifold_guardian_bin,
        &temp.join("mg-0"),
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base,
        Some(&iroh_overrides),
        None,
        Some(callback_server.origin()),
    )?;
    read_locator(&mut daemon0, 0).await?;
    daemons.insert(0, daemon0);

    callback_server.allow_success();
    wait_for_callbacks_delivered(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    callback_server
        .wait_for_requests(GUARDIAN_COUNT * 2)
        .await?;
    anyhow::ensure!(
        callback_server
            .idempotency_keys()
            .iter()
            .all(|key| key == "fa-crash-recovery"),
        "callback retries must retain the FA's stable idempotency key"
    );

    shutdown_daemons(daemons).await?;
    defe.release(nostr_relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove crash-recovery E2E tempdir")?;
    Ok(())
}

/// Check the deployed composition rather than only either journal crate in
/// isolation: every MG and the fedimintd embedded in it must reach their
/// distinct on-disk sinks during a real formation.
fn assert_safe_event_journals(temp: &Path, guardian_count: usize) -> anyhow::Result<()> {
    for index in 0..guardian_count {
        let mg_root = temp.join(format!("mg-{index}"));
        assert_safe_event_journal(
            &mg_root.join("safe-events/fman"),
            &[
                "created a new Manifold Guardian identity",
                "driven DKG start was observed",
                "driven DKG configuration is durable",
            ],
        )?;
        assert_safe_event_journal(
            &mg_root.join("seats/0/safe-events"),
            &[
                "Starting fedimintd",
                "All connection-code checksums agree",
                "Module config generation completed",
                "Config generation has completed successfully",
            ],
        )?;
    }
    Ok(())
}

fn assert_safe_event_journal(directory: &Path, expected_messages: &[&str]) -> anyhow::Result<()> {
    let mode = std::fs::metadata(directory)
        .with_context(|| format!("stat journal directory {}", directory.display()))?
        .permissions()
        .mode()
        & 0o777;
    anyhow::ensure!(
        mode == 0o700,
        "journal directory {} has mode {mode:o}, expected 700",
        directory.display()
    );

    let mut segments = std::fs::read_dir(directory)
        .with_context(|| format!("read journal directory {}", directory.display()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("events-") && name.ends_with(".jsonl"))
        })
        .collect::<Vec<_>>();
    segments.sort_by_key(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .strip_prefix("events-")
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|number| number.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
    anyhow::ensure!(
        !segments.is_empty() && segments.len() <= 2,
        "journal {} retains one or two segments, got {}",
        directory.display(),
        segments.len()
    );

    let mut found_expected = vec![false; expected_messages.len()];
    let mut event_count = 0usize;
    for segment in segments {
        let metadata = segment.metadata()?;
        let segment_mode = metadata.permissions().mode() & 0o777;
        anyhow::ensure!(
            segment_mode == 0o600,
            "journal segment {} has mode {segment_mode:o}, expected 600",
            segment.path().display()
        );
        anyhow::ensure!(
            metadata.len() <= 5 * 1024 * 1024 / 2,
            "journal segment {} exceeds 2.5 MiB",
            segment.path().display()
        );
        let contents = std::fs::read_to_string(segment.path())?;
        anyhow::ensure!(contents.ends_with('\n'), "journal has an incomplete tail");
        for line in contents.lines() {
            let event: serde_json::Value = serde_json::from_str(line).with_context(|| {
                format!("parse journal event from {}", segment.path().display())
            })?;
            anyhow::ensure!(
                event["fields"]["safe_to_share"] == true,
                "journal admitted an event without typed safe_to_share=true"
            );
            anyhow::ensure!(
                event.get("span").is_none() && event.get("spans").is_none(),
                "journal event inherited span context"
            );
            if let Some(message) = event["fields"]["message"].as_str() {
                for (found, expected) in found_expected.iter_mut().zip(expected_messages) {
                    *found |= message.contains(expected);
                }
            }
            event_count += 1;
        }
    }
    anyhow::ensure!(event_count > 0, "journal contains no events");
    let missing = expected_messages
        .iter()
        .zip(found_expected)
        .filter_map(|(message, found)| (!found).then_some(*message))
        .collect::<Vec<_>>();
    anyhow::ensure!(missing.is_empty(), "journal is missing events {missing:?}");
    eprintln!(
        "safe-event journal verified: {} ({event_count} valid events, expected markers found)",
        directory.display()
    );
    Ok(())
}

fn journal_contains(directory: &Path, message: &str) -> anyhow::Result<bool> {
    Ok(journal_message_count(directory, message)? > 0)
}

fn journal_message_count(directory: &Path, message: &str) -> anyhow::Result<usize> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("events-") && name.ends_with(".jsonl"))
        {
            continue;
        }
        // The last line may still be in flight while the daemon is live. Only
        // count complete, valid journal records and let the next poll see it.
        let contents = std::fs::read_to_string(entry.path())?;
        for line in contents.lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            count += usize::from(
                event["fields"]["message"]
                    .as_str()
                    .is_some_and(|candidate| candidate.contains(message)),
            );
        }
    }
    Ok(count)
}

/// A Linux process identity pinned against PID reuse before fault injection.
struct ExactProcess {
    pid: u32,
    #[cfg(target_os = "linux")]
    pidfd: OwnedFd,
}

impl ExactProcess {
    #[cfg(target_os = "linux")]
    fn open_named(pid: u32, expected_name: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(pid > 1, "refusing to pin unsafe pid {pid}");
        // SAFETY: pidfd_open takes no pointers. A successful nonnegative fd is
        // newly owned by this call and is transferred exactly once to OwnedFd.
        let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if raw_fd < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("pidfd_open({pid})"));
        }
        let raw_fd = i32::try_from(raw_fd).context("pidfd fits a raw file descriptor")?;
        // SAFETY: the successful syscall returned a fresh owned descriptor.
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let process = Self { pid, pidfd };
        // `Command::spawn` may return while the forked child still exposes
        // its parent's argv, just before exec installs the requested binary.
        // The pidfd already pins the process identity, so wait briefly for
        // that exec boundary rather than misclassifying the right child.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let actual_name = process_name(pid);
            if actual_name.as_deref() == Some(expected_name) {
                break;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "pinned pid {pid} is {actual_name:?}, not {expected_name}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(process)
    }

    #[cfg(not(target_os = "linux"))]
    fn open_named(_pid: u32, _expected_name: &str) -> anyhow::Result<Self> {
        anyhow::bail!("pidfd fault injection requires Linux")
    }

    fn open_direct_child(parent: u32, pid: u32, expected_name: &str) -> anyhow::Result<Self> {
        let process = Self::open_named(pid, expected_name)?;
        anyhow::ensure!(
            is_named_direct_child(parent, pid, expected_name),
            "pinned {expected_name} pid {pid} is not a direct child of {parent}"
        );
        Ok(process)
    }

    #[cfg(target_os = "linux")]
    fn signal(&self, signal: i32) -> anyhow::Result<()> {
        // SAFETY: pidfd_send_signal receives our live descriptor, a normal
        // signal number, no siginfo pointer, and zero flags.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        anyhow::ensure!(
            result == 0,
            "pidfd_send_signal({}, {signal}): {}",
            self.pid,
            std::io::Error::last_os_error()
        );
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn signal(&self, _signal: i32) -> anyhow::Result<()> {
        anyhow::bail!("pidfd fault injection requires Linux")
    }
}

/// Find the named process among every task's direct children. Tokio may spawn
/// a child from a dedicated task thread, so `/proc/<pid>/task/<tid>/children`
/// is the authoritative Linux relationship rather than the main thread alone.
fn find_direct_child_named(parent: u32, expected_name: &str) -> anyhow::Result<u32> {
    anyhow::ensure!(parent > 1, "refusing to inspect unsafe parent pid {parent}");
    let task_dir = PathBuf::from(format!("/proc/{parent}/task"));
    for task in
        std::fs::read_dir(&task_dir).with_context(|| format!("read {}", task_dir.display()))?
    {
        let children_path = task?.path().join("children");
        let Ok(children) = std::fs::read_to_string(&children_path) else {
            continue;
        };
        for child in children.split_whitespace() {
            let child = child
                .parse::<u32>()
                .with_context(|| format!("parse child pid from {}", children_path.display()))?;
            if child <= 1 {
                continue;
            }
            let cmdline = std::fs::read(format!("/proc/{child}/cmdline"))?;
            let argv0 = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
            let name = Path::new(std::ffi::OsStr::from_bytes(argv0))
                .file_name()
                .and_then(|name| name.to_str());
            if name == Some(expected_name) {
                return Ok(child);
            }
        }
    }
    anyhow::bail!("pid {parent} has no direct child named {expected_name}")
}

fn process_name(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
    Path::new(std::ffi::OsStr::from_bytes(argv0))
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}

fn is_named_direct_child(parent: u32, child: u32, expected_name: &str) -> bool {
    if parent <= 1 || child <= 1 {
        return false;
    }
    let task_dir = PathBuf::from(format!("/proc/{parent}/task"));
    let Ok(tasks) = std::fs::read_dir(task_dir) else {
        return false;
    };
    let linked = tasks.filter_map(Result::ok).any(|task| {
        std::fs::read_to_string(task.path().join("children"))
            .ok()
            .is_some_and(|children| {
                children
                    .split_whitespace()
                    .any(|candidate| candidate.parse::<u32>() == Ok(child))
            })
    });
    if !linked {
        return false;
    }
    let Ok(cmdline) = std::fs::read(format!("/proc/{child}/cmdline")) else {
        return false;
    };
    let argv0 = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
    Path::new(std::ffi::OsStr::from_bytes(argv0))
        .file_name()
        .and_then(|name| name.to_str())
        == Some(expected_name)
}

async fn fa_status(fa_cli_bin: &Path, state_dir: &Path) -> anyhow::Result<serde_json::Value> {
    let mut command = Command::new(fa_cli_bin);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--json")
        .arg("status");
    let output = run_expect_success(command, "fa-cli status", Duration::from_secs(10)).await?;
    serde_json::from_str(output.trim()).context("parse fa-cli status JSON")
}

struct CallbackServer {
    origin: String,
    callback_url: String,
    allow_success: Arc<AtomicBool>,
    idempotency_keys: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl CallbackServer {
    async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}/");
        let callback_url = format!("{origin}hooks/e2e-hook/e2e-token");
        let allow_success = Arc::new(AtomicBool::new(false));
        let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
        let success = Arc::clone(&allow_success);
        let keys = Arc::clone(&idempotency_keys);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let success = Arc::clone(&success);
                let keys = Arc::clone(&keys);
                tokio::spawn(async move {
                    let _ = handle_callback_connection(stream, &success, &keys).await;
                });
            }
        });
        Ok(Self {
            origin,
            callback_url,
            allow_success,
            idempotency_keys,
            task,
        })
    }

    fn origin(&self) -> &str {
        &self.origin
    }

    fn callback_url(&self) -> &str {
        &self.callback_url
    }

    fn allow_success(&self) {
        self.allow_success.store(true, Ordering::SeqCst);
    }

    fn idempotency_keys(&self) -> Vec<String> {
        self.idempotency_keys
            .lock()
            .expect("callback request mutex is not poisoned")
            .clone()
    }

    async fn wait_for_requests(&self, minimum: usize) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                if self
                    .idempotency_keys
                    .lock()
                    .expect("callback request mutex is not poisoned")
                    .len()
                    >= minimum
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for {minimum} callback requests"))
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_callback_connection(
    mut stream: TcpStream,
    allow_success: &AtomicBool,
    idempotency_keys: &Mutex<Vec<String>>,
) -> anyhow::Result<()> {
    let mut request = Vec::new();
    let (header_end, content_length) = loop {
        anyhow::ensure!(request.len() <= 16 * 1024, "callback request is too large");
        let mut chunk = [0u8; 2048];
        let read = stream.read(&mut chunk).await?;
        anyhow::ensure!(read > 0, "callback connection closed before request body");
        request.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = header_end + 4;
            let headers = std::str::from_utf8(&request[..header_end])?;
            anyhow::ensure!(
                headers.lines().next() == Some("POST /hooks/e2e-hook/e2e-token HTTP/1.1"),
                "callback used an unexpected HTTP method, path, or version"
            );
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .context("callback request has Content-Length")?;
            if request.len() >= header_end + content_length {
                break (header_end, content_length);
            }
        }
    };
    let body: serde_json::Value =
        serde_json::from_slice(&request[header_end..header_end + content_length])?;
    let key = body["idempotency_key"]
        .as_str()
        .context("callback body carries idempotency_key")?;
    idempotency_keys
        .lock()
        .expect("callback request mutex is not poisoned")
        .push(key.to_owned());

    let status = if allow_success.load(Ordering::SeqCst) {
        "200 OK"
    } else {
        "500 Internal Server Error"
    };
    stream
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    Ok(())
}

async fn wait_for_callbacks_delivered(
    manifold_guardian_bin: &Path,
    temp: &Path,
    guardian_count: usize,
) -> anyhow::Result<()> {
    // Automatic guardian replay can finish formation while the FA's 76-second
    // lease is still held. Callbacks start failing against the deliberately
    // unavailable gateway then, so their durable backoff may already be at
    // 60-120 seconds by the time this test allows delivery. Wait for that
    // scheduled retry; do not weaken the requirement that every seat delivers.
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let mut delivered = true;
            for index in 0..guardian_count {
                let seats = manifold_guardian_admin(
                    manifold_guardian_bin,
                    &temp.join(format!("mg-{index}")),
                    &["seats", "list"],
                )
                .await?;
                delivered &= seats["seats"][0]["completion_callback"]["state"] == "delivered";
            }
            if delivered {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for every callback to be delivered"))?
}

async fn wait_for_callbacks_pending_after_attempt(
    manifold_guardian_bin: &Path,
    temp: &Path,
    guardian_count: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let mut all_attempted = true;
            for index in 0..guardian_count {
                let seats = manifold_guardian_admin(
                    manifold_guardian_bin,
                    &temp.join(format!("mg-{index}")),
                    &["seats", "list"],
                )
                .await?;
                let callback = &seats["seats"][0]["completion_callback"];
                all_attempted &= callback["state"] == "pending"
                    && callback["attempts"]
                        .as_u64()
                        .is_some_and(|attempts| attempts >= 1)
                    && callback["last_reason"] == "gateway_unavailable";
            }
            if all_attempted {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for every callback's first HTTP 500"))?
}

/// Check the artifact DKG made immutable, rather than merely the environment
/// and registry the composition root intended to pass to `fedimintd`.
fn assert_formation_has_v2_module_set(temp: &Path, guardian_count: usize) -> anyhow::Result<()> {
    let stability_pool = stability_pool_common::KIND;
    let expected = BTreeSet::from([
        "lnv2",
        "meta",
        "mintv2",
        stability_pool.as_str(),
        "walletv2",
    ]);
    for index in 0..guardian_count {
        let path = temp
            .join(format!("mg-{index}"))
            .join("seats/0/data/consensus.json");
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        let modules = config["modules"]
            .as_object()
            .with_context(|| format!("{} carries a module map", path.display()))?;
        let actual = modules
            .values()
            .filter_map(|module| module["kind"].as_str())
            .collect::<BTreeSet<_>>();
        anyhow::ensure!(
            actual == expected,
            "{} committed the wrong module set: expected {expected:?}, got {actual:?}",
            path.display(),
        );
    }
    Ok(())
}

/// The paid gate: seven MGs first form a free federation, which then serves as
/// the ecash *payment federation* for a second, paid formation — proving the
/// whole money path at product-valid scale: wallet join, on-chain
/// deposit, quote-bound foreign mint outputs, MG claim, and refund. The
/// payment federation carries a mintv2 module, so the (never negotiated)
/// generation both sides derive from it is mintv2; the mintv1 path keeps
/// its coverage in the fleet-manager unit and wallet tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifold_guardian_0_1_paid_formation_settles_real_ecash_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping Manifold Guardian 0.1 paid E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }

    tokio::time::timeout(PAID_FORMATION_TIMEOUT, run_paid_formation())
        .await
        .expect("Manifold Guardian 0.1 paid E2E timed out")
        .expect("Manifold Guardian 0.1 paid E2E failed");
}

async fn run_paid_formation() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")
        .unwrap_or_else(|err| panic!("{err}"));
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli").unwrap_or_else(|err| panic!("{err}"));
    let fedimint_cli_bin =
        locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli").unwrap_or_else(|err| panic!("{err}"));
    let bitcoin_cli_bin =
        locate_binary(BITCOIN_CLI_BIN_ENV, "bitcoin-cli").unwrap_or_else(|err| panic!("{err}"));

    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe from env; run under `defe exec` or a persistent defe server");
    let bitcoind_lease = defe
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .expect("allocate real regtest bitcoind through defe");
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        panic!(
            "expected bitcoind descriptor from defe, got {:?}",
            bitcoind_lease.descriptor
        );
    };
    // The MGs learn the accepted setup-payment set only from the signed
    // kind-37707 publication on this relay (SPEC-setup-payment-federations);
    // there is no operator-side acceptance verb.
    let nostr_relay_lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .expect("allocate Nostr relay through defe");
    let ResourceDescriptor::NostrRelay(nostr_relay) = &nostr_relay_lease.descriptor else {
        panic!(
            "expected Nostr relay descriptor from defe, got {:?}",
            nostr_relay_lease.descriptor
        );
    };
    let setup_payment_keys =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?;
    let setup_payment_publisher = setup_payment_keys.public_key().to_string();

    let temp = mg_e2e_temp_dir()?;
    eprintln!("paid E2E data dir: {}", temp.display());

    // The exclusive Defe node already has 101 blocks and mature wallet funds.
    // Reuse them rather than creating another wallet and mining 101 more blocks.
    let bitcoin_cli = BitcoinCli::new(&bitcoin_cli_bin, bitcoind)?;
    let miner_address = bitcoin_cli.funded_miner_address().await?;

    // max-seats 2: seat 1 forms the payment federation, seat 2 the paid one.
    let port_base = reserve_mg_port_grid(PAID_GUARDIAN_COUNT)?;
    let iroh_overrides = local_iroh_overrides_for_grid(port_base, 2, PAID_GUARDIAN_COUNT);
    let nostr = NostrEnv {
        relay_urls: &nostr_relay.url,
        holder_relay_url: &nostr_relay.url,
        setup_payment_publisher: &setup_payment_publisher,
    };
    let (mut daemons, locators) = start_daemons(
        &manifold_guardian_bin,
        &temp,
        bitcoind,
        2,
        port_base,
        Some(&iroh_overrides),
        PAID_GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;

    offer_free_seats(&manifold_guardian_bin, &temp, PAID_GUARDIAN_COUNT).await?;

    eprintln!("forming the payment federation (given away)");
    let mg_endpoints = mg_endpoint_addrs(&locators);
    let payment_invite = run_fa_cli(
        &fa_cli_bin,
        &nostr_relay.url,
        &mg_endpoints,
        FaCliInvocation {
            extra_args: &[],
            resume_args: None,
            wallet_secret: None,
            output: FaCliOutput::Human,
            seat_price_msats: 0,
        },
        Some(&iroh_overrides),
        PAID_GUARDIAN_COUNT,
        FA_CLI_TIMEOUT,
    )
    .await?;
    let payment_invite = payment_invite.trim().to_owned();

    // Publish the common setup-payment set naming the freshly formed
    // federation; every MG admits it from the relay and joins in its
    // wallet — there is no per-MG acceptance step.
    NostrRelayClient::connect(
        &nostr_relay.url,
        setup_payment_keys.clone(),
        Duration::from_secs(10),
    )
    .await
    .map_err(|err| anyhow::anyhow!("connect setup-payment publisher to relay: {err}"))?
    .publish_event(
        EventBuilder::new(
            Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
            serde_json::json!({
                "version": 1,
                "fman_version": "0.1.0",
                "federations": [payment_invite],
                "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
            })
            .to_string(),
        )
        .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
    )
    .await
    .map_err(|err| anyhow::anyhow!("publish setup-payment federations event: {err}"))?;

    // Every MG now offers only the paid plan and, once its refresh poll
    // admits the publication, accepts OOB ecash in the federation its own
    // first seat helps run.
    let configured = join_all((0..PAID_GUARDIAN_COUNT).map(|index| {
        let data_dir = temp.join(format!("mg-{index}"));
        let manifold_guardian_bin = &manifold_guardian_bin;
        async move {
            let price = SEAT_PRICE_MSAT.to_string();
            manifold_guardian_admin(
                manifold_guardian_bin,
                &data_dir,
                &["plans", "set", "--price-msats", &price],
            )
            .await?;
            let federation_id =
                wait_for_accepted_payment_federation(manifold_guardian_bin, &data_dir).await?;
            eprintln!("fleet-manager {index} accepts payments in {federation_id}");
            Ok::<_, anyhow::Error>(federation_id)
        }
    }))
    .await;

    let mut configured = configured.into_iter();
    let payment_federation_id = configured
        .next()
        .context("paid formation requires at least one guardian")??;
    for federation_id in configured {
        let federation_id = federation_id?;
        anyhow::ensure!(
            payment_federation_id == federation_id,
            "MGs disagree about the payment federation id"
        );
    }

    // FA-side money: peg in regtest coin, then move one real OOB token into
    // the same in-process wallet that fa-cli uses for quote-bound payments.
    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fa-wallet"),
        iroh_overrides: &iroh_overrides,
    };
    fedimint_cli
        .run(
            &["join-federation", &payment_invite],
            FEDIMINT_CLI_JOIN_TIMEOUT,
        )
        .await?;
    // The MG processes and their fedimintd children inherited these direct
    // Iroh routes before forming the payment federation, so their wallets can
    // use the bare-node-id invite without a restart.
    // WalletV2 deliberately ignores blocks predating the federation. Wait for
    // its first non-zero consensus height before creating an output that it
    // must discover. Its consensus height intentionally trails Bitcoin's tip
    // by the finality window, so waiting for the current tip cannot complete.
    loop {
        let block_count = fedimint_cli
            .run_json(
                &["module", "walletv2", "info", "block-count"],
                FEDIMINT_CLI_WALLETV2_TIMEOUT,
            )
            .await?;
        if block_count.as_u64().is_some_and(|count| count >= 1) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // Read the event log position before creating the address, so the wait
    // below sees the receive that this address produces.
    let receive_position = fedimint_cli
        .run_json(&["dev", "next-event-log-id"], FEDIMINT_CLI_WALLETV2_TIMEOUT)
        .await?
        .as_u64()
        .context("next-event-log-id returns a number")?
        .to_string();
    let deposit_address = fedimint_cli
        .run_json(
            &["module", "walletv2", "receive"],
            FEDIMINT_CLI_WALLETV2_TIMEOUT,
        )
        .await?
        .as_str()
        .context("walletv2 receive returns an address")?
        .to_owned();
    bitcoin_cli
        .run(
            Some(MINER_WALLET),
            &["sendtoaddress", &deposit_address, "0.001"],
        )
        .await?;
    // Match upstream's WalletV2 peg-in window: the containing block plus its
    // six-block confirmation finality delay.
    bitcoin_cli
        .run(None, &["generatetoaddress", "7", &miner_address])
        .await?;
    eprintln!("waiting for the peg-in to confirm");
    // Keep one client process alive while WalletV2 finds the next valid
    // receive index and claims the output. Valid indices occur only about once
    // per 65,536 candidates; restarting a short-lived client repeats that CPU
    // search from the same persisted index forever. `await-receive` returns
    // once the claim succeeds and its ecash is issued.
    fedimint_cli
        .run(
            &["module", "walletv2", "await-receive", &receive_position],
            FEDIMINT_CLI_WALLETV2_TIMEOUT,
        )
        .await?;
    let info = fedimint_cli
        .run_json(&["info"], Duration::from_secs(10))
        .await?;
    let balance = info["total_amount_msat"]
        .as_u64()
        .context("fedimint-cli info reports total_amount_msat")?;
    anyhow::ensure!(
        balance >= FA_FUNDING_MSAT,
        "WalletV2 peg-in credited only {balance} msat"
    );

    let funding = fedimint_cli
        .run_json(
            &["module", "mintv2", "send", &FA_FUNDING_MSAT.to_string()],
            Duration::from_secs(10),
        )
        .await?;
    let funding = funding
        .as_str()
        .context("mint-v2 send returns encoded ecash")?;
    let funding_ecash: fedimint_mintv2_client::ECash =
        fedimint_core::base32::decode_prefixed(fedimint_core::base32::FEDIMINT_PREFIX, funding)
            .context("decode the FA's focused funding token")?;
    anyhow::ensure!(
        funding_ecash.amount().msats == FA_FUNDING_MSAT && funding_ecash.notes().len() == 1,
        "focused payer funding must contain one {FA_FUNDING_MSAT} msat ecash note"
    );

    // Exercise the production wallet's lost-response recovery against the real
    // mint-v2 operation log before handing the FA its separate funding token.
    let replay_token = fedimint_cli
        .run_json(
            &["module", "mintv2", "send", "1024"],
            Duration::from_secs(10),
        )
        .await?;
    let replay_token = replay_token
        .as_str()
        .context("mint-v2 send returns replay-test ecash")?;
    let replay_token_file = temp.join("mint-v2-replay-token");
    write_sensitive_file(&replay_token_file, replay_token)?;
    let mut replay_command = Command::new(env::current_exe()?);
    replay_command
        .args([
            "--exact",
            "mint_v2_receive_replay_child",
            "--ignored",
            "--nocapture",
        ])
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", &iroh_overrides)
        .env(REPLAY_INVITE_ENV, &payment_invite)
        .env(REPLAY_TOKEN_FILE_ENV, &replay_token_file)
        .env(
            REPLAY_WALLET_DIR_ENV,
            temp.join("fman-mint-v2-replay-wallet"),
        );
    replay_command.kill_on_drop(true);
    let replay = tokio::time::timeout(MINT_V2_REPLAY_TIMEOUT, replay_command.output())
        .await
        .context("mint-v2 lost-response replay child timed out")??;
    anyhow::ensure!(
        replay.status.success(),
        "mint-v2 lost-response replay child failed: {}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let fa_wallet_dir = temp.join("fa-locked-payment-wallet");
    let funding_token_file = temp.join("fi-funding-token");
    write_sensitive_file(&funding_token_file, funding)?;

    let fa_wallet_secret = hex::encode(FA_WALLET_SECRET);
    let setup_payment_event = EventBuilder::new(
        Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
        serde_json::json!({
            "version": 1,
            "fman_version": "0.1.0",
            "federations": [payment_invite],
            "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
        })
        .to_string(),
    )
    .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG))
    .sign_with_keys(&setup_payment_keys)?;
    let setup_payment_event_file = temp.join("setup-payment-event.json");
    std::fs::write(
        &setup_payment_event_file,
        serde_json::to_vec(&setup_payment_event)?,
    )?;
    let fa_cli_paid_args = vec![
        "--setup-payment-event-file".to_owned(),
        setup_payment_event_file.display().to_string(),
        "--setup-payment-publisher".to_owned(),
        setup_payment_keys.public_key().to_string(),
        "--payment-federation-id".to_owned(),
        payment_federation_id.clone(),
        "--wallet-data-dir".to_owned(),
        fa_wallet_dir.display().to_string(),
        "--payment-invite-code".to_owned(),
        payment_invite.clone(),
        "--funding-token-file".to_owned(),
        funding_token_file.display().to_string(),
    ];
    let fa_cli_paid_resume_args = fa_cli_paid_args[..fa_cli_paid_args.len() - 2].to_vec();

    eprintln!("forming the paid federation");
    let paid_invite = run_fa_cli(
        &fa_cli_bin,
        &nostr_relay.url,
        &mg_endpoints,
        FaCliInvocation {
            extra_args: &fa_cli_paid_args,
            resume_args: Some(&fa_cli_paid_resume_args),
            wallet_secret: Some(&fa_wallet_secret),
            output: FaCliOutput::JsonContract,
            seat_price_msats: SEAT_PRICE_MSAT,
        },
        Some(&iroh_overrides),
        PAID_GUARDIAN_COUNT,
        Duration::from_secs(480),
    )
    .await?;
    anyhow::ensure!(
        !funding_token_file.exists(),
        "fa-cli did not delete its funding token file"
    );
    let wallet_secret_file = temp.join("fa-wallet-secret-balance");
    write_sensitive_file(&wallet_secret_file, &fa_wallet_secret)?;
    let mut accounting_command = Command::new(&fa_cli_bin);
    accounting_command
        .arg("--json")
        .arg("payment-wallet")
        .arg("--wallet-data-dir")
        .arg(&fa_wallet_dir)
        .arg("--wallet-secret-file")
        .arg(&wallet_secret_file)
        .arg("accounting")
        .arg("--payment-federation-id")
        .arg(&payment_federation_id)
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", &iroh_overrides);
    let accounting: serde_json::Value = serde_json::from_str(
        &run_expect_success(
            accounting_command,
            "fa-cli reopened payment-wallet accounting",
            Duration::from_secs(30),
        )
        .await?,
    )?;
    std::fs::remove_file(&wallet_secret_file).context("remove balance wallet secret")?;
    let final_balance_msat = accounting["balanceMsats"]
        .as_u64()
        .context("payment-wallet accounting reports balanceMsats")?;
    let received_input_msat = accounting["receivedInputMsats"]
        .as_u64()
        .context("payment-wallet accounting reports receivedInputMsats")?;
    let receive_fee_msat = accounting["receiveFeeMsats"]
        .as_u64()
        .context("payment-wallet accounting reports receiveFeeMsats")?;
    let observed_setup_payments_msat = accounting["setupOutputMsats"]
        .as_u64()
        .context("payment-wallet accounting reports setupOutputMsats")?;
    let setup_fee_msat = accounting["setupFeeMsats"]
        .as_u64()
        .context("payment-wallet accounting reports setupFeeMsats")?;
    let setup_transaction_count = accounting["setupTransactionCount"]
        .as_u64()
        .context("payment-wallet accounting reports setupTransactionCount")?;
    let setup_payments_msat = SEAT_PRICE_MSAT
        .checked_mul(u64::try_from(PAID_GUARDIAN_COUNT)?)
        .context("setup payment total overflow")?;
    anyhow::ensure!(
        received_input_msat == FA_FUNDING_MSAT,
        "reopened operation log did not recover the exact one-note funding input"
    );
    anyhow::ensure!(
        observed_setup_payments_msat == setup_payments_msat
            && setup_transaction_count == u64::try_from(PAID_GUARDIAN_COUNT)?,
        "reopened operation log did not recover exactly one setup payment per MG"
    );
    let total_fees_msat = receive_fee_msat
        .checked_add(setup_fee_msat)
        .context("Fedimint fee accounting overflow")?;
    anyhow::ensure!(
        final_balance_msat > 0 && total_fees_msat > 0,
        "one-note payer must retain returned change and account for positive Fedimint fees"
    );
    let independently_observed_total = final_balance_msat
        .checked_add(observed_setup_payments_msat)
        .and_then(|total| total.checked_add(receive_fee_msat))
        .and_then(|total| total.checked_add(setup_fee_msat))
        .context("one-note payer accounting overflow")?;
    anyhow::ensure!(
        independently_observed_total == received_input_msat,
        "one-note payer balance does not reconcile"
    );
    eprintln!(
        "one-note payer accounting: funding={received_input_msat} \
         payments={observed_setup_payments_msat} receive_fees={receive_fee_msat} \
         setup_fees={setup_fee_msat} returned_change={final_balance_msat} msat"
    );
    let joiner = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("paid-federation-client"),
        iroh_overrides: &iroh_overrides,
    };
    joiner
        .run(
            &["join-federation", paid_invite.trim()],
            FEDIMINT_CLI_JOIN_TIMEOUT,
        )
        .await?;

    // Close the acceptance money loop on MG 0: CreateSeat returned before
    // its background claim, so wait for the real wallet credit.
    let mg0_dir = temp.join("mg-0");
    let balance_msat = wait_for_mg_balance(&manifold_guardian_bin, &mg0_dir).await?;
    // The mint charges a base fee of 10 msat per transaction input and
    // output, so reissuing the note credits somewhat less than its face
    // value; half the price is a generous fee allowance.
    anyhow::ensure!(
        balance_msat >= SEAT_PRICE_MSAT / 2,
        "MG 0 wallet balance {balance_msat} msat is not a plausible \
         fee-reduced seat payment of {SEAT_PRICE_MSAT} msat"
    );
    exercise_removed_payment_federation_sweeps(
        &mut defe,
        bitcoind,
        &manifold_guardian_bin,
        &fa_cli_bin,
        &bitcoin_cli,
        &miner_address,
        &temp,
        port_base,
        &iroh_overrides,
        nostr,
        &setup_payment_keys,
        &payment_invite,
        &payment_federation_id,
        paid_invite.trim(),
        &mut daemons,
    )
    .await?;
    shutdown_daemons(daemons)
        .await
        .expect("Manifold Guardians shut down cleanly");
    defe.release(nostr_relay_lease.handle_id)
        .await
        .expect("release Nostr relay lease");
    defe.release(bitcoind_lease.handle_id)
        .await
        .expect("release bitcoind lease");
    std::fs::remove_dir_all(&temp).expect("remove paid E2E tempdir after success");

    Ok(())
}

/// A federation dropped from the setup-payment set stays sweepable
/// (SPEC-setup-payment-federations): MG 0 sweeps it after a daemon restart,
/// which must reopen its retained wallet on demand, and MG 1 sweeps it after a
/// mnemonic restore onto a fresh data root, which must rejoin it from the
/// backed-up claim evidence rather than from the current set.
#[expect(clippy::too_many_arguments)]
async fn exercise_removed_payment_federation_sweeps(
    defe: &mut AsyncDefeClient,
    bitcoind: &defe_api::BitcoindInfo,
    manifold_guardian_bin: &Path,
    fa_cli_bin: &Path,
    bitcoin_cli: &BitcoinCli<'_>,
    miner_address: &str,
    temp: &Path,
    port_base: u16,
    iroh_overrides: &str,
    nostr: NostrEnv<'_>,
    setup_payment_keys: &NostrKeys,
    payment_invite: &str,
    payment_federation_id: &str,
    replacement_invite: &str,
    daemons: &mut Vec<Child>,
) -> anyhow::Result<()> {
    let gateway_cli_bin = locate_binary(GATEWAY_CLI_BIN_ENV, "gateway-cli")?;
    let gateway_lease = defe
        .request_gatewayd(GatewaydRequest {
            sharing: SharingMode::Exclusive,
            bitcoind: bitcoind.clone(),
            iroh_connect_overrides: Some(iroh_overrides.to_owned()),
        })
        .await
        .context("allocate a real Fedimint gateway through defe")?;
    let ResourceDescriptor::Gatewayd(gateway) = &gateway_lease.descriptor else {
        anyhow::bail!(
            "expected gatewayd descriptor from defe, got {:?}",
            gateway_lease.descriptor
        );
    };
    wait_for_gateway_connect(&gateway_cli_bin, gateway, payment_invite).await?;
    register_local_gateway_for_lnv2(temp, payment_invite, iroh_overrides, &gateway.api_url).await?;
    fund_gateway_federation_wallet(
        &gateway_cli_bin,
        gateway,
        bitcoin_cli,
        miner_address,
        payment_federation_id,
    )
    .await?;
    let recipient = SweepRecipient {
        fa_cli_bin,
        wallet_dir: temp.join("former-payment-sweep-recipient"),
        wallet_secret_file: temp.join("former-payment-sweep-recipient-secret"),
        iroh_overrides,
        federation_id: payment_federation_id,
    };
    write_sensitive_file(&recipient.wallet_secret_file, &hex::encode([73_u8; 64]))?;
    run_fa_payment_wallet(
        fa_cli_bin,
        &recipient.wallet_dir,
        &recipient.wallet_secret_file,
        &["join", "--payment-invite-code", payment_invite],
        iroh_overrides,
        Duration::from_secs(60),
    )
    .await?;

    // Replace the common set with one that no longer names the payment
    // federation every MG was paid in.
    NostrRelayClient::connect(
        nostr.relay_urls,
        setup_payment_keys.clone(),
        Duration::from_secs(10),
    )
    .await
    .map_err(|err| anyhow::anyhow!("connect setup-payment publisher to relay: {err}"))?
    .publish_event(
        EventBuilder::new(
            Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
            serde_json::json!({
                "version": 1,
                "fman_version": "0.1.0",
                "federations": [replacement_invite],
                "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
            })
            .to_string(),
        )
        .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
    )
    .await
    .map_err(|err| anyhow::anyhow!("publish replacement setup-payment federations event: {err}"))?;

    let mg0_dir = temp.join("mg-0");
    let mg1_dir = temp.join("mg-1");
    for dir in [&mg0_dir, &mg1_dir] {
        wait_for_former_payment_federation(manifold_guardian_bin, dir, payment_federation_id)
            .await?;
    }

    // Restart MG 0 on its own data root. The removed member is no longer
    // joined by policy, so only an on-demand reopen of the retained wallet
    // can list its balance and sweep it.
    let original = daemons.remove(0);
    shutdown_daemons(vec![original]).await?;
    let mut restarted = spawn_manifold_guardian(
        manifold_guardian_bin,
        &mg0_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base,
        Some(iroh_overrides),
        Some(nostr),
        None,
    )?;
    read_locator(&mut restarted, 0).await?;
    daemons.insert(0, restarted);
    wait_for_former_payment_federation(manifold_guardian_bin, &mg0_dir, payment_federation_id)
        .await?;
    sweep_former_payment_federation(
        manifold_guardian_bin,
        &mg0_dir,
        &recipient,
        "former-payment-sweep-after-restart",
    )
    .await?;

    // Restore MG 1 from its mnemonic onto a fresh data root. Its wallet
    // database is gone; the backed-up claim evidence names the payment
    // federation, and the restored wallet must rejoin it by recovery scan.
    wait_for_settled_backup(manifold_guardian_bin, &mg1_dir, 2).await?;
    let mnemonic = manifold_guardian_admin(manifold_guardian_bin, &mg1_dir, &["show-mnemonic"])
        .await?["mnemonic"]
        .as_str()
        .context("show-mnemonic returns the root phrase")?
        .to_owned();
    let original = daemons.remove(1);
    shutdown_daemons(vec![original]).await?;
    let restored_dir = temp.join("restored-mg-1");
    let mnemonic_file = temp.join("restore-mnemonic-mg-1");
    write_sensitive_file(&mnemonic_file, &mnemonic)?;
    let mut restored = spawn_manifold_guardian(
        manifold_guardian_bin,
        &restored_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base + MG_PORT_STRIDE,
        Some(iroh_overrides),
        Some(nostr),
        None,
    )?;
    let mnemonic_file_arg = mnemonic_file.display().to_string();
    let restored_answer = retry_manifold_guardian_admin(
        manifold_guardian_bin,
        &restored_dir,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &mnemonic_file_arg,
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    anyhow::ensure!(
        restored_answer["onboarded"] == "restored" && restored_answer["seats"].as_u64() == Some(2),
        "restore must recover both paid seats: {restored_answer}"
    );
    complete_onboarding_stages(manifold_guardian_bin, &restored_dir, 2, nostr).await?;
    read_locator(&mut restored, 1).await?;
    daemons.insert(1, restored);
    wait_for_former_payment_federation(manifold_guardian_bin, &restored_dir, payment_federation_id)
        .await?;
    sweep_former_payment_federation(
        manifold_guardian_bin,
        &restored_dir,
        &recipient,
        "former-payment-sweep-after-restore",
    )
    .await?;

    defe.release(gateway_lease.handle_id).await?;
    Ok(())
}

/// The recipient wallet both former-member sweeps pay into.
struct SweepRecipient<'a> {
    fa_cli_bin: &'a Path,
    wallet_dir: PathBuf,
    wallet_secret_file: PathBuf,
    iroh_overrides: &'a str,
    federation_id: &'a str,
}

/// Wait until `data_dir` lists `federation_id` as a former (no longer
/// accepted) member whose wallet can fund [`FORMER_PAYMENT_SWEEP_MSAT`].
async fn wait_for_former_payment_federation(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    federation_id: &str,
) -> anyhow::Result<()> {
    let mut last = serde_json::Value::Null;
    let waited = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let listed = manifold_guardian_admin(
                manifold_guardian_bin,
                data_dir,
                &["payment-federations", "list"],
            )
            .await?;
            let former = listed["federations"].as_array().and_then(|federations| {
                federations
                    .iter()
                    .find(|federation| federation["federation_id"] == federation_id)
            });
            if let Some(former) = former
                && former["accepted"] == serde_json::Value::Bool(false)
                && former["wallet"]["economically_sweepable_recipient_msat"]
                    .as_u64()
                    .is_some_and(|msat| msat >= FORMER_PAYMENT_SWEEP_MSAT)
            {
                return Ok::<_, anyhow::Error>(());
            }
            last = listed;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;
    match waited {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "{} never listed a sweepable former payment federation {federation_id}: {last}",
            data_dir.display()
        ),
    }
}

/// Sweep exactly [`FORMER_PAYMENT_SWEEP_MSAT`] out of the former payment
/// federation over real LNv2 and prove the recipient claimed it.
async fn sweep_former_payment_federation(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    recipient: &SweepRecipient<'_>,
    request_id: &str,
) -> anyhow::Result<()> {
    // Invoice creation fails before any operation while the gateway's LNv2
    // routing converges, so retrying cannot duplicate a receive.
    let amount_sats = (FORMER_PAYMENT_SWEEP_MSAT / 1_000).to_string();
    let invoice_args = [
        "invoice",
        "--payment-federation-id",
        recipient.federation_id,
        "--amount-sats",
        &amount_sats,
    ];
    let invoice = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if let Ok(invoice) = run_fa_payment_wallet(
                recipient.fa_cli_bin,
                &recipient.wallet_dir,
                &recipient.wallet_secret_file,
                &invoice_args,
                recipient.iroh_overrides,
                Duration::from_secs(15),
            )
            .await
            {
                return invoice;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("LNv2 gateway did not become ready for invoice creation"))?;
    let invoice_operation = invoice["operationId"]
        .as_str()
        .context("payment-wallet invoice returns operation id")?
        .to_owned();
    let invoice = invoice["invoice"]
        .as_str()
        .context("payment-wallet invoice returns BOLT11")?
        .to_owned();
    let lnurl = LnurlPayServer::start(invoice, FORMER_PAYMENT_SWEEP_MSAT).await?;
    manifold_guardian_admin(
        manifold_guardian_bin,
        data_dir,
        &["payout", "set", lnurl.destination()],
    )
    .await?;
    manifold_guardian_admin(
        manifold_guardian_bin,
        data_dir,
        &[
            "payment-federations",
            "sweep",
            recipient.federation_id,
            "--request-id",
            request_id,
        ],
    )
    .await?;
    let terminal = manifold_guardian_admin_with_timeout(
        manifold_guardian_bin,
        data_dir,
        &["payout", "await", request_id],
        Duration::from_secs(120),
    )
    .await?;
    anyhow::ensure!(
        terminal["payout"]["state"] == "succeeded"
            && terminal["payout"]["recipient_amount_msat"].as_u64()
                == Some(FORMER_PAYMENT_SWEEP_MSAT),
        "{} must sweep the former payment federation: {terminal}",
        data_dir.display()
    );
    let received = run_fa_payment_wallet(
        recipient.fa_cli_bin,
        &recipient.wallet_dir,
        &recipient.wallet_secret_file,
        &[
            "await-invoice",
            "--payment-federation-id",
            recipient.federation_id,
            "--operation-id",
            &invoice_operation,
        ],
        recipient.iroh_overrides,
        Duration::from_secs(60),
    )
    .await?;
    anyhow::ensure!(
        received["state"] == "claimed",
        "the sweep recipient must claim the invoice: {received}"
    );
    Ok(())
}

// review: nothing in tests is sensitive, a stupid agent wrote this
// review: use TempFile?
fn write_sensitive_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("create funding token file")?;
    file.write_all(contents.as_bytes())
        .context("write funding token file")?;
    Ok(())
}

/// Prove the MG seat-binding directory reached *consensus* metadata.
///
/// `fa-client` already verifies the directory it publishes — it derives the
/// peer set from the downloaded config, checks every attestation against it,
/// and compares its own readback — so a formation that returned an invite code
/// has already passed those checks, and the unit tests cover them. The one
/// thing no unit test can show is that the value became consensus in a live
/// federation. This reads it back through a `fedimint-cli` client that took no
/// part in writing it.
///
/// The daemons run with `MG_E2E_LOCAL_IROH=1`, so their fedimintd children
/// use the port-derived API keys rather than mnemonic-derived production keys.
/// Directory consensus therefore also proves every MG verified the setup
/// envelopes under the effective child keys for the real defe formation path.
///
/// Canonicality is load-bearing rather than cosmetic: threshold guardians
/// converge only on byte-identical submissions, so a value that parses but is
/// not canonical would mean agreement happened on something other than what
/// the writers intended. `parse_canonical` rejects exactly that.
async fn wait_for_seat_bindings_consensus(
    fedimint_cli: &FedimintCli<'_>,
    expected_seats: usize,
) -> anyhow::Result<()> {
    let mut last_observation = "no consensus read completed".to_owned();
    let value = tokio::time::timeout(FEDIMINT_META_CONSENSUS_TIMEOUT, async {
        loop {
            match fedimint_cli
                .run_json(&["module", "meta", "get"], FEDIMINT_CLI_META_TIMEOUT)
                .await
            {
                Ok(meta) => {
                    let Some(value) = meta
                        .get("value")
                        .and_then(|fields| fields.get(MG_SEAT_BINDINGS_META_FIELD_KEY))
                    else {
                        last_observation = format!(
                            "consensus metadata does not yet carry \
                             {MG_SEAT_BINDINGS_META_FIELD_KEY}: {meta}"
                        );
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    };
                    return value.as_str().map(ToOwned::to_owned).with_context(|| {
                        format!(
                            "consensus metadata carries {MG_SEAT_BINDINGS_META_FIELD_KEY} \
                             as a string, got {meta}"
                        )
                    });
                }
                Err(error) => {
                    last_observation = format!("consensus metadata read failed: {error:#}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "timed out after {}s waiting for consensus metadata; last observation: \
             {last_observation}",
            FEDIMINT_META_CONSENSUS_TIMEOUT.as_secs()
        )
    })??;

    let bindings = MgSeatBindings::parse_canonical(&value).map_err(|error| {
        anyhow::anyhow!("consensus directory is canonical and structurally valid: {error:?}")
    })?;

    let mut peer_ids = Vec::with_capacity(bindings.seat_bindings().len());
    let mut federation_id = None;
    for binding in bindings.seat_bindings() {
        let statement = binding.verify().map_err(|error| {
            anyhow::anyhow!("every seat binding carries a valid signature: {error:?}")
        })?;
        match &federation_id {
            None => federation_id = Some(statement.federation_id.clone()),
            Some(first) => anyhow::ensure!(
                *first == statement.federation_id,
                "every seat binding names one federation: {first:?} vs {:?}",
                statement.federation_id
            ),
        }
        peer_ids.push(statement.peer_id);
    }

    anyhow::ensure!(
        peer_ids.len() == expected_seats,
        "the directory covers every guardian seat: expected {expected_seats}, got {peer_ids:?}"
    );
    eprintln!(
        "seat-binding directory in consensus: {} seats, federation {:?}",
        peer_ids.len(),
        federation_id.expect("a verified binding set names its federation")
    );

    Ok(())
}

async fn wait_for_mg_balance(manifold_guardian_bin: &Path, data_dir: &Path) -> anyhow::Result<u64> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let listed = manifold_guardian_admin(
                manifold_guardian_bin,
                data_dir,
                &["payment-federations", "list"],
            )
            .await?;
            let balance = listed["federations"][0]["wallet"]["available_ecash_msat"]
                .as_u64()
                .context("payment-federations list reports available ecash")?;
            if balance >= SEAT_PRICE_MSAT / 2 {
                return Ok(balance);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for MG to claim its locked payment"))?
}

async fn run_gateway_cli(
    gateway_cli_bin: &Path,
    gateway: &defe_api::GatewaydInfo,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let mut command = Command::new(gateway_cli_bin);
    command
        .arg("--address")
        .arg(&gateway.api_url)
        .arg(format!("--rpcpassword={}", gateway.password))
        .args(args)
        .stderr(Stdio::piped());
    let output =
        run_expect_success(command, &format!("gateway-cli {}", args.join(" ")), timeout).await?;
    Ok(serde_json::from_str(output.trim())?)
}

/// Defe's resource readiness proves the admin API is up; the embedded LDK
/// node can become connectable a moment later.
async fn wait_for_gateway_connect(
    gateway_cli_bin: &Path,
    gateway: &defe_api::GatewaydInfo,
    invite: &str,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match run_gateway_cli(
            gateway_cli_bin,
            gateway,
            &["connect-fed", invite],
            Duration::from_secs(15),
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error.context("wait for gateway Lightning readiness")),
        }
    }
}

async fn run_fa_payment_wallet(
    fa_cli_bin: &Path,
    wallet_dir: &Path,
    wallet_secret_file: &Path,
    args: &[&str],
    iroh_overrides: &str,
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let mut command = Command::new(fa_cli_bin);
    command
        .arg("--json")
        .arg("payment-wallet")
        .arg("--wallet-data-dir")
        .arg(wallet_dir)
        .arg("--wallet-secret-file")
        .arg(wallet_secret_file)
        .args(args)
        .env("MG_E2E_LOCAL_IROH", "1")
        .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", iroh_overrides)
        .stderr(Stdio::piped());
    let output = run_expect_success(
        command,
        &format!("fa-cli payment-wallet {}", args.join(" ")),
        timeout,
    )
    .await?;
    Ok(serde_json::from_str(output.trim())?)
}

/// Minimal loopback LNURL-pay endpoint. The protocol is real; only discovery
/// is local, while invoice creation, payment, gateway routing, and claiming
/// all run through the federation.
struct LnurlPayServer {
    destination: String,
    callbacks: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl LnurlPayServer {
    async fn start(invoice: String, amount_msat: u64) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}");
        let pay_url = format!("{origin}/pay");
        let callback = format!("{origin}/callback");
        let destination = lnurl::lnurl::LnUrl::from_url(pay_url).encode();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let task_callbacks = callbacks.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let invoice = invoice.clone();
                let callback = callback.clone();
                let callbacks = task_callbacks.clone();
                tokio::spawn(async move {
                    let mut request = vec![0_u8; 4096];
                    let Ok(read) = stream.read(&mut request).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..read]);
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let body = if target == "/pay" {
                        serde_json::json!({
                            "callback": callback,
                            "maxSendable": amount_msat,
                            "minSendable": amount_msat,
                            "metadata": "[]",
                            "tag": "payRequest",
                        })
                    } else if target == format!("/callback?amount={amount_msat}") {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                        serde_json::json!({ "pr": invoice, "routes": [] })
                    } else {
                        serde_json::json!({ "status": "ERROR", "reason": "unexpected request" })
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Ok(Self {
            destination,
            callbacks,
            task,
        })
    }

    fn destination(&self) -> &str {
        &self.destination
    }

    fn callback_count(&self) -> usize {
        self.callbacks.load(Ordering::SeqCst)
    }
}

impl Drop for LnurlPayServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reserve a port grid for `mg_count` MGs, [`MG_PORT_STRIDE`] ports each,
/// and return its first port.
///
/// The grid comes from defe's allocator rather than a fixed base: the
/// allocator stays below the kernel's ephemeral port range, where any
/// outgoing socket of the test could take a seat port before its fedimintd
/// binds it, and it keeps tests that run at once on one host apart.
fn reserve_mg_port_grid(mg_count: usize) -> anyhow::Result<u16> {
    let ports = u16::try_from(mg_count)?
        .checked_mul(MG_PORT_STRIDE)
        .context("MG port grid exceeds the port range")?;
    defe_portalloc::port_alloc(ports)
}

/// Spawn `guardian_count` daemons and collect their locators.
#[expect(clippy::too_many_arguments)]
async fn start_daemons(
    manifold_guardian_bin: &Path,
    temp: &Path,
    bitcoind: &defe_api::BitcoindInfo,
    max_seats: u32,
    first_port_base: u16,
    iroh_overrides: Option<&str>,
    guardian_count: usize,
    nostr: Option<NostrEnv<'_>>,
    push_gateway_origin: Option<&str>,
) -> (Vec<Child>, Vec<String>) {
    let mut starting = Vec::with_capacity(guardian_count);
    for index in 0..guardian_count {
        let data_dir = temp.join(format!("mg-{index}"));
        // The seat port grid is per-host; give each daemon a disjoint block.
        let port_base = first_port_base + u16::try_from(index).unwrap() * MG_PORT_STRIDE;
        let daemon = spawn_manifold_guardian(
            manifold_guardian_bin,
            &data_dir,
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            port_base,
            iroh_overrides,
            nostr,
            push_gateway_origin,
        )
        .unwrap_or_else(|err| panic!("spawn fleet-manager {index}: {err}"));
        starting.push((index, daemon));
    }

    let ready = join_all(starting.into_iter().map(|(index, mut daemon)| {
        let data_dir = temp.join(format!("mg-{index}"));
        async move {
            // A Manifold Guardian has no identity until it is onboarded, so it waits
            // on its admin socket instead of serving RPC. This is the operator's
            // first act, and the deployment's.
            onboard_new_manifold_guardian(
                manifold_guardian_bin,
                &data_dir,
                max_seats,
                nostr.expect("formation tests supply an isolated Nostr relay"),
            )
            .await?;
            let locator = read_locator(&mut daemon, index).await?;
            eprintln!("fleet-manager {index} printed locator");
            Ok::<_, anyhow::Error>((daemon, locator))
        }
    }))
    .await;
    let (daemons, locators) = ready
        .into_iter()
        .map(|result| result.unwrap_or_else(|err| panic!("read fleet-manager locator: {err}")))
        .unzip();
    (daemons, locators)
}

fn mg_e2e_temp_dir() -> anyhow::Result<PathBuf> {
    // Keep the component below TMPDIR compact: every MG appends an admin
    // socket path, and Darwin counts the entire path against SUN_LEN.
    Ok(tempfile::Builder::new()
        .prefix("fm-")
        .tempdir()
        .context("create private Manifold Guardian E2E tempdir")?
        .keep())
}

async fn shutdown_daemons(daemons: Vec<Child>) -> anyhow::Result<()> {
    for daemon in &daemons {
        let pid = daemon
            .id()
            .context("Manifold Guardian process is still running")?;
        // SAFETY: `pid` came from this live child and SIGINT is an ordinary
        // process-directed signal. No pointer or borrowed memory is involved.
        let result = unsafe { libc::kill(pid.cast_signed(), libc::SIGINT) };
        anyhow::ensure!(
            result == 0,
            "send SIGINT to Manifold Guardian {pid}: {}",
            std::io::Error::last_os_error()
        );
    }

    for result in join_all(daemons.into_iter().map(|mut daemon| async move {
        match tokio::time::timeout(Duration::from_secs(15), daemon.wait()).await {
            Ok(status) => {
                let status = status.context("reap Manifold Guardian")?;
                anyhow::ensure!(status.success(), "Manifold Guardian exited with {status}");
                Ok(())
            }
            Err(_) => {
                let _ = daemon.start_kill();
                let _ = daemon.wait().await;
                anyhow::bail!("Manifold Guardian did not stop within 15 seconds after SIGINT")
            }
        }
    }))
    .await
    {
        result?;
    }
    Ok(())
}

/// Development-only Nostr wiring for a spawned MG: the defe relay URL and
/// the test setup-payment publisher key, applied through the
/// `MANIFOLD_DEV_*` overrides the development profile resolves itself.
#[derive(Clone, Copy)]
struct NostrEnv<'a> {
    /// Complete relay list for `MANIFOLD_DEV_NOSTR_RELAYS`
    /// (whitespace- or comma-separated). The daemon pools over all of them.
    relay_urls: &'a str,
    /// The one relay the test Holder issuer publishes the authorization to.
    holder_relay_url: &'a str,
    setup_payment_publisher: &'a str,
}

#[expect(clippy::too_many_arguments)]
fn spawn_manifold_guardian(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    bitcoind_url: &str,
    bitcoind_username: &str,
    bitcoind_password: &str,
    first_port_base: u16,
    iroh_overrides: Option<&str>,
    nostr: Option<NostrEnv<'_>>,
    push_gateway_origin: Option<&str>,
) -> std::io::Result<Child> {
    spawn_routed_manifold_guardian(
        manifold_guardian_bin,
        data_dir,
        bitcoind_url,
        bitcoind_username,
        bitcoind_password,
        first_port_base,
        iroh_overrides,
        nostr,
        push_gateway_origin,
        None,
        None,
    )
}

/// [`spawn_manifold_guardian`] for an MG that dials other MGs, as a transfer
/// destination does. `mg_endpoints` routes their endpoint ids, as
/// `MG_E2E_MG_ENDPOINT_ADDRS` does for fa-cli. `endpoint_bind` pins this MG's
/// own endpoint, so a route to it survives a restart.
#[expect(clippy::too_many_arguments)]
fn spawn_routed_manifold_guardian(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    bitcoind_url: &str,
    bitcoind_username: &str,
    bitcoind_password: &str,
    first_port_base: u16,
    iroh_overrides: Option<&str>,
    nostr: Option<NostrEnv<'_>>,
    push_gateway_origin: Option<&str>,
    mg_endpoints: Option<&str>,
    endpoint_bind: Option<&str>,
) -> std::io::Result<Child> {
    let mut command = Command::new(manifold_guardian_bin);
    if let Some(mg_endpoints) = mg_endpoints {
        command.env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints);
    }
    if let Some(endpoint_bind) = endpoint_bind {
        command.env("MG_E2E_IROH_BIND_ADDR", endpoint_bind);
    }
    command
        .arg("serve")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--manifold-environment")
        .arg("development")
        .arg("--bitcoind-url")
        .arg(bitcoind_url)
        .arg("--bitcoind-username")
        .arg(bitcoind_username)
        .arg("--bitcoind-password")
        .arg(bitcoind_password)
        .arg("--first-port-base")
        .arg(first_port_base.to_string())
        .env("MG_E2E_LOCAL_IROH", "1")
        // Seven fedimintd children otherwise emit tens of MiB of INFO logs,
        // swamping the CI runner and obscuring the test's own milestones.
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    if let Some(overrides) = iroh_overrides {
        // 0.11 uses NodeTickets; 0.12 accepts plain socket addresses.
        // Both point at the same isolated local guardian identities.
        let legacy_overrides = overrides
            .split(',')
            .map(|entry| {
                let (node, address) = entry.split_once('=').expect("local override pair");
                let ticket = NodeTicket::new(
                    NodeAddr::new(node.parse().expect("local node id"))
                        .with_direct_addresses([address.parse().expect("local socket address")]),
                );
                format!("{node}={ticket}")
            })
            .collect::<Vec<_>>()
            .join(",");
        command
            .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", overrides)
            .env("FM_IROH_CONNECT_OVERRIDES", legacy_overrides);
    }
    if let Some(nostr) = nostr {
        command
            .env(
                fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                nostr.relay_urls,
            )
            .env(
                fedi_decentralized_manifold_environment::DEV_SETUP_PAYMENT_PUBLISHER_ENV,
                nostr.setup_payment_publisher,
            );
    }
    if let Some(origin) = push_gateway_origin {
        command
            .arg("--push-gateway-origin")
            .arg(origin)
            .arg("--allow-insecure-push-gateway-origin");
    }
    if data_dir.join(PAYOUT_CRASH_SEAM_ENABLE).is_file() {
        // The upgrade qualification runs pre-rename executables, which read
        // the FMAN_* name of this seam.
        for name in [
            "MG_E2E_PAUSE_AFTER_GUARDIAN_FEE_PAYOUT_START",
            "FMAN_E2E_PAUSE_AFTER_GUARDIAN_FEE_PAYOUT_START",
        ] {
            command.env(name, data_dir.join(PAYOUT_CRASH_SEAM_REACHED));
        }
    }
    command.spawn()
}

async fn read_locator(daemon: &mut Child, index: usize) -> anyhow::Result<String> {
    let stdout = daemon
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("fleet-manager {index} stdout was not piped"))?;
    let mut lines = BufReader::new(stdout).lines();
    tokio::time::timeout(LOCATOR_TIMEOUT, async {
        loop {
            let Some(line) = lines.next_line().await? else {
                anyhow::bail!("fleet-manager {index} exited before printing locator");
            };
            if let Some(locator) = line.strip_prefix(Locator::LOG_PREFIX) {
                tokio::spawn(async move {
                    while lines.next_line().await.is_ok_and(|line| line.is_some()) {}
                });
                return Ok(locator.to_owned());
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for fleet-manager {index} locator"))?
}

/// `MG_E2E_MG_ENDPOINT_ADDRS` for fa-cli. Advertisements carry only MG
/// endpoint ids, and this network-isolated harness has no address lookup,
/// so route each id to the transport addresses its daemon printed.
fn mg_endpoint_addrs(locators: &[String]) -> String {
    let mut pairs = Vec::new();
    for locator in locators {
        let locator: Locator =
            serde_json::from_str(locator).expect("daemons print valid locator JSON");
        let id = locator.endpoint_addr.id;
        pairs.extend(
            locator
                .endpoint_addr
                .ip_addrs()
                .map(|addr| format!("{id}={addr}")),
        );
    }
    pairs.join(",")
}

/// Wait until fa-cli's verified selection over `relay_url` fills
/// `federation_size` seats at exactly `total_msats`: MGs advertise
/// asynchronously after onboarding and after every plan change.
async fn wait_for_mg_selection(
    fa_cli_bin: &Path,
    relay_url: &str,
    mg_endpoints: &str,
    federation_size: usize,
    total_msats: u64,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut preview = Command::new(fa_cli_bin);
        preview
            .arg("--json")
            .arg("preview")
            .arg("--federation-size")
            .arg(federation_size.to_string())
            .arg("--timeout-secs")
            .arg("10")
            .env("MG_E2E_LOCAL_IROH", "1")
            .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
            .env(
                fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                relay_url,
            )
            .stderr(Stdio::piped());
        let result = run_expect_success(preview, "fa-cli preview", Duration::from_secs(20))
            .await
            .and_then(|output| Ok(serde_json::from_str::<serde_json::Value>(output.trim())?));
        let last = match result {
            Ok(preview)
                if preview["selected"].as_u64() == Some(federation_size as u64)
                    && preview["totalAdvertisedMsats"].as_u64() == Some(total_msats) =>
            {
                return Ok(());
            }
            Ok(preview) => preview.to_string(),
            Err(error) => format!("{error:#}"),
        };
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "MG selection did not fill {federation_size} seats at {total_msats} msat: {last}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FaCliOutput {
    Human,
    JsonContract,
}

struct FaCliInvocation<'a> {
    extra_args: &'a [String],
    resume_args: Option<&'a [String]>,
    wallet_secret: Option<&'a str>,
    output: FaCliOutput,
    /// Price every MG advertises; a paid run caps the formation at the
    /// exact total of its seats.
    seat_price_msats: u64,
}

/// Run `fa-cli create` through verified selection over the MGs advertised on
/// `relay_url`, waiting first until the relay advertises a full federation at
/// the invocation's seat price.
async fn run_fa_cli(
    fa_cli_bin: &Path,
    relay_url: &str,
    mg_endpoints: &str,
    invocation: FaCliInvocation<'_>,
    iroh_overrides: Option<&str>,
    guardian_count: usize,
    timeout: Duration,
) -> anyhow::Result<String> {
    let state = tempfile::tempdir().context("create fa-cli state directory")?;
    let mut init = Command::new(fa_cli_bin);
    init.arg("--state-dir").arg(state.path()).arg("init");
    run_expect_success(init, "fa-cli init", Duration::from_secs(10)).await?;
    let total_msats = invocation.seat_price_msats * guardian_count as u64;
    wait_for_mg_selection(
        fa_cli_bin,
        relay_url,
        mg_endpoints,
        guardian_count,
        total_msats,
    )
    .await?;

    let mut command = Command::new(fa_cli_bin);
    command.arg("--state-dir").arg(state.path());
    if invocation.output == FaCliOutput::JsonContract {
        command.arg("--json");
    }
    let fa_fee_account_file = write_fa_fee_account_fixture(state.path())?;
    command
        .arg("create")
        .arg("--fa-spv2-account-file")
        .arg(&fa_fee_account_file)
        .arg("--federation-size")
        .arg(guardian_count.to_string())
        .arg("--poll-timeout-secs")
        .arg(timeout.as_secs().to_string())
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
        .env(
            fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
            relay_url,
        )
        .stderr(if invocation.output == FaCliOutput::JsonContract {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    if total_msats > 0 {
        command
            .arg("--max-total-msats")
            .arg(total_msats.to_string());
    }
    if let Some(overrides) = iroh_overrides {
        command.env("FM_IROH_CONNECT_OVERRIDES_PLAIN", overrides);
    }
    command.args(invocation.extra_args);
    if let Some(wallet_secret) = invocation.wallet_secret {
        let wallet_secret_file = state.path().join("wallet-secret");
        write_sensitive_file(&wallet_secret_file, wallet_secret)?;
        command
            .arg("--wallet-secret-file")
            .arg(&wallet_secret_file)
            .kill_on_drop(true)
            .stdout(Stdio::piped());
        let child = command.spawn().context("spawn fa-cli formation run")?;
        let mut output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for fa-cli formation run"))??;
        let mut recovery_formation_id = None;
        if !output.status.success()
            && let Some(resume_args) = invocation.resume_args
        {
            let failed_status: serde_json::Value = serde_json::from_slice(&output.stdout)
                .context("recoverable paid failure writes JSON status")?;
            let formation = &failed_status["formation"];
            anyhow::ensure!(
                formation["payment_outputs_started"].as_bool() == Some(false),
                "paid E2E may retry only before the wallet-output boundary: {failed_status}"
            );
            recovery_formation_id = Some(
                formation["formation_id"]
                    .as_str()
                    .context("recoverable paid failure names its formation")?
                    .to_owned(),
            );
            let mut resume = Command::new(fa_cli_bin);
            resume
                .arg("--state-dir")
                .arg(state.path())
                .arg("--json")
                .arg("resume")
                .arg("--fa-spv2-account-file")
                .arg(&fa_fee_account_file)
                .env("MG_E2E_LOCAL_IROH", "1")
                .args(resume_args)
                .arg("--wallet-secret-file")
                .arg(&wallet_secret_file)
                .env(MG_ENDPOINT_ADDRS_ENV, mg_endpoints)
                .env(
                    fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                    relay_url,
                )
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(overrides) = iroh_overrides {
                resume.env("FM_IROH_CONNECT_OVERRIDES_PLAIN", overrides);
            }
            let child = resume.spawn().context("spawn fa-cli formation resume")?;
            output = tokio::time::timeout(timeout, child.wait_with_output())
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for fa-cli formation resume"))??;
        }
        anyhow::ensure!(
            output.status.success(),
            "fa-cli formation run failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        if invocation.output == FaCliOutput::JsonContract {
            // The approved spending cap is the only payment authorization, so
            // a successful JSON create or resume writes nothing to stderr.
            anyhow::ensure!(
                output.stderr.is_empty(),
                "successful JSON formation must leave stderr empty: {}",
                String::from_utf8_lossy(&output.stderr),
            );
            let stdout = String::from_utf8(output.stdout)?;
            anyhow::ensure!(
                stdout.lines().count() == 1,
                "JSON formation must write exactly one stdout line"
            );
            let status: serde_json::Value = serde_json::from_str(stdout.trim())?;
            let invite = formed_invite(&status)
                .with_context(|| format!("paid formation did not reach Formed: {status}"))?;
            if let Some(recovery_formation_id) = recovery_formation_id {
                anyhow::ensure!(
                    status["formed"]["formation_id"].as_str()
                        == Some(recovery_formation_id.as_str()),
                    "paid resume changed formation identity: {status}"
                );
            }
            return Ok(invite.to_owned());
        }
        human_invite(&String::from_utf8(output.stdout)?)
    } else {
        human_invite(&run_expect_success(command, "fa-cli formation run", timeout).await?)
    }
}

/// The invite line of fa-cli's human status, which also carries state lines.
fn human_invite(stdout: &str) -> anyhow::Result<String> {
    stdout
        .lines()
        .find(|line| line.starts_with("fed1"))
        .map(ToOwned::to_owned)
        .with_context(|| format!("fa-cli printed no federation invite: {stdout}"))
}

fn write_fa_fee_account_fixture(state_dir: &Path) -> anyhow::Result<PathBuf> {
    // fa-cli documents this explicit account as a development-only consumer
    // override. It is a valid single-signature BtcDepositor account, while
    // the real MG guardians still derive and validate every other recipient.
    let path = state_dir.join("fi-spv2-account.json");
    std::fs::write(
        &path,
        br#"{"acc_type":"BtcDepositor","pub_keys":["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],"threshold":1}"#,
    )
    .context("write public FA SPv2 account fixture")?;
    Ok(path)
}

/// Give the Fedimint client direct localhost routes to the freshly-created
/// federation. Its invite contains only bare iroh node IDs, for which public
/// discovery is intentionally unavailable in this local test.
fn local_iroh_overrides_for_grid(
    first_port_base: u16,
    max_seats: u16,
    guardian_count: usize,
) -> String {
    let mut overrides = Vec::with_capacity(guardian_count * usize::from(max_seats) * 2);
    for guardian in 0..u16::try_from(guardian_count).unwrap() {
        for seat in 0..max_seats {
            let base = first_port_base + guardian * MG_PORT_STRIDE + seat * 4;
            for (port, role) in [(base, b"p2p".as_slice()), (base + 1, b"api".as_slice())] {
                let secret = SecretKey::from_bytes(&e2e_iroh_key(port, role));
                let node_id: NodeId = secret.public();
                overrides.push(format!("{node_id}=127.0.0.1:{port}"));
            }
        }
    }
    overrides.join(",")
}

fn e2e_iroh_key(port: u16, role: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"fman-e2e-local-iroh-v1\0")
        .chain_update(port.to_be_bytes())
        .chain_update(role)
        .finalize()
        .into()
}

/// Run a subprocess to completion; a non-zero exit or a timeout fails with
/// the captured output, and the trimmed stdout is returned on success.
async fn run_expect_success(
    mut command: Command,
    what: &str,
    timeout: Duration,
) -> anyhow::Result<String> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("spawn {what}: {err}"))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for {what}"))??;
    anyhow::ensure!(
        output.status.success(),
        "{what} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// Wait until the daemon's admitted setup-payment set contains a receivable
/// member: publication admission (the local E2E 15s poll) plus the wallet join.
async fn wait_for_accepted_payment_federation(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
) -> anyhow::Result<String> {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let listed = manifold_guardian_admin(
                manifold_guardian_bin,
                data_dir,
                &["payment-federations", "list"],
            )
            .await?;
            let accepted = listed["federations"].as_array().and_then(|federations| {
                federations.iter().find(|federation| {
                    federation["accepted"] == serde_json::Value::Bool(true)
                        && federation["receivable"] == serde_json::Value::Bool(true)
                })
            });
            if let Some(federation) = accepted {
                return federation["federation_id"]
                    .as_str()
                    .map(ToOwned::to_owned)
                    .context("payment federation listing carries an id");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("timed out waiting for an accepted, receivable payment federation")
    })?
}

/// `fman-cli` against a daemon's socket; answers are JSON.
/// Offer every MG's seats at zero, the bootstrap admission.
///
/// A fresh MG sells nothing until its operator says what it sells, and no
/// ecash exists yet to pay the first federation's guardians with — so the
/// deployment starts by giving those seats away.
async fn offer_free_seats(
    manifold_guardian_bin: &Path,
    temp: &Path,
    guardian_count: usize,
) -> anyhow::Result<()> {
    for index in 0..guardian_count {
        manifold_guardian_admin(
            manifold_guardian_bin,
            &temp.join(format!("mg-{index}")),
            &["plans", "set", "--price-msats", "0"],
        )
        .await?;
    }
    Ok(())
}

/// Onboard a freshly started daemon as a new Manifold Guardian, retrying while it
/// binds its socket.
async fn onboard_new_manifold_guardian(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    max_seats: u32,
    nostr: NostrEnv<'_>,
) -> anyhow::Result<()> {
    retry_manifold_guardian_admin(manifold_guardian_bin, data_dir, &["onboard", "new"]).await?;
    complete_onboarding_stages(manifold_guardian_bin, data_dir, max_seats, nostr).await
}

/// Walk the staged wizard from the daemon's durable cursor to completion:
/// publish a test Holder authorization when the stage asks for one, then
/// configure the initial offer. Also the tail of a restore, which re-enters
/// the wizard at the authorization stage rather than at a formed fleet.
async fn complete_onboarding_stages(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    max_seats: u32,
    nostr: NostrEnv<'_>,
) -> anyhow::Result<()> {
    let onboarding =
        manifold_guardian_admin(manifold_guardian_bin, data_dir, &["onboarding"]).await?;
    let stage = onboarding["stage"]
        .as_str()
        .context("onboarding response has a stage")?;
    if stage == "complete" {
        return Ok(());
    }
    if stage == "holder_authorization" {
        let subject = onboarding["service_nostr_pubkey"]
            .as_str()
            .context("onboarding response has a service Nostr pubkey")?;
        let authorization_request = serde_json::json!({ "subject_pubkey": subject }).to_string();
        let issuer = manifold_guardian_bin.with_file_name("manifold-test-issuer");
        run_expect_success(
            {
                let mut command = Command::new(issuer);
                command.args([
                    "--environment",
                    "development",
                    "--relay",
                    nostr.holder_relay_url,
                    "--authorization-request",
                    &authorization_request,
                    "--publish-mg-authorization",
                ]);
                command
            },
            "publish test Holder authorization",
            Duration::from_secs(30),
        )
        .await?;
        manifold_guardian_admin(
            manifold_guardian_bin,
            data_dir,
            &["refresh-holder-authorizations"],
        )
        .await?;
    }
    let max_seats = max_seats.to_string();
    // Offer free seats right at onboarding so the daemon is accepting seats
    // — and therefore advertising — from its first Nostr cycle. Every test
    // either wants free seats or re-prices immediately after startup.
    manifold_guardian_admin(
        manifold_guardian_bin,
        data_dir,
        &[
            "onboard",
            "offer",
            "--max-seats",
            &max_seats,
            "--price-msats",
            "0",
        ],
    )
    .await?;
    Ok(())
}

/// Run an admin command once a newly spawned daemon binds its socket.
async fn retry_manifold_guardian_admin(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    args: &[&str],
) -> anyhow::Result<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + LOCATOR_TIMEOUT;
    loop {
        match manifold_guardian_admin(manifold_guardian_bin, data_dir, args).await {
            Ok(answer) => return Ok(answer),
            Err(err) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = err;
            }
            Err(err) => return Err(err.context("run Manifold Guardian admin command")),
        }
    }
}

async fn manifold_guardian_admin(
    manifold_guardian_bin: &Path,
    data_dir: &Path,
    args: &[&str],
) -> anyhow::Result<serde_json::Value> {
    manifold_guardian_admin_with_timeout(
        manifold_guardian_bin,
        data_dir,
        args,
        Duration::from_secs(30),
    )
    .await
}

async fn manifold_guardian_admin_with_timeout(
    _manifold_guardian_bin: &Path,
    data_dir: &Path,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let mg_cli_bin = locate_binary(MG_CLI_BIN_ENV, "fman-cli")?;
    let mut command = Command::new(mg_cli_bin);
    command
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .stderr(Stdio::piped());
    let stdout =
        run_expect_success(command, &format!("fman-cli {}", args.join(" ")), timeout).await?;
    Ok(serde_json::from_str(&stdout)?)
}

/// One FA-side `fedimint-cli` wallet.
struct FedimintCli<'a> {
    bin: &'a Path,
    data_dir: PathBuf,
    iroh_overrides: &'a str,
}

impl FedimintCli<'_> {
    async fn run(&self, args: &[&str], timeout: Duration) -> anyhow::Result<String> {
        let mut command = Command::new(self.bin);
        command
            .arg("--data-dir")
            .arg(&self.data_dir)
            .args(args)
            .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", self.iroh_overrides)
            .stderr(Stdio::piped());
        if std::env::var_os("DEV_DEFE_SOCKET_PATH").is_some() {
            command.env("FM_IN_DEVIMINT", "1");
        }
        run_expect_success(
            command,
            &format!("fedimint-cli {}", args.first().copied().unwrap_or("")),
            timeout,
        )
        .await
    }

    async fn run_json(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::from_str(&self.run(args, timeout).await?)?)
    }
}

/// `bitcoin-cli` against the defe-allocated regtest node.
struct BitcoinCli<'a> {
    bin: &'a Path,
    bitcoind: &'a defe_api::BitcoindInfo,
}

impl<'a> BitcoinCli<'a> {
    fn new(bin: &'a Path, bitcoind: &'a defe_api::BitcoindInfo) -> anyhow::Result<Self> {
        Ok(Self { bin, bitcoind })
    }

    /// Use the mature funds Defe creates for this fixture's exclusive node.
    async fn funded_miner_address(&self) -> anyhow::Result<String> {
        // Both callers hold exclusive leases: every fixture has its own node,
        // data directory and default wallet, so coin selection cannot overlap
        // with another test. Defe mines 101 blocks before returning the lease.
        let balance = self.run(Some(MINER_WALLET), &["getbalance"]).await?;
        let balance = fedimint_core::bitcoin::Amount::from_str_in(
            balance.trim(),
            fedimint_core::bitcoin::Denomination::Bitcoin,
        )
        .context("parse exclusive Defe wallet's spendable balance")?;
        anyhow::ensure!(
            balance >= fedimint_core::bitcoin::Amount::from_sat(100_000_000),
            "exclusive Defe wallet must have mature funds before fixture startup"
        );
        Ok(self
            .run(Some(MINER_WALLET), &["getnewaddress"])
            .await?
            .trim()
            .to_owned())
    }

    async fn run(&self, wallet: Option<&str>, args: &[&str]) -> anyhow::Result<String> {
        let mut command = Command::new(self.bin);
        command
            .arg(format!("-rpcconnect={}", self.bitcoind.rpc_host))
            .arg(format!("-rpcport={}", self.bitcoind.rpc_port))
            .arg(format!("-rpcuser={}", self.bitcoind.rpc_username))
            .arg(format!("-rpcpassword={}", self.bitcoind.rpc_password))
            .stderr(Stdio::piped());
        if let Some(wallet) = wallet {
            command.arg(format!("-rpcwallet={wallet}"));
        }
        command.args(args);
        run_expect_success(
            command,
            &format!("bitcoin-cli {}", args.first().copied().unwrap_or("")),
            BITCOIN_CLI_TIMEOUT,
        )
        .await
    }
}

fn locate_binary(env_var: &str, binary_name: &str) -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os(env_var) {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "{env_var} points to missing binary: {}",
            path.display()
        );
        return Ok(path);
    }

    if let Some(path) = binary_in_target_dir(binary_name) {
        return Ok(path);
    }
    if let Some(path) = binary_on_path(binary_name) {
        return Ok(path);
    }

    anyhow::bail!(
        "could not locate `{binary_name}`; build it first, expose it on PATH, or set {env_var} to its full path \
         (the CI test derivation in flake.nix sets these env vars to the flake-pinned fedimint binaries; \
         the ordinary dev shell does not, so set them by hand, e.g. from `nix build .#fedimintd`)"
    )
}

fn binary_in_target_dir(binary_name: &str) -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let deps_dir = exe.parent()?;
    let target_dir = deps_dir.parent()?;
    let candidate = target_dir.join(exe_name(binary_name));
    candidate.is_file().then_some(candidate)
}

fn binary_on_path(binary_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(exe_name(binary_name)))
        .find(|candidate| candidate.is_file())
}

fn exe_name(binary_name: &str) -> String {
    format!("{binary_name}{}", env::consts::EXE_SUFFIX)
}
