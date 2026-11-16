//! Seat-cost simulation rig: one Manifold Guardian under test hosts one
//! guardian seat in each of N federations whose other guardians run on helper
//! MGs, while in-process clients drive a controlled transaction rate.
//!
//! This is a measurement fixture, not a gate. It runs only when
//! `SEAT_SIM_DIR` names a control directory, and it is meant to be started
//! by `scripts/seat-cost-sim/launch.sh`, which places the MG under test in its
//! own cgroup, CPU set, and network namespace so a sampler can attribute its
//! CPU, memory, disk, and network use exactly. See
//! `scripts/seat-cost-sim/README.md` for the protocol.
//!
//! The rig reads `control.json` from the control directory:
//! `{"federations": N, "ops_per_sec": r, "stop": false}`. It forms
//! federations until N exist, keeps every federation at `r` user operations
//! per second (alternating an ecash transfer and a Lightning payment through
//! the local gateway), and exits cleanly when `stop` is true. It writes
//! `state.json` after every formation and `load.json` every few seconds.

use std::sync::atomic::AtomicU64;

use fedimint_client::{Client, ClientHandleArc, RootSecret};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::Amount;
use fedimint_core::db::Database;
use fedimint_core::db::mem_impl::MemDatabase;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::util::SafeUrl;
use fedimint_lnv2_client::{
    FinalReceiveOperationState as LnReceiveState, FinalSendOperationState as LnSendState,
    LightningClientInit as LightningV2ClientInit, LightningClientModule as LightningV2Module,
};
use fedimint_lnv2_common::Bolt11InvoiceDescription;
use fedimint_mintv2_client::{
    FinalReceiveOperationState as EcashReceiveState, MintClientInit as MintV2ClientInit,
    MintClientModule as MintV2Module,
};
use fedimint_walletv2_client::{WalletClientInit as WalletV2ClientInit, WalletClientModule};

use super::*;

const SIM_DIR_ENV: &str = "SEAT_SIM_DIR";
const SIM_GUARDIANS_ENV: &str = "SEAT_SIM_GUARDIANS";
const SIM_MAX_SEATS_ENV: &str = "SEAT_SIM_MAX_SEATS";
/// Wrapper that execs the MG under test inside its cgroup, CPU set, and
/// network namespace.
const SIM_MUT_BIN_ENV: &str = "SEAT_SIM_MUT_MG_BIN";
/// Wrapper that execs a helper MG with the same environment adjustments.
const SIM_HELPER_BIN_ENV: &str = "SEAT_SIM_HELPER_MG_BIN";
const SIM_GATEWAYD_BIN_ENV: &str = "SEAT_SIM_GATEWAYD_BIN";
const SIM_BLOCK_SECS_ENV: &str = "SEAT_SIM_BLOCK_SECS";
/// Address of the rig's side of the veth pair. Helpers, bitcoind, and the
/// relay are reachable from the MG under test only through it.
const HELPER_IP: &str = "10.77.0.1";
/// Address of the MG under test, alone in its network namespace.
const MUT_IP: &str = "10.77.0.2";
/// Port offset, within the MG under test's grid block, of its pinned service
/// endpoint. Seat blocks start at offset 0 and take 4 ports each.
const MUT_ENDPOINT_OFFSET: u16 = 90;
const GATEWAY_PASSWORD: &str = "testpassword";
const TRANSFER_MSAT: u64 = 20_000;
const CLIENT_PEGIN_BTC: &str = "0.01";
const MAX_IN_FLIGHT_PER_FEDERATION: u64 = 16;
const SIM_FORMATION_TIMEOUT: Duration = Duration::from_secs(600);

#[test]
fn seat_cost_rig() {
    let Some(dir) = env::var_os(SIM_DIR_ENV).map(PathBuf::from) else {
        eprintln!("skipping seat-cost rig; set {SIM_DIR_ENV} to run it");
        return;
    };
    let guardians = env_number(SIM_GUARDIANS_ENV, 10);
    let max_seats = env_number(SIM_MAX_SEATS_ENV, 16);
    assert!(
        u16::try_from(max_seats).unwrap() * SeatBlock::PORTS <= MUT_ENDPOINT_OFFSET,
        "seat blocks must stay below the pinned endpoint port"
    );
    let port_base = reserve_mg_port_grid(guardians).expect("reserve the MG port grid");
    let overrides = sim_iroh_overrides(port_base, max_seats, guardians);
    // SAFETY: no other thread runs yet; the runtime that reads the variable
    // through every in-process client connector is built below.
    unsafe { env::set_var("FM_IROH_CONNECT_OVERRIDES_PLAIN", &overrides) };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("build the rig runtime")
        .block_on(run_seat_cost_rig(
            dir, guardians, max_seats, port_base, overrides,
        ))
        .expect("seat-cost rig failed");
}

