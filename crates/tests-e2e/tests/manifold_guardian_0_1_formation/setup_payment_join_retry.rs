//! A setup-payment federation the MG could not reach when the policy named it
//! must still become sellable once it is reachable, without an MG restart
//! (SPEC-setup-payment-federations).

use fedi_decentralized_service_manifold_guardian::{
    FEDIMINTD_VERSION_0_1, FederationId as WireFederationId, FederationSize, GetQuoteRequest, Plan,
};

use super::*;

/// Twice the worst case after the federation is reachable: one capped retry
/// wait (60 s) plus one bounded join attempt (30 s).
const REJOIN_TIMEOUT: Duration = Duration::from_secs(180);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mg_joins_setup_payment_federation_after_failed_first_join_under_defe() {
    if env::var_os(OPT_IN_ENV).is_none() {
        eprintln!("skipping setup-payment join retry E2E; set {OPT_IN_ENV}=1 to run");
        return;
    }
    tokio::time::timeout(POST_FORMATION_TIMEOUT, run_join_retry())
        .await
        .expect("setup-payment join retry E2E timed out")
        .expect("setup-payment join retry E2E failed");
}

async fn run_join_retry() -> anyhow::Result<()> {
    let manifold_guardian_bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let fa_cli_bin = locate_binary(FA_CLI_BIN_ENV, "fa-cli")?;
    let fedimint_cli_bin = locate_binary(FEDIMINT_CLI_BIN_ENV, "fedimint-cli")?;
    let mut defe = AsyncDefeClient::connect_from_env().await?;
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
    let setup_payment_publisher = setup_payment_keys.public_key().to_string();
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &setup_payment_publisher,
    };
    let temp = mg_e2e_temp_dir()?;
    eprintln!("setup-payment join retry data dir: {}", temp.display());

    // Guardians of the payment federation, plus one MG that only sells.
    let port_base = reserve_mg_port_grid(GUARDIAN_COUNT + 1)?;
    let overrides = local_iroh_overrides_for_grid(port_base, 1, GUARDIAN_COUNT);
    let (guardians, locators) = start_daemons(
        &manifold_guardian_bin,
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
    offer_free_seats(&manifold_guardian_bin, &temp, GUARDIAN_COUNT).await?;
    let invite = run_fa_cli(
        &fa_cli_bin,
        &relay.url,
        &mg_endpoint_addrs(&locators),
        FaCliInvocation {
            extra_args: &[],
            resume_args: None,
            wallet_secret: None,
            output: FaCliOutput::Human,
            seat_price_msats: 0,
        },
        Some(&overrides),
        GUARDIAN_COUNT,
        FA_CLI_TIMEOUT,
    )
    .await?;
    let invite = invite.trim().to_owned();
    let federation_id = invite
        .parse::<fedimint_core::invite_code::InviteCode>()
        .context("formation printed an invite code")?
        .federation_id()
        .to_string();

    // The payment federation goes dark before the seller ever sees it.
    shutdown_daemons(guardians).await?;

    let seller_temp = temp.join("seller");
    let (seller, seller_locators) = start_daemons(
        &manifold_guardian_bin,
        &seller_temp,
        bitcoind,
        1,
        port_base + u16::try_from(GUARDIAN_COUNT)? * MG_PORT_STRIDE,
        Some(&overrides),
        1,
        Some(nostr),
        None,
    )
    .await;
    let seller_dir = seller_temp.join("mg-0");
    let price = SEAT_PRICE_MSAT.to_string();
    manifold_guardian_admin(
        &manifold_guardian_bin,
        &seller_dir,
        &["plans", "set", "--price-msats", &price],
    )
    .await?;

    NostrRelayClient::connect(&relay.url, setup_payment_keys, Duration::from_secs(10))
        .await
        .map_err(|err| anyhow::anyhow!("connect setup-payment publisher to relay: {err}"))?
        .publish_event(
            EventBuilder::new(
                Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
                serde_json::json!({
                    "version": 1,
                    "fman_version": "0.1.0",
                    "federations": [invite],
                    "telemetry_registration_url": TELEMETRY_REGISTRATION_URL,
                })
                .to_string(),
            )
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
        )
        .await
        .map_err(|err| anyhow::anyhow!("publish setup-payment federations event: {err}"))?;

    let seller_journal = seller_dir.join("safe-events/fman");
    tokio::time::timeout(Duration::from_secs(180), async {
        while !journal_contains(&seller_journal, "failed to join setup-payment federation")? {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        anyhow::Ok(())
    })
    .await
    .context("seller never reported its first failed join")??;

    let endpoint = Endpoint::bind(presets::N0DisableRelay)
        .await
        .context("bind quoting FA endpoint")?;
    let seller_locator: Locator = serde_json::from_str(&seller_locators[0])?;
    let seller_client = ManifoldGuardianServiceClient::new(
        endpoint
            .connect(seller_locator.endpoint_addr, MANIFOLD_GUARDIAN_ALPN)
            .await
            .context("connect to the seller")?,
    );
    let quote = async || -> anyhow::Result<Option<ManifoldGuardianError>> {
        let response = seller_client
            .transport()
            .get_quote(GetQuoteRequest {
                fa_id: FaId(
                    secp256k1::Keypair::new(
                        &secp256k1::Secp256k1::new(),
                        &mut secp256k1::rand::thread_rng(),
                    )
                    .x_only_public_key()
                    .0,
                ),
                fedimintd_version: FEDIMINTD_VERSION_0_1.parse()?,
                federation_size: FederationSize(u16::try_from(GUARDIAN_COUNT)?),
                plan: Plan::InfiniteBestEffort {
                    price_msats: SEAT_PRICE_MSAT,
                },
                payment_federation_id: Some(WireFederationId(federation_id.clone())),
                refund_issuance: None,
            })
            .await
            .context("seller get_quote transport")?;
        Ok(response.err())
    };
    let unreachable = quote().await?;
    anyhow::ensure!(
        unreachable == Some(ManifoldGuardianError::PaymentFederationUnavailable),
        "an unjoined payment federation must be unavailable, got {unreachable:?}"
    );

    // Bring the payment federation back on its original seats.
    let mut guardians = Vec::with_capacity(GUARDIAN_COUNT);
    for index in 0..GUARDIAN_COUNT {
        let mut daemon = spawn_manifold_guardian(
            &manifold_guardian_bin,
            &temp.join(format!("mg-{index}")),
            &bitcoind.rpc_url,
            &bitcoind.rpc_username,
            &bitcoind.rpc_password,
            port_base + u16::try_from(index)? * MG_PORT_STRIDE,
            Some(&overrides),
            Some(nostr),
            None,
        )?;
        read_locator(&mut daemon, index).await?;
        guardians.push(daemon);
    }
    let probe = FedimintCli {
        bin: &fedimint_cli_bin,
        data_dir: temp.join("reachability-probe"),
        iroh_overrides: &overrides,
        esplora_url: None,
    };
    tokio::time::timeout(Duration::from_secs(120), async {
        while probe
            .run(&["join-federation", &invite], FEDIMINT_CLI_JOIN_TIMEOUT)
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .context("restarted payment federation never became joinable")?;
    eprintln!("payment federation is reachable again");

    // The seller's quote clears the payment-client gate on its own. With no
    // refund outputs requested, that surfaces as the refund refusal, which
    // only follows a successful locked-payment quote.
    let started = tokio::time::Instant::now();
    let mut last = unreachable;
    while last == Some(ManifoldGuardianError::PaymentFederationUnavailable) {
        anyhow::ensure!(
            started.elapsed() < REJOIN_TIMEOUT,
            "seller still answers {last:?} {}s after its payment federation became reachable",
            REJOIN_TIMEOUT.as_secs()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        last = quote().await?;
    }
    anyhow::ensure!(
        last == Some(ManifoldGuardianError::PlanNotOffered),
        "a joined payment federation must quote up to its refund check, got {last:?}"
    );
    eprintln!(
        "seller quotes {}s after its payment federation became reachable",
        started.elapsed().as_secs()
    );

    shutdown_daemons(seller).await?;
    shutdown_daemons(guardians).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoind_lease.handle_id).await?;
    std::fs::remove_dir_all(&temp)?;
    Ok(())
}
