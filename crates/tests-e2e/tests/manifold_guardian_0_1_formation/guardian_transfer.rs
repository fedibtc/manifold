//! Opt-in native transfer gate. The FA is a real library consumer, not a
//! protocol reimplementation or a pinned-selection shortcut.

use super::*;
use fa_client::{
    FaClient, FaFeeAccountError, FaFeeAccountProvider, FaIdentity, FaStatus,
    FederationConsensusError, FederationConsensusReader, FederationConsensusSnapshot,
    FormationRunOptions, FormationRunOptionsConfig, GuardianTransferPhase, MaintenanceRunOptions,
    MaintenanceRunOptionsConfig, ManifoldGuardianCallError, ManifoldGuardianConnector,
    ManifoldGuardianConnectorError, UnavailablePayments,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::attester::{
    ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG,
    NOSTR_REVOCATION_LOCATION_PROTOCOL,
};
use fedi_decentralized_nostr_clients::NostrFaClient;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_manifold_guardian::{
    FmResult, GetAvailabilityRequest, GetAvailabilityResponse, GetQuoteRequest, GetQuoteResponse,
    SignedResponse,
};
use fedimint_core::db::{Database, IRawDatabaseExt as _, mem_impl::MemDatabase};

const ROOT_ENV: &str = "MG_TRANSFER_E2E_ROOT";
const ROUTES_ENV: &str = "MG_E2E_TRANSFER_SERVICE_ROUTES";
const PORT_BASE: u16 = 48_000;

#[tokio::test]
async fn native_guardian_transfer_under_defe() -> anyhow::Result<()> {
    if env::var_os(OPT_IN_ENV).is_none() || !cfg!(target_os = "linux") {
        return Ok(());
    }
    // Connector environment belongs to this isolated process, never to other
    // concurrently running Rust tests.
    let temp = mg_e2e_temp_dir()?;
    let mut command = Command::new(env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "guardian_transfer::native_guardian_transfer_child",
            "--nocapture",
        ])
        .env(ROOT_ENV, &temp)
        .env(ROUTES_ENV, temp.join("service-routes.json"))
        .env("MG_E2E_LOCAL_IROH", "1")
        .env(
            "FM_IROH_CONNECT_OVERRIDES_PLAIN",
            local_iroh_overrides_for_grid(PORT_BASE, 1, GUARDIAN_COUNT),
        );
    run_expect_success(command, "native transfer child", Duration::from_secs(180)).await?;
    std::fs::remove_dir_all(temp)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "isolated child of the native transfer gate"]
