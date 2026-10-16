//! A guardian of a formed federation moves to an MG that joined later.

use super::*;

const TRANSFER_TEST_TIMEOUT: Duration = Duration::from_secs(600);
const TRANSFER_PORT_BASE: u16 = 46_000;

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

async fn run_guardian_transfer() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
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
    let temp = mg_e2e_temp_dir()?;
    eprintln!("guardian transfer E2E data dir: {}", temp.display());
    // One more MG than the federation has guardians: the destination.
    let overrides = local_iroh_overrides_for_grid(TRANSFER_PORT_BASE, 1, GUARDIAN_COUNT + 1);
    let (mut daemons, mut locators) = start_daemons(
        &manifold_guardian_bin,
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
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
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

    // The test moves the guardian of seat 0.
    let status = fa_status(&fa_cli_bin, &state_dir).await?;
    let source_seat_id = status["formation"]["seats"]
        .as_array()
        .and_then(|seats| seats.iter().find(|seat| seat["index"] == 0))
        .and_then(|seat| seat["seat_id"].as_str())
        .with_context(|| format!("the formed FA lists seat 0: {status}"))?
        .to_owned();
    let source_index = mg_index_of_seat(&manifold_guardian_bin, &temp, &source_seat_id).await?;
    let source_dir = temp.join(format!("mg-{source_index}"));

    // The moved guardian keeps its Iroh ids but listens on the destination
    // seat's ports. This harness routes ids by static overrides, so an MG
    // reaches the moved guardian only with this route.
    let destination_dir = temp.join(format!("mg-{GUARDIAN_COUNT}"));
    let destination_base = TRANSFER_PORT_BASE + u16::try_from(GUARDIAN_COUNT)? * 100;
    let moved_overrides = route_moved_guardian(
        &overrides,
        TRANSFER_PORT_BASE + u16::try_from(source_index)? * 100,
        destination_base,
    );

    // A new MG joins after the federation formed. It dials the source MG of
    // the move, so it gets the routes of every MG that exists now.
    let mut destination = spawn_manifold_guardian_with_mg_routes(
        &manifold_guardian_bin,
        &destination_dir,
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        destination_base,
        Some(&moved_overrides),
        Some(nostr),
        None,
        Some(&mg_endpoint_addrs(&locators)),
    )?;
    onboard_new_manifold_guardian(&manifold_guardian_bin, &destination_dir, 1, nostr).await?;
    locators.push(read_locator(&mut destination, GUARDIAN_COUNT).await?);
    daemons.push(destination);
    manifold_guardian_admin(
        &manifold_guardian_bin,
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
        iroh_overrides: &overrides,
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
    eprintln!("guardian transfer started: {started}");
    anyhow::ensure!(
        started["destinationFmanPubkey"] == destination_mg.as_str(),
        "the transfer buys the previewed seat: {started}"
    );
    let source_mg = started["sourceFmanPubkey"]
        .as_str()
        .context("transfer names its source MG")?
        .to_owned();
    anyhow::ensure!(
        started["sourceSeatId"] == source_seat_id.as_str(),
        "the transfer moves seat 0: {started}"
    );
    let destination_seat_id = started["destinationSeatId"]
        .as_str()
        .context("a started transfer has bought its seat")?
        .to_owned();

    // The source answers the destination, consensus adopts the new binding,
    // and the FA removes the source seat.
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

    let source = manifold_guardian_admin(
        &manifold_guardian_bin,
        &source_dir,
        &["seats", "status", &source_seat_id],
    )
    .await?;
    anyhow::ensure!(
        source["report"]["phase"] == "Removed",
        "the source seat no longer holds the guardian: {source}"
    );
    let moved = manifold_guardian_admin(
        &manifold_guardian_bin,
        &destination_dir,
        &["seats", "status", &destination_seat_id],
    )
    .await?;
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()?
        .federation_id();
    anyhow::ensure!(
        moved["report"]["phase"] == "running"
            && moved["report"]["invite_code"]
                .as_str()
                .context("a running seat reports its invite")?
                .parse::<fedimint_core::invite_code::InviteCode>()?
                .federation_id()
                == federation_id,
        "the destination seat runs the federation's guardian: {moved}"
    );

    // Restart the MGs that stay with the route to the moved guardian.
    let destination = daemons.pop().context("the destination daemon runs")?;
    shutdown_daemons(std::mem::take(&mut daemons)).await?;
    let kept = (0..GUARDIAN_COUNT)
        .filter(|index| *index != source_index)
        .collect::<Vec<_>>();
    let mut kept_locators = Vec::with_capacity(kept.len() + 1);
    for index in &kept {
        let mut daemon = spawn_manifold_guardian(
            &manifold_guardian_bin,
            &temp.join(format!("mg-{index}")),
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            TRANSFER_PORT_BASE + u16::try_from(*index)? * 100,
            Some(&moved_overrides),
            Some(nostr),
            None,
        )?;
        kept_locators.push(read_locator(&mut daemon, *index).await?);
        daemons.push(daemon);
    }
    kept_locators.push(locators[GUARDIAN_COUNT].clone());
    daemons.push(destination);

    let fedimint_cli = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("fedimint-cli-client"),
        iroh_overrides: &moved_overrides,
    };
    fedimint_cli
        .run(&["join-federation", &invite], FEDIMINT_CLI_JOIN_TIMEOUT)
        .await?;
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
    update_federation_name(
        &fa_cli_bin,
        &state_dir,
        &mg_endpoint_addrs(&kept_locators),
        &moved_overrides,
        name,
    )
    .await?;

    // With two other guardians stopped, a read needs five of seven answers,
    // so the moved guardian must serve the consensus that set the name.
    let stopped = daemons.drain(..2).collect::<Vec<_>>();
    shutdown_daemons(stopped).await?;
    tokio::time::timeout(Duration::from_secs(60), async {
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

    shutdown_daemons(daemons).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove guardian transfer E2E tempdir")?;
    Ok(())
}

/// `fa-cli transfer` against this test's MGs.
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
async fn mg_index_of_seat(
    manifold_guardian_bin: &Path,
    temp: &Path,
    seat_id: &str,
) -> anyhow::Result<usize> {
    for index in 0..GUARDIAN_COUNT {
        let seats = manifold_guardian_admin(
            manifold_guardian_bin,
            &temp.join(format!("mg-{index}")),
            &["seats", "list"],
        )
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
