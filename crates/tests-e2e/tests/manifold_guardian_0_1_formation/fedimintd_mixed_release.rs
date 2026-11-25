//! MGs of the previous release, whose seats run fedimintd 0.12, and MGs of
//! this release, whose seats run the 0.13 line, form one federation through
//! DKG and finish sessions together, before and after the policy turns on
//! the 0.13 line's kill switches.
//!
//! The previous release runs every behaviour the kill switches restore, so
//! the federation runs with each switch off on some guardians and in effect
//! on others throughout.

use super::*;

const MIXED_RELEASE_TEST_TIMEOUT: Duration = Duration::from_secs(900);
const SESSION_PROGRESS_TIMEOUT: Duration = Duration::from_secs(180);
/// MGs on the previous release. Seven guardians need five to finish a
/// session, so the four on this release and these three can only finish one
/// together.
const PREVIOUS_RELEASE_MGS: usize = 3;

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicit old/new release binaries and isolated defe resources"]
async fn previous_and_current_release_guardians_form_and_finish_sessions_under_defe() {
    let previous = env::var_os("MG_UPGRADE_FROM_BIN")
        .map(PathBuf::from)
        .expect("MG_UPGRADE_FROM_BIN must name the previous production fleet-manager");
    tokio::time::timeout(MIXED_RELEASE_TEST_TIMEOUT, run_mixed_release(&previous))
        .await
        .expect("mixed-release E2E timed out")
        .expect("mixed-release E2E failed");
}

#[allow(clippy::too_many_lines)]
async fn run_mixed_release(previous_bin: &Path) -> anyhow::Result<()> {
    let started_at = std::time::Instant::now();
    let progress = |step: &str| eprintln!("mixed release [{:?}]: {step}", started_at.elapsed());
    let current_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
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
    eprintln!("mixed-release E2E data dir: {}", temp.display());

    // The harness runs its test issuer from beside each MG binary.
    let previous_release = temp.join("previous-release");
    std::fs::create_dir_all(&previous_release)?;
    std::fs::copy(previous_bin, previous_release.join("fleet-manager"))?;
    std::fs::copy(
        current_bin.with_file_name("manifold-test-issuer"),
        previous_release.join("manifold-test-issuer"),
    )?;
    let previous_bin = previous_release.join("fleet-manager");
    let bins = (0..GUARDIAN_COUNT)
        .map(|index| {
            if index < PREVIOUS_RELEASE_MGS {
                previous_bin.as_path()
            } else {
                current_bin.as_path()
            }
        })
        .collect::<Vec<_>>();

    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT)?;
    let overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (mut daemons, locators) = start_daemons_from(
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
    offer_free_seats(&current_bin, &temp, GUARDIAN_COUNT).await?;
    let invite = form_federation_in_state(
        &fa_cli_bin,
        &temp.join("fa-state"),
        &relay.url,
        &mg_endpoint_addrs(&locators),
        &overrides,
    )
    .await?;
    progress("federation formed");

    let observer = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("observer"),
        iroh_overrides: &overrides,
    };
    observer
        .run(&["join-federation", &invite], FEDIMINT_CLI_JOIN_TIMEOUT)
        .await?;
    // Each guardian's release marker, as its own metrics report it: the
    // fedimintd_version endpoint reports the DKG line, the same on both.
    let mut releases = Vec::with_capacity(GUARDIAN_COUNT);
    for daemon in &daemons {
        let parent = daemon.id().context("running MG has a process id")?;
        releases.push(seat_release_marker(parent).await?);
    }
    let current = releases
        .iter()
        .filter(|(version, hash)| version == FEDIMINTD_VERSION && hash == FEDIMINTD_SOURCE_REV)
        .count();
    let previous = releases
        .iter()
        .filter(|(version, _)| version.starts_with("0.12."))
        .count();
    anyhow::ensure!(
        previous == PREVIOUS_RELEASE_MGS && current == GUARDIAN_COUNT - PREVIOUS_RELEASE_MGS,
        "guardians run {releases:?}, expected {PREVIOUS_RELEASE_MGS} on 0.12 and the rest on \
         {FEDIMINTD_VERSION} at {FEDIMINTD_SOURCE_REV}"
    );
    let lines = guardian_dkg_lines(&observer).await?;
    anyhow::ensure!(
        lines.iter().all(|line| line.starts_with("0.12.")),
        "every guardian reports the 0.12 DKG line, got {lines:?}"
    );
    anyhow::ensure!(
        lines.iter().filter(|line| *line == "0.12.0+fedi").count()
            >= GUARDIAN_COUNT - PREVIOUS_RELEASE_MGS,
        "the 0.13 guardians report the DKG line 0.12.0+fedi, got {lines:?}"
    );
    progress(&format!("guardians formed on mixed releases: {releases:?}"));

    wait_for_sessions(&observer, 2).await?;
    progress("sessions finish with the kill switches off");

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
                    "iroh_legacy_timeouts": true,
                    "walletv2_legacy_vote_delay": true,
                })
                .to_string(),
            )
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
        )
        .await
        .map_err(|err| anyhow::anyhow!("publish setup-payment feature policy: {err}"))?;
    progress("policy turns the kill switches on");

    // The vote delay switches on the running guardians; the iroh timeouts
    // only when a guardian next starts, so restart one MG on this release.
    let restarted = GUARDIAN_COUNT - 1;
    let restarted_seat_journal = temp.join(format!("mg-{restarted}/seats/0/safe-events"));
    tokio::time::timeout(SESSION_PROGRESS_TIMEOUT, async {
        loop {
            if journal_has_event(
                &restarted_seat_journal,
                "Walletv2 vote delay switched",
                "legacy",
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the running guardian switches its walletv2 vote delay")?;
    let mut daemon = daemons.remove(restarted);
    daemon.kill().await?;
    daemon.wait().await?;
    let port_base_of_restarted = port_base + u16::try_from(restarted).unwrap() * MG_PORT_STRIDE;
    let mut daemon = spawn_manifold_guardian(
        &current_bin,
        &temp.join(format!("mg-{restarted}")),
        &bitcoind.rpc_url,
        &bitcoind.rpc_username,
        &bitcoind.rpc_password,
        port_base_of_restarted,
        Some(&overrides),
        Some(nostr),
        None,
    )?;
    read_locator(&mut daemon, restarted).await?;
    daemons.push(daemon);
    progress("one current-release MG restarted under the policy");
    wait_for_sessions(&observer, 2).await?;
    progress("sessions finish with the kill switches on");

    shutdown_daemons(daemons).await?;
    anyhow::ensure!(
        journal_has_event(
            &restarted_seat_journal,
            "Iroh timeouts set",
            "iroh_legacy_timeouts"
        ),
        "the restarted guardian starts with the legacy iroh timeouts"
    );
    for index in PREVIOUS_RELEASE_MGS..GUARDIAN_COUNT {
        assert_safe_event_journal(
            &temp.join(format!("mg-{index}/seats/0/safe-events")),
            &["Walletv2 vote delay switched"],
        )?;
    }
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp).context("remove mixed-release E2E tempdir")?;
    Ok(())
}

/// Whether the safe-event journal in `directory` holds an event with
/// `message` whose boolean field `field` is true.
fn journal_has_event(directory: &Path, message: &str, field: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        std::fs::read_to_string(entry.path()).is_ok_and(|contents| {
            contents.lines().any(|line| {
                serde_json::from_str::<serde_json::Value>(line).is_ok_and(|event| {
                    event["fields"]["message"] == message && event["fields"][field] == true
                })
            })
        })
    })
}