async fn native_guardian_transfer_child() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    fedimint_core::rustls::install_crypto_provider().await;
    let temp = PathBuf::from(env::var_os(ROOT_ENV).context("parent supplies root")?);
    let bin = locate_binary(MANIFOLD_GUARDIAN_BIN_ENV, "fleet-manager")?;
    let mut defe = AsyncDefeClient::connect_from_env().await?;
    let bitcoin_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoin) = &bitcoin_lease.descriptor else {
        anyhow::bail!("expected bitcoind");
    };
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = &relay_lease.descriptor else {
        anyhow::bail!("expected relay");
    };
    // Onboarding publishes the profile's pinned authority on the advertisement
    // relay. Keep the FA's explicitly test-only authority on a separate relay,
    // so later onboarding cannot replace its isolated revocation locations.
    let authority_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(authority_relay) = &authority_lease.descriptor else {
        anyhow::bail!("expected authority relay");
    };
    publish_isolated_authority(&authority_relay.url).await?;
    let publisher = NostrKeys::generate().public_key().to_string();
    let nostr = NostrEnv {
        relay_urls: &relay.url,
        holder_relay_url: &relay.url,
        setup_payment_publisher: &publisher,
    };
    let overrides = local_iroh_overrides_for_grid(PORT_BASE, 1, GUARDIAN_COUNT);
    let (daemons, encoded_locators) = start_daemons(
        &bin,
        &temp,
        bitcoin,
        1,
        PORT_BASE,
        Some(&overrides),
        GUARDIAN_COUNT,
        Some(nostr),
        None,
    )
    .await;
    let mut daemons = daemons.into_iter().map(Some).collect::<Vec<_>>();
    let mut locators = encoded_locators
        .iter()
        .map(|value| serde_json::from_str::<Locator>(value))
        .collect::<Result<Vec<_>, _>>()?;
    write_routes(&temp, &locators)?;
    offer_free_seats(&bin, &temp, GUARDIAN_COUNT).await?;
    eprintln!("native transfer [{:?}]: daemons ready", started.elapsed());

    let database = MemDatabase::new().into_database();
    let client = open_client(
        database.clone(),
        &relay.url,
        &authority_relay.url,
        publisher.parse()?,
    )
    .await?;
    let versions = fa_client::FedimintdVersionRange::one_core(
        fedi_decentralized_service_manifold_guardian::FEDIMINTD_VERSION_0_1
            .parse::<fa_client::FedimintdVersion>()?
            .core(),
    )?;
    let request = fa_client::MgSelectionRequest::new(
        fa_client::FederationSize(GUARDIAN_COUNT as u16),
        versions.clone(),
        fa_client::PlanPreference::InfiniteBestEffort,
    )?;
    let approval = client
        .preview_mg_selection(&request, discovery())
        .await?
        .approve(0)?;
    client
        .create_without_payer(
            fa_client::FormationIntent::new(
                None,
                fa_client::FederationSize(GUARDIAN_COUNT as u16),
                fa_client::PlanPreference::InfiniteBestEffort,
                versions,
            )?,
            approval,
            formation_options()?,
        )
        .await?;
    let FaStatus::Formation(formed) = client.status() else {
        anyhow::bail!("formation did not produce a snapshot");
    };
    let invite = formed.invite_code.context("formed invite")?;
    eprintln!(
        "native transfer [{:?}]: verified federation formed",
        started.elapsed()
    );
    let source = &formed.seats[0];
    anyhow::ensure!(
        source.mg_id.is_some(),
        "source must have verified admission"
    );
    let source_index = locators
        .iter()
        .position(|locator| locator.service_pubkey == source.locator.service_pubkey)
        .context("selected source has a native daemon")?;
    let source_dir = temp.join(format!("mg-{source_index}"));
    let source_seat = source.seat_id.clone().context("source seat")?;
    let native_files = native_material(&source_dir)?;
    shutdown_daemons(vec![daemons[source_index].take().unwrap()]).await?;

    // Only now advertise the spare: selection cannot accidentally consume it
    // during formation. Reuse the stopped source's socket grid, not its keys.
    // After Pull admission the spare's parked child stops before source restart.
    // This keeps native Iroh routing deterministic without rewriting consensus.
    let destination_dir = temp.join("destination");
    let port = PORT_BASE + u16::try_from(source_index)? * 100;
    let spawn = |executable: &Path, dir: &Path, port| {
        spawn_manifold_guardian(
            executable,
            dir,
            &bitcoin.rpc_url,
            &bitcoin.rpc_username,
            &bitcoin.rpc_password,
            port,
            Some(&overrides),
            Some(nostr),
            None,
        )
    };
    let mut destination = spawn(&bin, &destination_dir, port)?;
    onboard_new_manifold_guardian(&bin, &destination_dir, 1, nostr).await?;
    locators.push(serde_json::from_str(
        &read_locator(&mut destination, GUARDIAN_COUNT).await?,
    )?);
    write_routes(&temp, &locators)?;
    manifold_guardian_admin(
        &bin,
        &destination_dir,
        &["plans", "set", "--price-msats", "0"],
    )
    .await?;
    // Admin admission precedes the asynchronous relay advertisement. Wait for
    // verified discovery, not an arbitrary publication sleep or a pinned seat.
    let preview = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match client.preview_spare_seat(discovery()).await {
                Ok(preview) => return Ok::<_, anyhow::Error>(preview),
                Err(fa_client::FaError::InsufficientMgSeats { .. }) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await
    .context("spare advertisement did not become eligible")??;
    let spare = client
        .buy_spare_seat(preview.approve(1)?, formation_options()?)
        .await?;
    eprintln!(
        "native transfer [{:?}]: spare bought; source offline",
        started.elapsed()
    );
    let transfer = client
        .start_guardian_transfer(
            source.index,
            spare.seat_id.clone(),
            None,
            maintenance_options(30)?,
        )
        .await?;
    assert_eq!(transfer.phase, GuardianTransferPhase::AwaitingSource);
    wait_phase(&bin, &destination_dir, &spare.seat_id, "AwaitingTransfer").await?;

    // No FA exists while the destination restarts and autonomously completes.
    // Reopening the retained consumer database is a logical client restart, not
    // a claim of FA mnemonic recovery or consumer disk durability.
    drop(client);
    eprintln!(
        "native transfer [{:?}]: Pull persisted; FA offline",
        started.elapsed()
    );
    shutdown_daemons(vec![destination]).await?;
    destination = spawn(&bin, &destination_dir, port)?;
    *locators.last_mut().unwrap() =
        serde_json::from_str(&read_locator(&mut destination, GUARDIAN_COUNT).await?)?;
    write_routes(&temp, &locators)?;
    wait_phase(&bin, &destination_dir, &spare.seat_id, "AwaitingTransfer").await?;
    let mut restarted_source = spawn(&bin, &source_dir, port)?;
    locators[source_index] =
        serde_json::from_str(&read_locator(&mut restarted_source, source_index).await?)?;
    write_routes(&temp, &locators)?;
    daemons[source_index] = Some(restarted_source);
    wait_phase(&bin, &destination_dir, &spare.seat_id, "running").await?;
    eprintln!(
        "native transfer [{:?}]: destination completed without FA",
        started.elapsed()
    );
    assert_eq!(native_material(&destination_dir)?, native_files);

    let client = open_client(
        database,
        &relay.url,
        &authority_relay.url,
        publisher.parse()?,
    )
    .await?;
    let completed = client
        .resume_guardian_transfer(maintenance_options(180)?)
        .await?
        .context("open transfer")?;
    assert_eq!(completed.phase, GuardianTransferPhase::Completed);
    eprintln!(
        "native transfer [{:?}]: binding confirmed and source removed",
        started.elapsed()
    );
    assert!(client.guardian_transfer().await?.is_none());
    anyhow::ensure!(
        !source_dir.join("seats/0/data").exists(),
        "FI readback must precede actual source data removal"
    );
    let removed = manifold_guardian_admin(
        &bin,
        &source_dir,
        &["seats", "status", &source_seat.to_string()],
    )
    .await?;
    assert_eq!(removed["report"]["phase"], "Removed");
    drop(client);
    shutdown_daemons(vec![daemons[source_index].take().unwrap()]).await?;
    let mut retired = spawn(&bin, &source_dir, port)?;
    read_locator(&mut retired, source_index).await?;
    wait_phase(&bin, &source_dir, &source_seat, "Removed").await?;
    anyhow::ensure!(
        find_direct_child_named(retired.id().context("retired daemon")?, "fedimintd").is_err(),
        "removed source spawned its guardian after restart"
    );
    anyhow::ensure!(
        source_dir.join("wallet/client.db").is_dir(),
        "source payment wallet was deleted"
    );
    daemons[source_index] = Some(retired);

    // Restore the imported guardian from only the destination mnemonic + relay.
    wait_phase(&bin, &destination_dir, &spare.seat_id, "running").await?;
    wait_for_settled_backup(&bin, &destination_dir, 1).await?;
    let mnemonic =
        manifold_guardian_admin(&bin, &destination_dir, &["show-mnemonic"]).await?["mnemonic"]
            .as_str()
            .context("destination mnemonic")?
            .to_owned();
    shutdown_daemons(vec![destination]).await?;
    let phrase_file = temp.join("destination-mnemonic");
    write_sensitive_file(&phrase_file, &mnemonic)?;
    let restored_dir = temp.join("restored-destination");
    let mut restored = spawn(&bin, &restored_dir, port)?;
    retry_manifold_guardian_admin(
        &bin,
        &restored_dir,
        &[
            "onboard",
            "restore",
            "--mnemonic-file",
            &phrase_file.display().to_string(),
            "--acknowledge-original-host-is-gone",
        ],
    )
    .await?;
    complete_onboarding_stages(&bin, &restored_dir, 1, nostr).await?;
    read_locator(&mut restored, GUARDIAN_COUNT).await?;
    let status = wait_seat(&bin, &restored_dir, &spare.seat_id, "running", true).await?;
    anyhow::ensure!(
        status["report"]["invite_code"] == invite.0,
        "restored invite changed"
    );
    assert_eq!(native_material(&restored_dir)?, native_files);
    let mut remaining = daemons.into_iter().flatten().collect::<Vec<_>>();
    remaining.push(restored);
    shutdown_daemons(remaining).await?;
    defe.release(authority_lease.handle_id).await?;
    defe.release(relay_lease.handle_id).await?;
    defe.release(bitcoin_lease.handle_id).await?;
    eprintln!(
        "native transfer [{:?}]: restore and cleanup complete",
        started.elapsed()
    );
    Ok(())
}

