//! A guardian of a formed federation moves to an MG that joined later,
//! driven by `fa-cli` as a consumer would drive it.

use std::collections::BTreeMap;

use super::*;

const TRANSFER_TEST_TIMEOUT: Duration = Duration::from_secs(600);
const TRANSFER_PORT_BASE: u16 = 46_000;
/// Offset in an MG's port block for its pinned service endpoint, above its
/// one seat's ports.
const ENDPOINT_PORT_OFFSET: u16 = 99;

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
    let publisher =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?
            .public_key()
            .to_string();
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &publisher,
    };
    let progress = |step: &str| eprintln!("guardian transfer [{:?}]: {step}", started_at.elapsed());
    let temp = mg_e2e_temp_dir()?;
    eprintln!("guardian transfer E2E data dir: {}", temp.display());
    // One more MG than the federation has guardians: the destination.
    let overrides = local_iroh_overrides_for_grid(TRANSFER_PORT_BASE, 1, GUARDIAN_COUNT + 1);
    let (daemons, mut locators) = start_daemons(
        bin,
        &temp,
        bitcoind,
        1,
        TRANSFER_PORT_BASE,
        Some(&overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    let mut daemons = daemons.into_iter().map(Some).collect::<Vec<_>>();
    offer_free_seats(bin, &temp, GUARDIAN_COUNT).await?;
    let state_dir = temp.join("fa-state");
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &relay.url,
        &mg_endpoint_addrs(&locators),
        &overrides,
        None,
    )
    .await?;
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()?
        .federation_id();
    progress("federation formed");

    // The test moves the guardian of seat 0.
    let status = fa_status(&fa_cli_bin, &state_dir).await?;
    let source_seat_id = status["formation"]["seats"]
        .as_array()
        .and_then(|seats| seats.iter().find(|seat| seat["index"] == 0))
        .and_then(|seat| seat["seat_id"].as_str())
        .with_context(|| format!("the formed FA lists seat 0: {status}"))?
        .to_owned();
    let source_index = mg_index_of_seat(bin, &temp, &source_seat_id).await?;
    let source_dir = temp.join(format!("mg-{source_index}"));
    let source_base = TRANSFER_PORT_BASE + u16::try_from(source_index)? * 100;
    let source_material = guardian_config(&source_dir)?;
    let destination_dir = temp.join(format!("mg-{GUARDIAN_COUNT}"));
    let destination_base = TRANSFER_PORT_BASE + u16::try_from(GUARDIAN_COUNT)? * 100;
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

    let started = fa
        .run(&["start", "--seat-index", "0", "--max-msats", "0"])
        .await?;
    progress(&format!(
        "transfer started with the source offline: {started}"
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
            // A failed step leaves the transfer open for the next resume.
            let resumed = match fa.run(&["resume"]).await {
                Ok(resumed) => resumed,
                Err(error) => {
                    eprintln!("guardian transfer resume failed: {error:#}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            eprintln!("guardian transfer resumed: {resumed}");
            if resumed["phase"] == "completed" {
                return Ok::<_, anyhow::Error>(resumed);
            }
            anyhow::ensure!(
                !resumed.is_null(),
                "the transfer closed before it completed"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
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
            TRANSFER_PORT_BASE + u16::try_from(*index)? * 100,
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

/// Three guardians of a seven-guardian federation move in the same consensus
/// session. Each source must keep its guardian running until
/// the session ends: a source that stopped mid-session would let its
/// destination rejoin that session with the same key, peers would mark it a
/// forker, and three forkers exceed the fault budget of two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_guardians_move_in_one_session_without_halting_the_federation() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping guardian transfer E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    // The three handovers must land in one session for the test to mean
    // anything. Timing alone does not ensure that, so a run that observes
    // them in different sessions sets up again.
    for attempt in 1..=3 {
        match tokio::time::timeout(TRANSFER_TEST_TIMEOUT, run_three_transfers_in_one_session())
            .await
            .expect("three-transfer E2E timed out")
            .expect("three-transfer E2E failed")
        {
            SessionAttempt::Done => return,
            SessionAttempt::Split { requested } => eprintln!(
                "three transfers, attempt {attempt}: handover requests by guardian and \
                 session were {requested:?}, not three in one session; setting up again"
            ),
        }
    }
    panic!("three handovers never landed in one session");
}

/// How one run of the three-transfer scenario ended.
enum SessionAttempt {
    /// The handovers landed in one session and the federation went on.
    Done,
    /// The three handover requests were not all observed in one session.
    Split { requested: BTreeMap<u16, u64> },
}

#[allow(clippy::too_many_lines)]
async fn run_three_transfers_in_one_session() -> anyhow::Result<SessionAttempt> {
    const MOVES: usize = 3;
    const PORT_BASE: u16 = 47_000;
    let started_at = std::time::Instant::now();
    let progress = |step: &str| eprintln!("three transfers [{:?}]: {step}", started_at.elapsed());
    let bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let bin = bin.as_path();
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let fedimint_cli_bin = locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to defe from env")?;
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
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &publisher,
    };
    let temp = mg_e2e_temp_dir()?;
    eprintln!("three-transfer E2E data dir: {}", temp.display());
    let overrides = local_iroh_overrides_for_grid(PORT_BASE, 1, GUARDIAN_COUNT + MOVES);
    let (daemons, mut locators) = start_daemons(
        bin,
        &temp,
        bitcoind,
        1,
        PORT_BASE,
        Some(&overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    let mut daemons = daemons.into_iter().map(Some).collect::<Vec<_>>();
    offer_free_seats(bin, &temp, GUARDIAN_COUNT).await?;
    let state_dir = temp.join("fa-state");
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &state_dir,
        &relay.url,
        &mg_endpoint_addrs(&locators),
        &overrides,
        None,
    )
    .await?;
    progress("federation formed");

    // The guardians of seats 0..3 move; MG `GUARDIAN_COUNT + n` receives the
    // guardian of seat n.
    let status = fa_status(&fa_cli_bin, &state_dir).await?;
    let mut sources = Vec::new();
    for seat_index in 0..MOVES {
        let seat_id = status["formation"]["seats"]
            .as_array()
            .and_then(|seats| seats.iter().find(|seat| seat["index"] == seat_index))
            .and_then(|seat| seat["seat_id"].as_str())
            .with_context(|| format!("the formed FA lists seat {seat_index}: {status}"))?
            .to_owned();
        sources.push((
            seat_id.clone(),
            mg_index_of_seat(bin, &temp, &seat_id).await?,
        ));
    }
    let source_routes = mg_endpoint_addrs(&locators);

    // The destinations start without a route to any source MG, so every
    // pull waits; the sources keep running consensus meanwhile.
    let destination_dir = |n: usize| temp.join(format!("mg-{}", GUARDIAN_COUNT + n));
    let destination_base = |n: usize| PORT_BASE + u16::try_from(GUARDIAN_COUNT + n).unwrap() * 100;
    let destination_endpoint =
        |n: usize| format!("127.0.0.1:{}", destination_base(n) + ENDPOINT_PORT_OFFSET);
    let spawn_destination = |n: usize, iroh_overrides: &str, routes: Option<&str>| {
        spawn_routed_manifold_guardian(
            bin,
            &destination_dir(n),
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            destination_base(n),
            Some(iroh_overrides),
            Some(nostr),
            None,
            routes,
            Some(&destination_endpoint(n)),
        )
    };
    let mut destinations = Vec::new();
    for n in 0..MOVES {
        let mut destination = spawn_destination(n, &overrides, None)?;
        onboard_new_manifold_guardian(bin, &destination_dir(n), 1, nostr).await?;
        locators.push(read_locator(&mut destination, GUARDIAN_COUNT + n).await?);
        manifold_guardian_admin(
            bin,
            &destination_dir(n),
            &["plans", "set", "--price-msats", "0"],
        )
        .await?;
        destinations.push(destination);
    }
    let mg_endpoints = mg_endpoint_addrs(&locators);

    // fa-client keeps one transfer open at a time, so each move runs from its
    // own copy of the FA state.
    let mut destination_seats = Vec::new();
    let mut used_mgs = BTreeSet::new();
    for (n, (source_seat, _)) in sources.iter().enumerate() {
        let fa_state = temp.join(format!("fa-state-{n}"));
        let copied = std::process::Command::new("cp")
            .arg("-a")
            .arg(&state_dir)
            .arg(&fa_state)
            .status()?;
        anyhow::ensure!(copied.success(), "copy the FA state");
        let fa = TransferCli {
            bin: &fa_cli_bin,
            state_dir: &fa_state,
            relay_url: &relay.url,
            mg_endpoints: &mg_endpoints,
            iroh_overrides: &overrides,
        };
        let started = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                match fa.run(&["preview"]).await {
                    Ok(preview)
                        if preview["seat"]["advertisedPriceMsats"] == 0
                            && preview["seat"]["fmanPubkey"]
                                .as_str()
                                .is_some_and(|mg| !used_mgs.contains(mg)) =>
                    {
                        match fa
                            .run(&["start", "--seat-index", &n.to_string(), "--max-msats", "0"])
                            .await
                        {
                            Ok(started) => return started,
                            Err(error) => eprintln!("transfer start failed: {error:#}"),
                        }
                    }
                    Ok(preview) => eprintln!("transfer preview not usable yet: {preview}"),
                    Err(error) => eprintln!("transfer preview not ready: {error:#}"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await
        .context("a transfer did not start")?;
        progress(&format!("transfer {n} started: {started}"));
        anyhow::ensure!(
            started["sourceSeatId"] == source_seat.as_str(),
            "transfer {n} moves seat {n}: {started}"
        );
        used_mgs.insert(
            started["destinationFmanPubkey"]
                .as_str()
                .context("a started transfer names its destination")?
                .to_owned(),
        );
        let seat = started["destinationSeatId"]
            .as_str()
            .context("a started transfer has bought its seat")?
            .to_owned();
        // Which of the new MGs sold it.
        let mut on = None;
        for d in 0..MOVES {
            let seats =
                manifold_guardian_admin(bin, &destination_dir(d), &["seats", "list"]).await?;
            if seats["seats"]
                .as_array()
                .is_some_and(|seats| seats.iter().any(|s| s["seat_id"] == seat.as_str()))
            {
                on = Some(d);
            }
        }
        destination_seats.push((seat, on.context("a new MG holds the bought seat")?));
    }
    for (seat, d) in &destination_seats {
        wait_seat(bin, &destination_dir(*d), seat, "AwaitingTransfer", false).await?;
    }
    progress("three pulls wait for their sources");

    // A moved guardian keeps its iroh keys, so its routes follow it to the
    // new MG that bought its seat.
    let mut moved_overrides = overrides.clone();
    for ((_, source_index), (_, d)) in sources.iter().zip(&destination_seats) {
        moved_overrides = route_moved_guardian(
            &moved_overrides,
            PORT_BASE + u16::try_from(*source_index)? * 100,
            destination_base(*d),
        );
    }

    // A client that reads the session count through the guardians as
    // they are before the moves.
    let observer = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fedimint-cli-observer"),
        iroh_overrides: &overrides,
    };
    join_federation(&observer, &invite).await?;
    shutdown_daemons(destinations).await?;

    // Start the pulls right after a session boundary, so that the whole
    // next session is left for the three handover requests.
    let last = guardians_session_count(&observer).await?;
    let triggered = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match guardians_session_count(&observer).await {
                Ok(count) if last < count => return count,
                Ok(_) => {}
                Err(error) => eprintln!("session count failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("the federation completed no session before the pulls")?;
    // Watch every guardian's own status for the session its handover
    // request was made in: `shutdown_after_current_session` records it as
    // the guardian's scheduled shutdown until the guardian stops.
    let mut requested = BTreeMap::<u16, u64>::new();
    let mut destinations = (0..MOVES)
        .map(|n| spawn_destination(n, &moved_overrides, Some(&source_routes)))
        .collect::<std::io::Result<Vec<_>>>()?;
    progress(&format!("pulls start in session {triggered}"));
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            for (peer, session) in scheduled_shutdowns(&observer).await {
                requested.entry(peer).or_insert(session);
            }
            if requested.len() >= MOVES {
                return anyhow::Ok(());
            }
            // A source that stopped before it was seen leaves its request
            // unobserved.
            let mut stopped = 0;
            for (seat, index) in &sources {
                let status = manifold_guardian_admin(
                    bin,
                    &temp.join(format!("mg-{index}")),
                    &["seats", "status", seat],
                )
                .await?;
                if status["report"]["phase"] == "Transferred" {
                    stopped += 1;
                }
            }
            if stopped == MOVES {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("no handover request was observed")??;
    for (n, destination) in destinations.iter_mut().enumerate() {
        read_locator(destination, GUARDIAN_COUNT + n).await?;
    }
    let sessions = requested.values().copied().collect::<BTreeSet<_>>();
    let request_session = match (requested.len(), sessions.first()) {
        (MOVES, Some(session)) if sessions.len() == 1 => *session,
        _ => {
            let mut remaining = daemons.into_iter().flatten().collect::<Vec<_>>();
            remaining.extend(destinations);
            shutdown_daemons(remaining).await?;
            defe.release(relay_lease.handle_id).await?;
            defe.release(bitcoind_lease.handle_id).await?;
            std::fs::remove_dir_all(&temp).context("remove three-transfer E2E tempdir")?;
            return Ok(SessionAttempt::Split { requested });
        }
    };
    progress(&format!(
        "all three handover requests landed in session {request_session}"
    ));
    // A source stops only at the end of the session, which may run long.
    for (seat, index) in &sources {
        wait_seat_for(
            bin,
            &temp.join(format!("mg-{index}")),
            seat,
            "Transferred",
            false,
            Duration::from_secs(300),
        )
        .await?;
    }
    progress("all three sources stopped");
    for (seat, d) in &destination_seats {
        wait_seat(bin, &destination_dir(*d), seat, "running", false).await?;
    }
    progress("all three destinations run their guardians");
    // The MGs that keep their guardians restart with routes to the moved
    // ones; fedimintd resumes its session from its own database.
    let kept = (0..GUARDIAN_COUNT)
        .filter(|index| !sources.iter().any(|(_, source)| source == index))
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
            PORT_BASE + u16::try_from(*index)? * 100,
            Some(&moved_overrides),
            Some(nostr),
            None,
        )?;
        read_locator(&mut daemon, *index).await?;
        daemons[*index] = Some(daemon);
    }

    // A halted federation answers its last session count forever. A live
    // one completes a session about every ten seconds under defe.
    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fedimint-cli-client"),
        iroh_overrides: &moved_overrides,
    };
    join_federation(&fedimint_cli, &invite).await?;
    // The session of the requests ends with count `request_session + 1`;
    // the federation must complete three more after it.
    let target = request_session + 4;
    let after = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match session_count(&fedimint_cli, Duration::from_secs(10)).await {
                Ok(count) if count >= target => return count,
                Ok(count) => eprintln!("session count {count}"),
                Err(error) => eprintln!("session count failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await
    .context("the federation stopped completing sessions after three moves")?;
    progress(&format!(
        "federation completed sessions {}..{after} after the requests' session",
        request_session + 1
    ));

    let mut remaining = daemons.into_iter().flatten().collect::<Vec<_>>();
    remaining.extend(destinations);
    shutdown_daemons(remaining).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove three-transfer E2E tempdir")?;
    Ok(SessionAttempt::Done)
}

/// Join `cli` to the federation, retrying while guardians restart.
async fn join_federation(cli: &FedimintCli<'_>, invite: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let _ = std::fs::remove_dir_all(&cli.data_dir);
            match cli
                .run(&["join-federation", invite], FEDIMINT_CLI_JOIN_TIMEOUT)
                .await
            {
                Ok(_) => return,
                Err(error) => eprintln!("fedimint-cli join failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("fedimint-cli did not join")
}

/// The session in which each guardian's handover request was made, by
/// guardian, read from the guardians' own status. Guardians that are
/// unreachable or have no scheduled shutdown are left out.
async fn scheduled_shutdowns(cli: &FedimintCli<'_>) -> Vec<(u16, u64)> {
    futures_util::future::join_all((0..GUARDIAN_COUNT).map(|peer| async move {
        let peer = u16::try_from(peer).ok()?;
        let output = cli
            .run_json(
                &["dev", "api", "--peer-id", &peer.to_string(), "status"],
                FEDIMINT_CLI_META_TIMEOUT,
            )
            .await
            .ok()?;
        Some((
            peer,
            output["value"]["federation"]["scheduled_shutdown"].as_u64()?,
        ))
    }))
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// The highest session count that any guardian reports on its own. Unlike
/// the federation's count, it needs no threshold, so it can be read while
/// moved guardians are unreachable.
async fn guardians_session_count(cli: &FedimintCli<'_>) -> anyhow::Result<u64> {
    let counts = futures_util::future::join_all((0..GUARDIAN_COUNT).map(|peer| async move {
        let output = cli
            .run_json(
                &[
                    "dev",
                    "api",
                    "--peer-id",
                    &peer.to_string(),
                    "session_count",
                ],
                FEDIMINT_CLI_META_TIMEOUT,
            )
            .await
            .ok()?;
        output["value"].as_u64()
    }))
    .await;
    counts
        .into_iter()
        .flatten()
        .max()
        .context("no guardian answered its session count")
}

/// The federation's session count, retrying for up to `wait` until a
/// threshold of guardians answers.
async fn session_count(cli: &FedimintCli<'_>, wait: Duration) -> anyhow::Result<u64> {
    tokio::time::timeout(wait, async {
        loop {
            match cli
                .run_json(&["dev", "session-count"], FEDIMINT_CLI_META_TIMEOUT)
                .await
                .and_then(|output| {
                    output["count"]
                        .as_u64()
                        .with_context(|| format!("session count output: {output}"))
                }) {
                Ok(count) => return count,
                Err(error) => eprintln!("session count failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("no threshold of guardians answered the session count")
}

/// `fa-cli transfer` against this test's MGs.
#[derive(Clone, Copy)]
struct TransferCli<'a> {
    bin: &'a Path,
    state_dir: &'a Path,
    relay_url: &'a str,
    mg_endpoints: &'a str,
    iroh_overrides: &'a str,
}

impl TransferCli<'_> {
    async fn run(&self, args: &[&str]) -> anyhow::Result<serde_json::Value> {
        let mut command = Command::new(self.bin);
        command
            .arg("--state-dir")
            .arg(self.state_dir)
            .arg("--json")
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
    wait_seat_for(bin, root, seat_id, phase, healthy, Duration::from_secs(120)).await
}

async fn wait_seat_for(
    bin: &Path,
    root: &Path,
    seat_id: &str,
    phase: &str,
    healthy: bool,
    wait: Duration,
) -> anyhow::Result<serde_json::Value> {
    let mut last = serde_json::Value::Null;
    tokio::time::timeout(wait, async {
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