fn env_number(name: &str, default: usize) -> usize {
    env::var(name).map_or(default, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a number"))
    })
}

struct SeatBlock;

impl SeatBlock {
    const PORTS: u16 = 4;
}

/// Routes for every seat the rig can create: the MG under test's seats at
/// its namespace address, every helper's at the rig's veth address.
fn sim_iroh_overrides(port_base: u16, max_seats: usize, guardians: usize) -> String {
    let mut overrides = Vec::new();
    for guardian in 0..u16::try_from(guardians).unwrap() {
        let ip = if guardian == 0 { MUT_IP } else { HELPER_IP };
        for seat in 0..u16::try_from(max_seats).unwrap() {
            let base = port_base + guardian * MG_PORT_STRIDE + seat * SeatBlock::PORTS;
            for (port, role) in [(base, b"p2p".as_slice()), (base + 1, b"api".as_slice())] {
                let node_id: NodeId = SecretKey::from_bytes(&e2e_iroh_key(port, role)).public();
                overrides.push(format!("{node_id}={ip}:{port}"));
            }
        }
    }
    overrides.join(",")
}

#[derive(serde::Deserialize, Clone, Copy, PartialEq)]
struct Control {
    federations: usize,
    ops_per_sec: f64,
    #[serde(default)]
    stop: bool,
}

fn read_control(dir: &Path) -> anyhow::Result<Control> {
    let text = std::fs::read_to_string(dir.join("control.json")).context("read control.json")?;
    Ok(serde_json::from_str(&text)?)
}

fn write_json(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Default)]
struct OpStats {
    started: AtomicU64,
    ecash_ok: AtomicU64,
    ecash_err: AtomicU64,
    ln_ok: AtomicU64,
    ln_err: AtomicU64,
    skipped_busy: AtomicU64,
    latency_ms_total: AtomicU64,
}

impl OpStats {
    fn json(&self) -> serde_json::Value {
        let load = |value: &AtomicU64| value.load(Ordering::Relaxed);
        serde_json::json!({
            "started": load(&self.started),
            "ecash_ok": load(&self.ecash_ok),
            "ecash_err": load(&self.ecash_err),
            "ln_ok": load(&self.ln_ok),
            "ln_err": load(&self.ln_err),
            "skipped_busy": load(&self.skipped_busy),
            "latency_ms_total": load(&self.latency_ms_total),
        })
    }
}

struct SimFederation {
    invite: String,
    federation_id: String,
    clients: [ClientHandleArc; 2],
    stats: Arc<OpStats>,
}