fn write_routes(root: &Path, routes: &[Locator]) -> anyhow::Result<()> {
    let temporary = root.join("service-routes.next");
    std::fs::write(&temporary, serde_json::to_vec(routes)?)?;
    std::fs::rename(temporary, root.join("service-routes.json"))?;
    Ok(())
}

fn native_material(root: &Path) -> anyhow::Result<Vec<Vec<u8>>> {
    [
        "local.json",
        "consensus.json",
        "private.encrypt",
        "private.salt",
    ]
    .into_iter()
    .map(|file| std::fs::read(root.join("seats/0/data").join(file)).map_err(Into::into))
    .collect()
}

async fn wait_phase(
    bin: &Path,
    root: &Path,
    seat: &SeatId,
    phase: &str,
) -> anyhow::Result<serde_json::Value> {
    wait_seat(bin, root, seat, phase, false).await
}

async fn wait_seat(
    bin: &Path,
    root: &Path,
    seat: &SeatId,
    phase: &str,
    healthy: bool,
) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let status =
                manifold_guardian_admin(bin, root, &["seats", "status", &seat.to_string()]).await?;
            if status["report"]["phase"] == phase
                && (!healthy || status["report"]["health"] == "healthy")
            {
                return Ok::<_, anyhow::Error>(status);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .with_context(|| format!("seat did not reach {phase}"))?
}

