use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fi_client_restarts_interrupted_message_exchange_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() || !cfg!(target_os = "linux") {
        return;
    }
    tokio::time::timeout(FORMATION_TIMEOUT, run_restart())
        .await
        .expect("explicit DKG restart E2E timed out")
        .expect("explicit DKG restart E2E failed");
}

async fn run_restart() -> anyhow::Result<()> {
    let fleet_manager_bin = locate_binary(FLEET_MANAGER_BIN_ENV, "fleet-manager")?;
    let fi_cli_bin = locate_binary(FI_CLI_BIN_ENV, "fi-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env().await?;
    let bitcoin_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoin) = &bitcoin_lease.descriptor else {
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
    let temp = fman_e2e_temp_dir()?;
    let overrides = local_iroh_overrides_for_grid(42_000, 1, GUARDIAN_COUNT);
    let (daemons, locators) = start_daemons(
        &fleet_manager_bin,
        &temp,
        bitcoin,
        1,
        42_000,
        Some(&overrides),
        GUARDIAN_COUNT,
        Some(NostrEnv {
            relay_urls: &relay.url,
            holder_relay_url: &relay.url,
            setup_payment_publisher: &publisher,
        }),
        None,
    )
    .await;
    offer_free_seats(&fleet_manager_bin, &temp, GUARDIAN_COUNT).await?;

    let state_dir = temp.join("fi-state");
    let mut init = Command::new(&fi_cli_bin);
    init.arg("--state-dir").arg(&state_dir).arg("init");
    run_expect_success(init, "fi-cli init", Duration::from_secs(10)).await?;
    let account_file = write_fi_fee_account_fixture(&state_dir)?;
    let command = |verb: &str| {
        let mut command = Command::new(&fi_cli_bin);
        command
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--json")
            .arg(verb)
            .arg("--fi-spv2-account-file")
            .arg(&account_file)
            .env("FMAN_E2E_LOCAL_IROH", "1")
            .env("FM_IROH_CONNECT_OVERRIDES_PLAIN", &overrides)
            .env(
                fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                &relay.url,
            );
        command
    };

    // Only DKG peer 0 pauses. It actively reconnects after replacement, so
    // this test need not wait for other peers' dead-connection timeouts.
    let pause = |index| temp.join(format!("fman-{index}/seats/0/data.pause-dkg"));
    for index in 0..GUARDIAN_COUNT {
        std::fs::create_dir_all(pause(index).parent().unwrap())?;
        std::fs::write(pause(index), b"")?;
    }
    let mut create = command("create");
    create
        .arg("--federation-size")
        .arg(GUARDIAN_COUNT.to_string())
        .args(["--poll-interval-secs", "1", "--poll-timeout-secs", "15"]);
    for locator in &locators {
        create.arg("--locator").arg(locator);
    }
    let mut fi = create
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;

    let victim = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(index) =
                (0..GUARDIAN_COUNT).find(|&index| pause(index).with_extension("received").exists())
            {
                return index;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("guardian did not receive a G1 message")?;
    let parent = daemons[victim].id().context("victim's FMan exited")?;
    let child = find_direct_child_named(parent, "fedimintd")?;
    ExactProcess::open_direct_child(parent, child, "fedimintd")?.signal(libc::SIGKILL)?;
    for index in 0..GUARDIAN_COUNT {
        std::fs::remove_file(pause(index))?;
    }
    anyhow::ensure!(
        !temp.join(format!("fman-{victim}/seats/0/data")).exists(),
        "victim must not have completed DKG"
    );
    wait_for_replacement_child(parent, child).await?;
    anyhow::ensure!(
        !fi.wait().await?.success(),
        "interrupted formation unexpectedly completed"
    );

    let mut resume = command("resume");
    resume.args(["--run-timeout-secs", "10"]);
    let failed =
        tokio::time::timeout(Duration::from_secs(20), resume.kill_on_drop(true).output()).await??;
    let error = String::from_utf8_lossy(&failed.stderr);
    anyhow::ensure!(
        !failed.status.success() && error.contains("formation timed out"),
        "ordinary resume must time out on the broken ceremony: {error}"
    );
    anyhow::ensure!(
        journal_message_count(
            &temp.join(format!("fman-{victim}/safe-events/fman")),
            "driven DKG start was observed"
        )? >= 2,
        "resume must start the replacement child"
    );
    let mut protocol_failed = false;
    for index in 0..GUARDIAN_COUNT {
        protocol_failed |= journal_contains(
            &temp.join(format!("fman-{index}/seats/0/safe-events")),
            "distributed key generation failed",
        )?;
    }
    anyhow::ensure!(
        protocol_failed,
        "require a protocol failure, not just a timeout"
    );

    run_expect_success(
        command("restart-dkg"),
        "fi-cli restart-dkg",
        Duration::from_secs(60),
    )
    .await?;
    let formed =
        run_expect_success(command("resume"), "fi-cli resume", Duration::from_secs(90)).await?;
    let formed: serde_json::Value = serde_json::from_str(formed.trim())?;
    anyhow::ensure!(
        formed["formation"]["phase"] == "formed" && formed["formation"]["invite_code"].is_string(),
        "restarted formation must finish with an agreed invite: {formed}"
    );
    for index in 0..GUARDIAN_COUNT {
        let starts = journal_message_count(
            &temp.join(format!("fman-{index}/safe-events/fman")),
            "driven DKG start was observed",
        )?;
        let minimum = if index == victim { 3 } else { 2 };
        anyhow::ensure!(
            starts >= minimum,
            "guardian {index}: {starts} starts, expected {minimum}"
        );
    }
    shutdown_daemons(daemons).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoin_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp)?;
    Ok(())
}