/// Wait until the federation finishes `sessions` more sessions.
async fn wait_for_sessions(observer: &FedimintCli<'_>, sessions: u64) -> anyhow::Result<()> {
    let start = session_count(observer).await?;
    tokio::time::timeout(SESSION_PROGRESS_TIMEOUT, async {
        loop {
            if session_count(observer)
                .await
                .is_ok_and(|count| count >= start + sessions)
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("the federation finishes sessions on mixed lines")
}

/// What each guardian answers on `fedimintd_version`, by peer id.
async fn guardian_dkg_lines(observer: &FedimintCli<'_>) -> anyhow::Result<Vec<String>> {
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
        lines.push(version.to_owned());
    }
    Ok(lines)
}

/// The `version` and `version_hash` of the `fm_app_start_ts` metric of the
/// one seat child of MG `parent`, read from its loopback metrics port.
async fn seat_release_marker(parent: u32) -> anyhow::Result<(String, String)> {
    let metrics_addr = seat_child_metrics_addr(parent)?;
    let body = reqwest::get(format!("http://{metrics_addr}/metrics"))
        .await?
        .error_for_status()?
        .text()
        .await?;
    let line = body
        .lines()
        .find(|line| line.starts_with("fm_app_start_ts{"))
        .with_context(|| format!("seat of MG {parent} exposes fm_app_start_ts"))?;
    let label = |name: &str| {
        let start = line
            .find(&format!("{name}=\""))
            .with_context(|| format!("{line} carries {name}"))?
            + name.len()
            + 2;
        let len = line[start..]
            .find('"')
            .with_context(|| format!("{line} closes {name}"))?;
        anyhow::Ok(line[start..start + len].to_owned())
    };
    Ok((label("version")?, label("version_hash")?))
}

/// The `--bind-metrics` address of the one seat child of MG `parent`.
fn seat_child_metrics_addr(parent: u32) -> anyhow::Result<String> {
    let task_dir = PathBuf::from(format!("/proc/{parent}/task"));
    for task in std::fs::read_dir(&task_dir)? {
        let Ok(children) = std::fs::read_to_string(task?.path().join("children")) else {
            continue;
        };
        for child in children.split_whitespace() {
            let Ok(cmdline) = std::fs::read(format!("/proc/{child}/cmdline")) else {
                continue;
            };
            let args = cmdline
                .split(|byte| *byte == 0)
                .map(String::from_utf8_lossy)
                .collect::<Vec<_>>();
            if let Some(position) = args.iter().position(|arg| arg == "--bind-metrics") {
                return args
                    .get(position + 1)
                    .map(ToString::to_string)
                    .with_context(|| format!("child {child} names its metrics address"));
            }
        }
    }
    anyhow::bail!("MG {parent} has no seat child binding metrics")
}

async fn session_count(observer: &FedimintCli<'_>) -> anyhow::Result<u64> {
    let response = observer
        .run_json(&["dev", "api", "session_count"], Duration::from_secs(10))
        .await?;
    response["value"]
        .as_u64()
        .with_context(|| format!("session count: {response}"))
}