fn discovery() -> fa_client::MgDiscoveryOptions {
    fa_client::MgDiscoveryOptions::with_timeout(Duration::from_secs(30))
}

fn formation_options() -> anyhow::Result<FormationRunOptions> {
    Ok(FormationRunOptions::new(FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(200),
        run_timeout: Duration::from_secs(180),
        request_timeout: Duration::from_secs(20),
    })?)
}

fn maintenance_options(seconds: u64) -> anyhow::Result<MaintenanceRunOptions> {
    Ok(MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(200),
        run_timeout: Duration::from_secs(seconds),
        request_timeout: Duration::from_secs(20),
    })?)
}

struct Identity;
impl FaIdentity for Identity {
    fn scoped_root(&self) -> DerivableSecret {
        DerivableSecret::new_root(&[73; 32], b"native-transfer-e2e")
    }
}

struct FeeAccount;
impl FaFeeAccountProvider for FeeAccount {
    fn formed_federation_fee_account(
        &self,
        _: &fa_client::FedimintFederationId,
    ) -> Result<fa_client::GuardianFeeAccount, FaFeeAccountError> {
        Ok(serde_json::from_str(
            r#"{"acc_type":"BtcDepositor","pub_keys":["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],"threshold":1}"#,
        ).expect("valid development account"))
    }
}

struct Connector(Endpoint);
impl ManifoldGuardianConnector for Connector {
    type Client = ManifoldGuardianServiceClient;

    async fn connect(
        &self,
        locator: &Locator,
    ) -> Result<Self::Client, ManifoldGuardianConnectorError> {
        let path = env::var_os(ROUTES_ENV).expect("isolated child routing");
        let routes: Vec<Locator> = serde_json::from_slice(
            &tokio::fs::read(path)
                .await
                .map_err(|e| ManifoldGuardianConnectorError::new(e.to_string()))?,
        )
        .map_err(|e| ManifoldGuardianConnectorError::new(e.to_string()))?;
        let address = routes
            .into_iter()
            .find(|route| route.endpoint_addr.id == locator.endpoint_addr.id)
            .map_or_else(
                || locator.endpoint_addr.clone(),
                |route| route.endpoint_addr,
            );
        self.0
            .connect(address, MANIFOLD_GUARDIAN_ALPN)
            .await
            .map(ManifoldGuardianServiceClient::new)
            .map_err(|e| ManifoldGuardianConnectorError::new(e.to_string()))
    }

