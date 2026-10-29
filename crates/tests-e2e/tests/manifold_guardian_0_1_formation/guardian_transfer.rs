//! A guardian of a formed federation moves to an MG that joined later,
//! driven by `fa-cli` as a consumer would drive it.

use super::*;

const TRANSFER_TEST_TIMEOUT: Duration = Duration::from_secs(600);
/// Offset in an MG's port block for its pinned service endpoint, above its
/// one seat's ports.
const ENDPOINT_PORT_OFFSET: u16 = 99;
/// Absence after which the stopped source qualifies for replacement.
const SOURCE_OFFLINE_LIMIT_SECS: u64 = 20;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fa_moves_a_formed_guardian_to_a_new_mg_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping guardian transfer E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    tokio::time::timeout(TRANSFER_TEST_TIMEOUT, run_guardian_transfer())
        .await
        .expect("guardian transfer E2E timed out")
        .expect("guardian transfer E2E failed");
}

#[allow(clippy::too_many_lines)]
async fn run_guardian_transfer() -> anyhow::Result<()> {
    let started_at = std::time::Instant::now();
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let bin = manifold_guardian_bin.as_path();
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let fedimint_cli_bin = locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
    let bitcoind_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoind) = &bitcoind_lease.descriptor else {
        anyhow::bail!("expected bitcoind descriptor");
    };
    // Exclusive: the preview must find exactly the MGs of this test.
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = &relay_lease.descriptor else {
        anyhow::bail!("expected Nostr relay descriptor");
    };
    let setup_payment_keys =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?;
    let publisher = setup_payment_keys.public_key().to_string();
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &publisher,
    };
    let progress = |step: &str| eprintln!("guardian transfer [{:?}]: {step}", started_at.elapsed());
    let temp = mg_e2e_temp_dir()?;
    eprintln!("guardian transfer E2E data dir: {}", temp.display());
    // One more MG than the federation has guardians: the destination.
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT + 1)?;
    let overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT + 1);
    let (daemons, mut locators) = start_daemons(
        bin,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    let mut daemons = daemons.into_iter().map(Some).collect::<Vec<_>>();
    offer_free_seats(bin, &temp, GUARDIAN_COUNT).await?;
    let state_dir = temp.join("fa-state");
    // The FA replaces a guardian only through a replacement check, which
    // reads the absence limit from the setup-payment policy. A short limit
    // lets the stopped source qualify within the test.
    let setup_payment_event_file = temp.join("setup-payment-event.json");
    std::fs::write(
        &setup_payment_event_file,
        serde_json::to_vec(
            &EventBuilder::new(
                Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
                serde_json::json!({
                    "version": 1,
                    "fman_version": "0.1.0",
                    "federations": [],
                    "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
                    "max_guardian_offline_secs": SOURCE_OFFLINE_LIMIT_SECS,
                })
                .to_string(),
            )
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG))
            .sign_with_keys(&setup_payment_keys)?,
        )?,
    )?;
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &relay.url,
        &mg_endpoint_addrs(&locators),
        &overrides,
    )
    .await?;
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()?
        .federation_id();
    progress("federation formed");

    // The test moves the guardian of seat 0.
    let status = fa_status(&fa_cli_bin, &state_dir).await?;
    let source_seat_id = status["formed"]["seats"]
        .as_array()
        .and_then(|seats| seats.first())
        .and_then(|seat| seat["seat_id"].as_str())
        .with_context(|| format!("the formed FA lists seat 0: {status}"))?
        .to_owned();
    let source_index = mg_index_of_seat(bin, &temp, &source_seat_id).await?;
    let source_dir = temp.join(format!("mg-{source_index}"));
    let source_base = port_base + u16::try_from(source_index)? * MG_PORT_STRIDE;
    let source_material = guardian_config(&source_dir)?;
    let destination_dir = temp.join(format!("mg-{GUARDIAN_COUNT}"));
    let destination_base = port_base + u16::try_from(GUARDIAN_COUNT)? * MG_PORT_STRIDE;
    // The moved guardian keeps its Iroh ids but listens on the destination
    // seat's ports. This harness routes ids by static overrides, so an MG
    // reaches the moved guardian only with this route.
    let moved_overrides = route_moved_guardian(&overrides, source_base, destination_base);

    // The source MG is offline when the FA starts the move. When it comes
    // back its endpoint is pinned, so the destination gets that route now.
    let source_endpoint = format!("127.0.0.1:{}", source_base + ENDPOINT_PORT_OFFSET);
    let destination_endpoint = format!("127.0.0.1:{}", destination_base + ENDPOINT_PORT_OFFSET);
    let source_id = serde_json::from_str::<Locator>(&locators[source_index])?
        .endpoint_addr
        .id;
    let others = locators
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != source_index)
        .map(|(_, locator)| locator.clone())
        .collect::<Vec<_>>();
    let destination_routes = format!(
        "{},{source_id}={source_endpoint}",
        mg_endpoint_addrs(&others)
    );
    shutdown_daemons(vec![daemons[source_index].take().context("source runs")?]).await?;
    let spawn_destination = || {
        spawn_routed_manifold_guardian(
            bin,
            &destination_dir,
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            destination_base,
            Some(&moved_overrides),
            Some(nostr),
            None,
            Some(&destination_routes),
            Some(&destination_endpoint),
        )
    };
    let spawn_source = || {
        spawn_routed_manifold_guardian(
            bin,
            &source_dir,
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            source_base,
            Some(&moved_overrides),
            Some(nostr),
            None,
            None,
            Some(&source_endpoint),
        )
    };

    // A new MG joins after the federation formed.
    let mut destination = spawn_destination()?;
    onboard_new_manifold_guardian(bin, &destination_dir, 1, nostr).await?;
    locators.push(read_locator(&mut destination, GUARDIAN_COUNT).await?);
    manifold_guardian_admin(
        bin,
        &destination_dir,
        &["plans", "set", "--price-msats", "0"],
    )
    .await?;
    let mg_endpoints = mg_endpoint_addrs(&locators);
    let fa = TransferCli {
        bin: &fa_cli_bin,
        state_dir: &state_dir,
        relay_url: &relay.url,
        mg_endpoints: &mg_endpoints,
        iroh_overrides: &moved_overrides,
        setup_payment_event_file: &setup_payment_event_file,
        setup_payment_publisher: &publisher,
    };

    // The preview offers only the new MG: every other MG hosts a guardian.
    let preview = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            // The MG advertises again after its plan becomes free.
            match fa.run(&["preview"]).await {
                Ok(preview) if preview["seat"]["advertisedPriceMsats"] == 0 => return preview,
                Ok(preview) => eprintln!("transfer preview not free yet: {preview}"),
                Err(error) => eprintln!("transfer preview not ready: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the new MG never became a transfer destination")?;
    let destination_mg = preview["seat"]["fmanPubkey"]
        .as_str()
        .context("preview names its MG")?
        .to_owned();

    // The check flags the source once it was offline for longer than the
    // policy allows. Until then `replace` refuses the guardian.
    let started = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match fa
                .run(&["replace", "--seat-index", "0", "--max-msats", "0"])
                .await
            {
                Ok(started) => return started,
                Err(error) => eprintln!("guardian not replaceable yet: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
    .await
    .context("the stopped source never qualified for replacement")?;
    progress(&format!(
        "replacement started with the source offline: {started}"
    ));
    anyhow::ensure!(
        started["phase"] == "awaiting_source"
            && started["sourceSeatId"] == source_seat_id.as_str()
            && started["destinationFmanPubkey"] == destination_mg.as_str(),
        "the transfer moves seat 0 onto the previewed MG: {started}"
    );
    let source_mg = started["sourceFmanPubkey"]
        .as_str()
        .context("transfer names its source MG")?
        .to_owned();
    let destination_seat_id = started["destinationSeatId"]
        .as_str()
        .context("a started transfer has bought its seat")?
        .to_owned();
    wait_seat(
        bin,
        &destination_dir,
        &destination_seat_id,
        "AwaitingTransfer",
        false,
    )
    .await?;

    // From here the FA stays offline. The destination keeps its pull across
    // a restart, and finishes on its own once the source is back.
    shutdown_daemons(vec![destination]).await?;
    let mut destination = spawn_destination()?;
    locators[GUARDIAN_COUNT] = read_locator(&mut destination, GUARDIAN_COUNT).await?;
    wait_seat(
        bin,
        &destination_dir,
        &destination_seat_id,
        "AwaitingTransfer",
        false,
    )
    .await?;
    let mut source = spawn_source()?;
    locators[source_index] = read_locator(&mut source, source_index).await?;
    daemons[source_index] = Some(source);
    wait_seat(bin, &destination_dir, &destination_seat_id, "running", true).await?;
    anyhow::ensure!(
        guardian_config(&destination_dir)? == source_material,
        "the destination runs the source's guardian config"
    );
    progress("destination runs the guardian without the FA");

    // The FA comes back, publishes the binding, and removes the source seat.
    let mg_endpoints = mg_endpoint_addrs(&locators);
    let fa = TransferCli {
        mg_endpoints: &mg_endpoints,
        ..fa
    };
    let finished = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            // `run` returns at its run timeout with the transfer still open,
            // and a failed step leaves it open for the next run.
            let ran = match fa.run(&["run"]).await {
                Ok(ran) => ran,
                Err(error) => {
                    eprintln!("guardian transfer run failed: {error:#}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            eprintln!("guardian transfer ran: {ran}");
            if ran["phase"] == "completed" {
                return Ok::<_, anyhow::Error>(ran);
            }
            anyhow::ensure!(!ran.is_null(), "the transfer closed before it completed");
        }
    })
    .await
    .context("the guardian transfer did not complete")??;
    anyhow::ensure!(
        finished["destinationSeatId"] == destination_seat_id.as_str(),
        "the transfer completes on the seat it bought: {finished}"
    );
    anyhow::ensure!(
        fa.run(&["status"]).await?.is_null(),
        "a completed transfer is closed"
    );
    progress("transfer completed");

    anyhow::ensure!(
        !source_dir.join("seats/0/data").exists(),
        "the removed source keeps no guardian data"
    );
    let source_status = wait_seat(bin, &source_dir, &source_seat_id, "Removed", false).await?;
    eprintln!("removed source seat: {source_status}");
    // A removed source stays removed, and keeps its payment wallet.
    shutdown_daemons(vec![daemons[source_index].take().context("source runs")?]).await?;
    let mut source = spawn_source()?;
    locators[source_index] = read_locator(&mut source, source_index).await?;
    wait_seat(bin, &source_dir, &source_seat_id, "Removed", false).await?;
    #[cfg(target_os = "linux")]
    anyhow::ensure!(
        find_direct_child_named(source.id().context("source runs")?, "fedimintd").is_err(),
        "the removed source started its guardian after a restart"
    );
    anyhow::ensure!(
        source_dir.join("wallet/client.db").is_dir(),
        "the removed source kept its payment wallet"
    );
    daemons[source_index] = Some(source);

    // The MGs that stay restart with the route to the moved guardian.
    let kept = (0..GUARDIAN_COUNT)
        .filter(|index| *index != source_index)
        .collect::<Vec<_>>();
    shutdown_daemons(
        kept.iter()
            .map(|index| daemons[*index].take().context("guardian MG runs"))
            .collect::<anyhow::Result<Vec<_>>>()?,
    )
    .await?;
    for index in &kept {
        let mut daemon = spawn_manifold_guardian(
            bin,
            &temp.join(format!("mg-{index}")),
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            port_base + u16::try_from(*index)? * MG_PORT_STRIDE,
            Some(&moved_overrides),
            Some(nostr),
            None,
        )?;
        locators[*index] = read_locator(&mut daemon, *index).await?;
        daemons[*index] = Some(daemon);
    }

    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fedimint-cli-client"),
        iroh_overrides: &moved_overrides,
    };
    // The restarted guardians need a moment before they answer a join.
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            // A join that timed out may leave a partial client behind.
            let _ = std::fs::remove_dir_all(&fedimint_cli.data_dir);
            match fedimint_cli
                .run(&["join-federation", &invite], FEDIMINT_CLI_JOIN_TIMEOUT)
                .await
            {
                Ok(_) => return,
                Err(error) => eprintln!("fedimint-cli join failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("fedimint-cli did not join after the guardians restarted")?;
    wait_for_seat_bindings_consensus(&fedimint_cli, GUARDIAN_COUNT).await?;
    let meta = fedimint_cli
        .run_json(&["module", "meta", "get"], FEDIMINT_CLI_META_TIMEOUT)
        .await?;
    let bindings = MgSeatBindings::parse_canonical(
        meta["value"][MG_SEAT_BINDINGS_META_FIELD_KEY]
            .as_str()
            .context("consensus carries the seat bindings")?,
    )
    .map_err(|error| anyhow::anyhow!("seat bindings are canonical: {error:?}"))?;
    let hosts = bindings
        .seat_bindings()
        .iter()
        .map(|binding| {
            binding
                .verify()
                .map(|statement| statement.mg_pubkey.0)
                .map_err(|error| anyhow::anyhow!("seat binding verifies: {error:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    anyhow::ensure!(
        hosts.contains(&destination_mg) && !hosts.contains(&source_mg),
        "consensus names the destination for the moved guardian: {hosts:?}"
    );

    // Maintenance asks every guardian, the moved one through its new MG.
    let name = "moved-guardian-e2e";
    let guardian_locators = kept
        .iter()
        .map(|index| locators[*index].clone())
        .chain([locators[GUARDIAN_COUNT].clone()])
        .collect::<Vec<_>>();
    update_federation_name(
        &fa_cli_bin,
        &state_dir,
        &mg_endpoint_addrs(&guardian_locators),
        &moved_overrides,
        name,
    )
    .await?;
    progress("metadata updated through the moved guardian");

    // The destination restores the moved guardian from its mnemonic and the
    // relay alone, on a new host.
    wait_for_settled_backup(bin, &destination_dir, 1).await?;
    let mnemonic =
        manifold_guardian_admin(bin, &destination_dir, &["show-mnemonic"]).await?["mnemonic"]
            .as_str()
            .context("show-mnemonic returns the root phrase")?
            .to_owned();
    shutdown_daemons(vec![destination]).await?;
    let mnemonic_file = temp.join("destination-mnemonic");
    write_sensitive_file(&mnemonic_file, &mnemonic)?;
    let restored_dir = temp.join("restored-destination");
    let mut restored = spawn_manifold_guardian(
        bin,
        &restored_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        destination_base,
        Some(&moved_overrides),
        Some(nostr),
        None,
    )?;
    retry_manifold_guardian_admin(
        bin,
        &restored_dir,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &mnemonic_file.display().to_string(),
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    complete_onboarding_stages(bin, &restored_dir, 1, nostr).await?;
    read_locator(&mut restored, GUARDIAN_COUNT).await?;
    let restored_seat =
        wait_seat(bin, &restored_dir, &destination_seat_id, "running", true).await?;
    anyhow::ensure!(
        restored_seat["report"]["invite_code"]
            .as_str()
            .context("a running seat reports its invite")?
            .parse::<fedimint_core::invite_code::InviteCode>()?
            .federation_id()
            == federation_id,
        "the restored seat runs the federation's guardian: {restored_seat}"
    );
    anyhow::ensure!(
        guardian_config(&restored_dir)? == source_material,
        "the restored seat runs the moved guardian's config"
    );
    progress("moved guardian restored from the destination mnemonic");

    // The restored guardian catches up while every guardian runs. The
    // guardians that dial it keep their connection to the stopped destination
    // until its QUIC idle timeout (60 s) and only then reach the restored one,
    // so with two guardians stopped first, consensus could not move it to the
    // latest session before the read below gave up.
    let moved_peer = guardian_peer_id(&restored_dir)?;
    let reference_peer = guardian_peer_id(&temp.join(format!("mg-{}", kept[2])))?;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match (
                peer_meta(&fedimint_cli, &moved_peer).await,
                peer_meta(&fedimint_cli, &reference_peer).await,
            ) {
                (Ok(moved), Ok(reference)) if !moved.is_null() && moved == reference => {
                    return;
                }
                (moved, reference) => {
                    eprintln!("moved guardian has not caught up: {moved:?} vs {reference:?}");
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the restored moved guardian did not catch up with consensus")?;
    progress("moved guardian caught up");

    // With two other guardians stopped, a read needs five of seven answers,
    // so the restored moved guardian must serve the consensus that set the
    // name.
    let stopped = kept[..2]
        .iter()
        .map(|index| daemons[*index].take().context("guardian MG runs"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    shutdown_daemons(stopped).await?;
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match fedimint_cli
                .run_json(&["module", "meta", "get"], FEDIMINT_CLI_META_TIMEOUT)
                .await
            {
                Ok(meta) if meta["value"]["federation_name"] == name => {
                    return Ok::<_, anyhow::Error>(());
                }
                Ok(meta) => eprintln!("consensus does not name the federation yet: {meta}"),
                Err(error) => eprintln!("consensus read failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the moved guardian did not serve the latest consensus")??;
    progress("moved guardian serves consensus");

    let mut remaining = daemons.into_iter().flatten().collect::<Vec<_>>();
    remaining.push(restored);
    shutdown_daemons(remaining).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove guardian transfer E2E tempdir")?;
    Ok(())
}

/// `fa-cli transfer` against this test's MGs.
#[derive(Clone, Copy)]
struct TransferCli<'a> {
    bin: &'a Path,
    state_dir: &'a Path,
    relay_url: &'a str,
    mg_endpoints: &'a str,
    iroh_overrides: &'a str,
    setup_payment_event_file: &'a Path,
    setup_payment_publisher: &'a str,
}

impl TransferCli<'_> {
    async fn run(&self, args: &[&str]) -> anyhow::Result<serde_json::Value> {
        let mut command = Command::new(self.bin);
        command
            .arg("--state-dir")
            .arg(self.state_dir)
            .arg("--json")
            .arg("--setup-payment-event-file")
            .arg(self.setup_payment_event_file)
            .arg("--setup-payment-publisher")
            .arg(self.setup_payment_publisher)
            .arg("transfer")
            .args([
                "--poll-interval-secs",
                "1",
                "--run-timeout-secs",
                "60",
                "--request-timeout-secs",
                "10",
            ])
            .args(args)
            .env("MG_E2E_LOCAL_IROH", "1")
            .env(MG_ENDPOINT_ADDRS_ENV, self.mg_endpoints)
            .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", self.iroh_overrides)
            .env(
                fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                self.relay_url,
            );
        let output = run_expect_success(
            command,
            &format!("fa-cli transfer {}", args.join(" ")),
            Duration::from_secs(90),
        )
        .await?;
        serde_json::from_str(output.trim()).context("parse fa-cli transfer JSON")
    }
}

/// The index of the MG among the federation's that holds `seat_id`.
async fn mg_index_of_seat(bin: &Path, temp: &Path, seat_id: &str) -> anyhow::Result<usize> {
    for index in 0..GUARDIAN_COUNT {
        let seats =
            manifold_guardian_admin(bin, &temp.join(format!("mg-{index}")), &["seats", "list"])
                .await?;
        let holds = seats["seats"]
            .as_array()
            .is_some_and(|seats| seats.iter().any(|seat| seat["seat_id"] == seat_id));
        if holds {
            return Ok(index);
        }
    }
    anyhow::bail!("no MG holds seat {seat_id}")
}

/// Wait until the MG at `root` reports `phase` for `seat_id`, and health
/// `healthy` when asked.
async fn wait_seat(
    bin: &Path,
    root: &Path,
    seat_id: &str,
    phase: &str,
    healthy: bool,
) -> anyhow::Result<serde_json::Value> {
    let mut last = serde_json::Value::Null;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if let Ok(status) =
                manifold_guardian_admin(bin, root, &["seats", "status", seat_id]).await
            {
                if status["report"]["phase"] == phase
                    && (!healthy || status["report"]["health"] == "healthy")
                {
                    return status;
                }
                last = status;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "seat {seat_id} on {} did not reach {phase}: {last}",
            root.display()
        )
    })
}

/// The guardian config files of the MG's one seat.
fn guardian_config(root: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    ["local.json", "consensus.json", "private.json"]
        .into_iter()
        .map(|file| {
            let path = root.join("seats/0/data").join(file);
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            Ok(serde_json::from_slice(&bytes)?)
        })
        .collect()
}

/// The peer id of the guardian on the MG's one seat.
fn guardian_peer_id(root: &Path) -> anyhow::Result<String> {
    Ok(guardian_config(root)?[0]["identity"]
        .as_u64()
        .context("local.json names the guardian's peer id")?
        .to_string())
}

/// The meta consensus value one guardian answers, read from that guardian
/// alone.
async fn peer_meta(
    fedimint_cli: &FedimintCli<'_>,
    peer: &str,
) -> anyhow::Result<serde_json::Value> {
    fedimint_cli
        .run_json(
            &[
                "dev",
                "api",
                "--peer-id",
                peer,
                "--module",
                "meta",
                "get_consensus",
                "0",
            ],
            FEDIMINT_CLI_META_TIMEOUT,
        )
        .await
}

/// `overrides` with the Iroh ids of the guardian that ran on the seat at
/// `source_base` routed to the seat at `destination_base`.
fn route_moved_guardian(overrides: &str, source_base: u16, destination_base: u16) -> String {
    let moved = [
        (source_base, destination_base, b"p2p".as_slice()),
        (source_base + 1, destination_base + 1, b"api".as_slice()),
    ]
    .map(|(from, to, role)| {
        let node_id: NodeId = SecretKey::from_bytes(&e2e_iroh_key(from, role)).public();
        (node_id.to_string(), format!("127.0.0.1:{to}"))
    });
    overrides
        .split(',')
        .map(|entry| {
            let (node, address) = entry.split_once('=').expect("local override pair");
            let address = moved
                .iter()
                .find(|(id, _)| id == node)
                .map_or(address, |(_, to)| to.as_str());
            format!("{node}={address}")
        })
        .collect::<Vec<_>>()
        .join(",")
}
