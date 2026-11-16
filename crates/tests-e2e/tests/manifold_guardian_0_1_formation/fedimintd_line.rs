//! A formed federation moves to the 0.13 `fedimintd` line when the
//! setup-payment policy lists it, while some of its MGs run a release without
//! the 0.13 binary and stay on 0.12. The federation keeps finishing sessions
//! on mixed lines.

use super::*;

const LINE_MOVE_TEST_TIMEOUT: Duration = Duration::from_secs(600);
/// Time for every MG to admit the policy, reach its next healthy probe (one
/// a minute), and let its guardian finish a session.
const LINE_MOVE_TIMEOUT: Duration = Duration::from_secs(300);
const SESSION_PROGRESS_TIMEOUT: Duration = Duration::from_secs(180);
/// MGs whose release lacks the 0.13 binary. Seven guardians need five to
/// finish a session, so the four that move and these three can only finish
/// one together.
const STAYING_ON_0_12: usize = 3;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listed_federation_moves_to_0_13_and_finishes_sessions_on_mixed_lines_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping fedimintd line E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    tokio::time::timeout(LINE_MOVE_TEST_TIMEOUT, run_line_move())
        .await
        .expect("fedimintd line E2E timed out")
        .expect("fedimintd line E2E failed");
}

#[allow(clippy::too_many_lines)]
async fn run_line_move() -> anyhow::Result<()> {
    let started_at = std::time::Instant::now();
    let progress = |step: &str| eprintln!("fedimintd line [{:?}]: {step}", started_at.elapsed());
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    // The release ships the 0.13 line beside the MG binary.
    let fedimintd_0_13_bin = manifold_guardian_bin.with_file_name("fedimintd-0-13");
    anyhow::ensure!(
        fedimintd_0_13_bin.is_file(),
        "{} is missing: build crates/mg/fedimintd-0.13 and place its binary beside \
         fleet-manager, or point {MANIFOLD_GUARDIAN_BIN_ENV} at the Nix runtime bundle",
        fedimintd_0_13_bin.display()
    );
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
    let setup_payment_keys =
        NostrKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")?;
    let publisher = setup_payment_keys.public_key().to_string();
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &publisher,
    };
    let temp = mg_e2e_temp_dir()?;
    eprintln!("fedimintd line E2E data dir: {}", temp.display());

    // Two installs of one release: complete, and without the 0.13 binary.
    // The harness runs its test issuer from beside the MG binary.
    let complete = temp.join("release");
    let without_0_13 = temp.join("release-0.12");
    for dir in [&complete, &without_0_13] {
        std::fs::create_dir_all(dir)?;
        for name in ["fleet-manager", "manifold-test-issuer"] {
            std::fs::copy(manifold_guardian_bin.with_file_name(name), dir.join(name))?;
        }
    }
    std::fs::copy(&fedimintd_0_13_bin, complete.join("fedimintd-0-13"))?;
    let complete_bin = complete.join("fleet-manager");
    let without_0_13_bin = without_0_13.join("fleet-manager");
    let bins = (0..GUARDIAN_COUNT)
        .map(|index| {
            if index < STAYING_ON_0_12 {
                without_0_13_bin.as_path()
            } else {
                complete_bin.as_path()
            }
        })
        .collect::<Vec<_>>();

    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (daemons, locators) = start_daemons_from(
        &bins,
        &temp,
        bitcoind,
        1,
        port_base,
        Some(&overrides),
        Some(nostr),
        None,
    )
    .await;
    offer_free_seats(&complete_bin, &temp, GUARDIAN_COUNT).await?;
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &temp.join("fa-state"),
        &relay.url,
        &mg_endpoint_addrs(&locators),
        &overrides,
    )
    .await?;
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()?
        .federation_id();
    progress("federation formed");

    let observer = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("line-observer"),
        iroh_overrides: &overrides,
    };
    observer
        .run(&["join-federation", &invite], FEDIMINT_CLI_JOIN_TIMEOUT)
        .await?;
    let lines = guardian_lines(&observer).await?;
    anyhow::ensure!(
        lines.iter().all(|line| line == "0.12"),
        "the federation forms on 0.12: {lines:?}"
    );

    NostrRelayClient::connect(&relay.url, setup_payment_keys, Duration::from_secs(10))
        .await
        .map_err(|err| anyhow::anyhow!("connect setup-payment publisher to relay: {err}"))?
        .publish_event(
            EventBuilder::new(
                Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
                serde_json::json!({
                    "version": 1,
                    "fman_version": "0.1.0",
                    "federations": [],
                    "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
                    "fedimintd_0_13_federations": [federation_id.to_string()],
                })
                .to_string(),
            )
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
        )
        .await
        .map_err(|err| anyhow::anyhow!("publish setup-payment federations event: {err}"))?;
    progress("policy lists the federation for 0.13");

    let expected = |lines: &[String]| {
        lines.iter().filter(|line| *line == "0.13").count() == GUARDIAN_COUNT - STAYING_ON_0_12
            && lines.iter().filter(|line| *line == "0.12").count() == STAYING_ON_0_12
    };
    let lines = tokio::time::timeout(LINE_MOVE_TIMEOUT, async {
        loop {
            match guardian_lines(&observer).await {
                Ok(lines) if expected(&lines) => return lines,
                Ok(lines) => eprintln!("guardian lines: {lines:?}"),
                Err(err) => eprintln!("guardian lines unavailable: {err:#}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await
    .context("every MG with the 0.13 binary moves its guardian")?;
    progress(&format!("guardians on mixed lines: {lines:?}"));

    // Any five signers include both lines.
    let start = session_count(&observer).await?;
    tokio::time::timeout(SESSION_PROGRESS_TIMEOUT, async {
        loop {
            if session_count(&observer)
                .await
                .is_ok_and(|count| count >= start + 2)
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the federation finishes sessions on mixed lines")?;
    let lines = guardian_lines(&observer).await?;
    anyhow::ensure!(
        expected(&lines),
        "no guardian changed line again: {lines:?}"
    );
    progress("sessions finish on mixed lines");

    shutdown_daemons(daemons).await?;
    Ok(())
}

/// Each guardian's `fedimintd` major.minor line, by peer id.
async fn guardian_lines(observer: &FedimintCli<'_>) -> anyhow::Result<Vec<String>> {
    let mut lines = Vec::with_capacity(GUARDIAN_COUNT);
    for peer in 0..GUARDIAN_COUNT {
        let peer = peer.to_string();
        let response = observer
            .run_json(
                &["dev", "api", "--peer-id", &peer, "fedimintd_version"],
                Duration::from_secs(10),
            )
            .await?;
        let version = response["value"]
            .as_str()
            .with_context(|| format!("peer {peer} reports its version: {response}"))?;
        let line = version.split('.').take(2).collect::<Vec<_>>().join(".");
        lines.push(line);
    }
    Ok(lines)
}

async fn session_count(observer: &FedimintCli<'_>) -> anyhow::Result<u64> {
    let response = observer
        .run_json(&["dev", "api", "session_count"], Duration::from_secs(10))
        .await?;
    response["value"]
        .as_u64()
        .with_context(|| format!("session count: {response}"))
}