    async fn get_availability(
        &self,
        client: &Self::Client,
        request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, ManifoldGuardianCallError> {
        client
            .transport()
            .get_availability(request)
            .await
            .map_err(|e| ManifoldGuardianCallError::new(e.to_string()))
    }

    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, ManifoldGuardianCallError> {
        client
            .transport()
            .get_quote(request)
            .await
            .map_err(|e| ManifoldGuardianCallError::new(e.to_string()))
    }
}

struct Consensus(fedi_decentralized_federation_preview::ConnectorRegistry);
impl FederationConsensusReader for Consensus {
    async fn read_consensus(
        &self,
        invite: &fa_client::InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        let snapshot = fedi_decentralized_federation_preview::read_consensus(&self.0, invite)
            .await
            .map_err(|e| FederationConsensusError::new(e.to_string()))?;
        Ok(FederationConsensusSnapshot {
            config: snapshot.config,
            meta_value: snapshot.meta_value,
            meta_revision: snapshot.meta_revision,
            network: snapshot.network,
        })
    }

    async fn read_lnv2_gateways(
        &self,
        invite: &fa_client::InviteCode,
    ) -> Result<Vec<fa_client::GatewayApiUrl>, FederationConsensusError> {
        fedi_decentralized_federation_preview::read_lnv2_gateways(&self.0, invite)
            .await
            .map_err(|e| FederationConsensusError::new(e.to_string()))
    }
}

type Client = FaClient<Identity, UnavailablePayments, NostrFaClient, Connector, Consensus>;

/// Sign a real authority with the committed development issuance key, but with
/// revocation hosted by the isolated fixture rather than public infrastructure.
async fn publish_isolated_authority(relay: &str) -> anyhow::Result<()> {
    let profile = ManifoldEnvironment::Development.profile()?;
    let secret: peerbadge_protocol::IssuerSecretKeys = serde_json::from_str(
        profile
            .test_issuer_secret_keys()
            .context("development profile has test issuer keys")?,
    )?;
    let issuer = peerbadge_protocol::IssuerContext::import_secret_key(&secret)?;
    let authority = issuer.issuer_authority(vec![peerbadge_protocol::RevocationLocation {
        protocol: NOSTR_REVOCATION_LOCATION_PROTOCOL.to_owned(),
        location: relay.to_owned(),
    }])?;
    NostrRelayClient::connect(
        relay,
        NostrKeys::parse(&secret.issuer_id_secret_key)?,
        Duration::from_secs(10),
    )
    .await?
    .publish_event(
        EventBuilder::new(
            Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
            serde_json::to_string(&authority)?,
        )
        .tags([
            Tag::identifier(ISSUER_AUTHORITY_D_TAG),
            Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
        ]),
    )
    .await?;
    Ok(())
}

async fn open_client(
    database: Database,
    relay: &str,
    authority_relay: &str,
    publisher: nostr_sdk::PublicKey,
) -> anyhow::Result<Client> {
    let profile = ManifoldEnvironment::Development.profile()?;
    let verifier = PeerBadgeVerifier::new_for_test(
        profile.peer_badge_issuer_identities().iter().copied(),
        [authority_relay.parse()?],
        profile.minimum_peer_badge_trust_level(),
    )?;
    Ok(FaClient::open_with_setup_payment_publisher(
        database,
        Identity,
        UnavailablePayments,
        NostrFaClient::new(
            NostrRelayClient::connect(relay, NostrKeys::generate(), Duration::from_secs(10))
                .await?,
        ),
        Connector(Endpoint::builder(presets::N0DisableRelay).bind().await?),
        verifier,
        Consensus(fedi_decentralized_federation_preview::bind_client_connectors().await?),
        FeeAccount,
        publisher,
        profile.guardian_verification_fee_account().cloned(),
    )
    .await?)
}