#[expect(clippy::too_many_lines)]
async fn run_seat_cost_rig(
    dir: PathBuf,
    guardians: usize,
    max_seats: usize,
    port_base: u16,
    overrides: String,
) -> anyhow::Result<()> {
    let mut_bin = locate_binary(SIM_MUT_BIN_ENV, "seat-sim-mut-mg")?;
    let helper_bin = locate_binary(SIM_HELPER_BIN_ENV, "seat-sim-helper-mg")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let bitcoin_cli_bin = locate_binary(BITCOIN_CLI_BIN_ENV, "bitcoin-cli")?;
    let gateway_cli_bin = locate_binary(GATEWAY_CLI_BIN_ENV, "gateway-cli")?;
    let gatewayd_bin = locate_binary(SIM_GATEWAYD_BIN_ENV, "gatewayd")?;
    let block_secs = env_number(SIM_BLOCK_SECS_ENV, 600) as u64;

    let mut defe = AsyncDefeClient::connect_from_env().await?;
    let bitcoind_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!("expected bitcoind descriptor");
    };
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = &relay_lease.descriptor else {
        anyhow::bail!("expected Nostr relay descriptor");
    };
    let publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();
    let bitcoin_cli = BitcoinCli::new(&bitcoin_cli_bin, bitcoind)?;
    let miner_address = bitcoin_cli.funded_miner_address().await?;

    // The MG under test reaches bitcoind and the relay through the veth
    // address, which the launcher forwards to the rig's loopback.
    let mut_bitcoind_url = bitcoind.rpc_url.replace("127.0.0.1", HELPER_IP);
    let mut_relay_url = relay.url.replace("127.0.0.1", HELPER_IP);
    let temp = dir.join("mgs");
    let mut daemons = Vec::with_capacity(guardians);
    let mut locators = Vec::with_capacity(guardians);
    for index in 0..guardians {
        let data_dir = temp.join(format!("mg-{index}"));
        let base = port_base + u16::try_from(index)? * MG_PORT_STRIDE;
        let (bin, bitcoind_url, relay_urls, endpoint_bind) = if index == 0 {
            (
                &mut_bin,
                mut_bitcoind_url.as_str(),
                mut_relay_url.as_str(),
                Some(format!("{MUT_IP}:{}", base + MUT_ENDPOINT_OFFSET)),
            )
        } else {
            (
                &helper_bin,
                bitcoind.rpc_url.as_str(),
                relay.url.as_str(),
                None,
            )
        };
        let nostr = NostrEnv {
            relay_urls,
            holder_relay_url: &relay.url,
            setup_payment_publisher: &publisher,
        };
        let mut daemon = spawn_routed_manifold_guardian(
            bin,
            &data_dir,
            bitcoind_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            base,
            Some(&overrides),
            Some(nostr),
            None,
            None,
            endpoint_bind.as_deref(),
        )
        .with_context(|| format!("spawn MG {index}"))?;
        onboard_new_manifold_guardian(bin, &data_dir, u32::try_from(max_seats)?, nostr).await?;
        locators.push(read_locator(&mut daemon, index).await?);
        daemons.push(daemon);
        eprintln!("seat-sim: MG {index} onboarded");
    }
    offer_free_seats(&helper_bin, &temp, guardians).await?;
    let mg_endpoints = mg_endpoint_addrs(&locators);

    let gateway = spawn_sim_gateway(&gatewayd_bin, &gateway_cli_bin, &dir, bitcoind).await?;
    let (gateway_info, mut gatewayd) = gateway;
    let gateway_url: SafeUrl = gateway_info.api_url.parse()?;

    // Production chains produce a block about every ten minutes; walletv2
    // consensus follows them.
    let miner = {
        let bin = bitcoin_cli_bin.clone();
        let bitcoind = bitcoind.clone();
        let miner_address = miner_address.clone();
        tokio::spawn(async move {
            let bitcoin_cli = BitcoinCli::new(&bin, &bitcoind).expect("bitcoin-cli");
            loop {
                tokio::time::sleep(Duration::from_secs(block_secs)).await;
                if let Err(error) = bitcoin_cli
                    .run(None, &["generatetoaddress", "1", &miner_address])
                    .await
                {
                    eprintln!("seat-sim: periodic block failed: {error:#}");
                }
            }
        })
    };

    let connectors = ConnectorRegistry::build_from_client_env()?.bind().await?;
    let mut federations: Vec<SimFederation> = Vec::new();
    let (rate_tx, rate_rx) = tokio::sync::watch::channel(0.0_f64);
    let mut load_tasks = Vec::new();
    let mut last_stats = tokio::time::Instant::now();
    let write_state = |federations: &[SimFederation]| {
        write_json(
            &dir.join("state.json"),
            &serde_json::json!({
                "port_base": port_base,
                "guardians": guardians,
                "max_seats": max_seats,
                "mg_port_stride": MG_PORT_STRIDE,
                "bitcoind_rpc_port": bitcoind.rpc_port,
                "relay_url": relay.url,
                "gateway_api_url": gateway_info.api_url,
                "mut_mg_pid": daemons[0].id(),
                "federations": federations
                    .iter()
                    .map(|federation| serde_json::json!({
                        "invite": federation.invite,
                        "federation_id": federation.federation_id,
                    }))
                    .collect::<Vec<_>>(),
            }),
        )
    };
    write_state(&federations)?;
    loop {
        let control = read_control(&dir)?;
        if control.stop {
            break;
        }
        rate_tx.send_replace(control.ops_per_sec);
        if federations.len() < control.federations.min(max_seats) {
            let index = federations.len();
            let formed = tokio::time::timeout(
                SIM_FORMATION_TIMEOUT,
                form_sim_federation(
                    &fa_cli_bin,
                    &gateway_cli_bin,
                    &gateway_info,
                    &gateway_url,
                    &bitcoin_cli,
                    &miner_address,
                    &connectors,
                    &relay.url,
                    &mg_endpoints,
                    &overrides,
                    guardians,
                    index,
                ),
            )
            .await
            .context("form federation timed out")??;
            eprintln!("seat-sim: federation {index} formed and funded");
            load_tasks.push(tokio::spawn(run_federation_load(
                formed.clients.clone(),
                gateway_url.clone(),
                formed.stats.clone(),
                rate_rx.clone(),
            )));
            federations.push(formed);
            write_state(&federations)?;
            continue;
        }
        if last_stats.elapsed() >= Duration::from_secs(5) {
            last_stats = tokio::time::Instant::now();
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
            write_json(
                &dir.join("load.json"),
                &serde_json::json!({
                    "ts": now,
                    "ops_per_sec": control.ops_per_sec,
                    "federations": federations
                        .iter()
                        .map(|federation| federation.stats.json())
                        .collect::<Vec<_>>(),
                }),
            )?;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eprintln!("seat-sim: stopping");
    rate_tx.send_replace(0.0);
    for task in load_tasks {
        task.abort();
    }
    miner.abort();
    for federation in federations {
        for client in federation.clients {
            let _ = client;
        }
    }
    shutdown_daemons(daemons).await?;
    let _ = gatewayd.start_kill();
    let _ = gatewayd.wait().await;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    Ok(())
}

/// A gatewayd like defe's, minus `FM_IN_DEVIMINT`: that switch would make
/// the gateway's federation clients poll the guardians at test cadence.
async fn spawn_sim_gateway(
    gatewayd_bin: &Path,
    gateway_cli_bin: &Path,
    dir: &Path,
    bitcoind: &defe_api::BitcoindInfo,
) -> anyhow::Result<(defe_api::GatewaydInfo, Child)> {
    let ports = defe_portalloc::port_alloc(3)?;
    let api_url = format!("http://127.0.0.1:{ports}");
    let data_dir = dir.join("gatewayd");
    std::fs::create_dir_all(&data_dir)?;
    let log = std::fs::File::create(dir.join("gatewayd.log"))?;
    let mut command = Command::new(gatewayd_bin);
    command
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["--listen", &format!("127.0.0.1:{ports}")])
        .args(["--api-addr", &api_url])
        .args(["--network", "regtest", "--num-route-hints", "0", "ldk"])
        .args(["--ldk-lightning-port", &(ports + 2).to_string()])
        .args(["--ldk-alias", "seat-sim-gateway"])
        .env("RUST_LOG", "info")
        .env("FM_GATEWAY_METRICS_LISTEN_ADDR", format!("127.0.0.1:{}", ports + 1))
        .env(
            "FM_GATEWAY_BCRYPT_PASSWORD_HASH",
            "$2b$12$Etlumnzi/VJ0Ky0Dqoe55eCbvDXItj94thfhvu2o423ox7os.6XfC",
        )
        .env("FM_GATEWAY_SKIP_SETUP", "true")
        .env("FM_BITCOIND_URL", &bitcoind.rpc_url)
        .env("FM_BITCOIND_USERNAME", &bitcoind.rpc_username)
        .env("FM_BITCOIND_PASSWORD", &bitcoind.rpc_password)
        .env("FM_DEFAULT_ROUTING_FEES", "0,0")
        .env("FM_DEFAULT_TRANSACTION_FEES", "0,0")
        .env("FM_PORT_LDK", (ports + 2).to_string())
        .env("FM_LDK_ALIAS", "seat-sim-gateway")
        .env(
            "FM_GATEWAY_MNEMONIC",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .env_remove("FM_IN_DEVIMINT")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true);
    let child = command.spawn().context("spawn gatewayd")?;
    let info = defe_api::GatewaydInfo {
        api_url,
        password: GATEWAY_PASSWORD.to_owned(),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if run_gateway_cli(gateway_cli_bin, &info, &["info"], Duration::from_secs(10))
            .await
            .is_ok()
        {
            return Ok((info, child));
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "gatewayd did not become ready"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[expect(clippy::too_many_arguments)]
async fn form_sim_federation(
    fa_cli_bin: &Path,
    gateway_cli_bin: &Path,
    gateway: &defe_api::GatewaydInfo,
    gateway_url: &SafeUrl,
    bitcoin_cli: &BitcoinCli<'_>,
    miner_address: &str,
    connectors: &ConnectorRegistry,
    relay_url: &str,
    mg_endpoints: &str,
    overrides: &str,
    guardians: usize,
    index: usize,
) -> anyhow::Result<SimFederation> {
    let invite = run_fa_cli(
        fa_cli_bin,
        relay_url,
        mg_endpoints,
        FaCliInvocation {
            extra_args: &[],
            resume_args: None,
            wallet_secret: None,
            output: FaCliOutput::Human,
            seat_price_msats: 0,
        },
        Some(overrides),
        guardians,
        SIM_FORMATION_TIMEOUT,
    )
    .await?;
    let invite = invite.trim().to_owned();
    let invite_code: InviteCode = invite.parse()?;
    let federation_id = invite_code.federation_id().to_string();
    eprintln!("seat-sim: federation {index} formed: {federation_id}");

    wait_for_gateway_connect(gateway_cli_bin, gateway, &invite).await?;
    let mut clients = Vec::new();
    for name in ["alice", "bob"] {
        clients.push(join_sim_client(connectors, &invite_code, index, name).await?);
    }
    let gateway_deposit = run_gateway_cli(
        gateway_cli_bin,
        gateway,
        &["ecash", "pegin", "--federation-id", &federation_id],
        Duration::from_secs(120),
    )
    .await?;
    let gateway_address = gateway_deposit["address"]
        .as_str()
        .context("gateway ecash pegin returns an address")?
        .to_owned();
    let client_address = clients[0]
        .get_first_module::<WalletClientModule>()?
        .receive()
        .await
        .to_string();
    for address in [&gateway_address, &client_address] {
        bitcoin_cli
            .run(
                Some(MINER_WALLET),
                &["sendtoaddress", address, CLIENT_PEGIN_BTC],
            )
            .await?;
    }
    bitcoin_cli
        .run(None, &["generatetoaddress", "7", miner_address])
        .await?;
    // Production walletv2 polls once a minute, so claims take a few rounds.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(900);
    loop {
        let balances = run_gateway_cli(
            gateway_cli_bin,
            gateway,
            &["get-balances"],
            Duration::from_secs(15),
        )
        .await?;
        let gateway_funded = balances["ecash_balances"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|balance| {
                balance["federation_id"] == federation_id.as_str()
                    && balance["ecash_balance_msats"].as_u64().unwrap_or_default() >= 100_000_000
            });
        let client_funded = clients[0].get_balance_for_btc().await?.msats >= 100_000_000;
        if gateway_funded && client_funded {
            break;
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "federation {index} peg-ins were not claimed: gateway {gateway_funded}, client {client_funded}"
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    // Give the second client half so transfers can run in both directions.
    let mint = clients[0].get_first_module::<MintV2Module>()?;
    let (_, ecash) = mint
        .send(
            Amount::from_msats(500_000_000),
            serde_json::Value::Null,
            false,
        )
        .await?;
    let bob_mint = clients[1].get_first_module::<MintV2Module>()?;
    let operation = bob_mint.receive(ecash, serde_json::Value::Null).await?;
    anyhow::ensure!(
        bob_mint
            .await_final_receive_operation_state(operation)
            .await?
            == EcashReceiveState::Success,
        "initial transfer to the second client failed"
    );
    // One Lightning round trip proves the gateway serves this federation.
    run_ln_payment(&clients[0], &clients[1], gateway_url).await?;
    let clients: [ClientHandleArc; 2] = clients
        .try_into()
        .map_err(|_| anyhow::anyhow!("two clients"))?;
    Ok(SimFederation {
        invite,
        federation_id,
        clients,
        stats: Arc::new(OpStats::default()),
    })
}

async fn join_sim_client(
    connectors: &ConnectorRegistry,
    invite: &InviteCode,
    index: usize,
    name: &str,
) -> anyhow::Result<ClientHandleArc> {
    let mut builder = Client::builder().await?;
    builder.with_module(MintV2ClientInit);
    builder.with_module(LightningV2ClientInit::default());
    builder.with_module(WalletV2ClientInit);
    let seed: [u8; 32] = Sha256::new()
        .chain_update(b"seat-sim-client\0")
        .chain_update(index.to_be_bytes())
        .chain_update(name.as_bytes())
        .finalize()
        .into();
    let root = DerivableSecret::new_root(&seed, b"seat-sim");
    let database: Database = MemDatabase::new().into();
    let client = builder
        .preview(connectors.clone(), invite)
        .await?
        .join(database, RootSecret::StandardDoubleDerive(root))
        .await?;
    Ok(Arc::new(client))
}

async fn run_ecash_transfer(from: &ClientHandleArc, to: &ClientHandleArc) -> anyhow::Result<()> {
    let (_, ecash) = from
        .get_first_module::<MintV2Module>()?
        .send(
            Amount::from_msats(TRANSFER_MSAT),
            serde_json::Value::Null,
            false,
        )
        .await?;
    let mint = to.get_first_module::<MintV2Module>()?;
    let operation = mint.receive(ecash, serde_json::Value::Null).await?;
    anyhow::ensure!(
        mint.await_final_receive_operation_state(operation).await? == EcashReceiveState::Success,
        "ecash receive was rejected"
    );
    Ok(())
}

async fn run_ln_payment(
    from: &ClientHandleArc,
    to: &ClientHandleArc,
    gateway: &SafeUrl,
) -> anyhow::Result<()> {
    let receiver = to.get_first_module::<LightningV2Module>()?;
    let (invoice, receive_operation) = receiver
        .receive(
            Amount::from_msats(TRANSFER_MSAT),
            3600,
            Bolt11InvoiceDescription::Direct(String::new()),
            Some(gateway.clone()),
            serde_json::Value::Null,
        )
        .await?;
    let sender = from.get_first_module::<LightningV2Module>()?;
    let send_operation = sender
        .send(invoice, Some(gateway.clone()), serde_json::Value::Null)
        .await?;
    let sent = sender
        .await_final_send_operation_state(send_operation)
        .await?;
    anyhow::ensure!(
        matches!(sent, LnSendState::Success(_)),
        "Lightning send ended {sent:?}"
    );
    let received = receiver
        .await_final_receive_operation_state(receive_operation)
        .await?;
    anyhow::ensure!(
        received == LnReceiveState::Claimed,
        "Lightning receive ended {received:?}"
    );
    Ok(())
}

/// Start operations at the controlled rate with exponential inter-arrival
/// times, alternating ecash and Lightning and the transfer direction. An
/// operation that would exceed the in-flight bound is counted and skipped,
/// so a slow federation shows up as a shortfall rather than a backlog.
async fn run_federation_load(
    clients: [ClientHandleArc; 2],
    gateway: SafeUrl,
    stats: Arc<OpStats>,
    mut rate: tokio::sync::watch::Receiver<f64>,
) {
    let clients = Arc::new(clients);
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut sequence: u64 = 0;
    let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15 ^ stats.as_ref() as *const OpStats as u64;
    loop {
        let ops_per_sec = *rate.borrow_and_update();
        if ops_per_sec <= 0.0 {
            let _ = rate.changed().await;
            continue;
        }
        // xorshift64*: uniform in (0, 1] for an exponential gap.
        rng_state ^= rng_state >> 12;
        rng_state ^= rng_state << 25;
        rng_state ^= rng_state >> 27;
        let uniform = ((rng_state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 1.0)
            / (1u64 << 53) as f64;
        let gap = Duration::from_secs_f64(-uniform.ln() / ops_per_sec);
        tokio::select! {
            () = tokio::time::sleep(gap) => {}
            _ = rate.changed() => continue,
        }
        if in_flight.load(Ordering::Relaxed) >= MAX_IN_FLIGHT_PER_FEDERATION {
            stats.skipped_busy.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        in_flight.fetch_add(1, Ordering::Relaxed);
        stats.started.fetch_add(1, Ordering::Relaxed);
        let (clients, gateway, stats, in_flight) = (
            clients.clone(),
            gateway.clone(),
            stats.clone(),
            in_flight.clone(),
        );
        let this = sequence;
        sequence += 1;
        tokio::spawn(async move {
            let (from, to) = if this / 2 % 2 == 0 {
                (&clients[0], &clients[1])
            } else {
                (&clients[1], &clients[0])
            };
            let started = std::time::Instant::now();
            let lightning = this % 2 == 1;
            let result = if lightning {
                run_ln_payment(from, to, &gateway).await
            } else {
                run_ecash_transfer(from, to).await
            };
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            stats.latency_ms_total.fetch_add(elapsed, Ordering::Relaxed);
            let counter = match (lightning, result.is_ok()) {
                (false, true) => &stats.ecash_ok,
                (false, false) => &stats.ecash_err,
                (true, true) => &stats.ln_ok,
                (true, false) => &stats.ln_err,
            };
            counter.fetch_add(1, Ordering::Relaxed);
            if let Err(error) = result {
                eprintln!("seat-sim: operation failed: {error:#}");
            }
            in_flight.fetch_sub(1, Ordering::Relaxed);
        });
    }
}
