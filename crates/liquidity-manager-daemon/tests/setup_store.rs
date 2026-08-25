use super::*;
use crate::test_support::test_sqlite_path;
use fedi_decentralized_service_liquidity_manager::{
    AcceptedAttesterPolicy, AdvertisementConfig, CapacityConfig, CapacityMode, DurationSecs,
    FundingPolicyConfig, GatewayConfig, GatewayId, GatewayName, ProviderPolicy, Pubkey,
    ReplenishmentConfig, RpcEndpointAddress, RpcEndpointConfig, RpcProtocolName, RpcTransport,
    Sats, ServiceErrorCode, SourceType, Url, VerificationRequirement,
};

use crate::Database;

#[test]
fn setup_config_view_redacts_secrets() -> ServiceResult<()> {
    let config = test_setup_config();
    let view = setup_config_to_view(&config, false)?;
    let json = serde_json::to_string(&view).map_err(internal_error)?;

    assert!(view.gateway.has_admin_credential);
    assert!(!json.contains("gateway-secret"));
    assert!(!json.contains("bitcoind-secret"));
    Ok(())
}

#[tokio::test]
async fn validate_setup_rejects_invalid_stability_pool_fee_rate() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-invalid-stability-fee")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.funding_policy.stability_pool_min_fee_rate_ppb = 1_000_000_001;

    let validation = validate_setup(
        &database,
        &secret_store,
        false,
        ValidateSetupRequest {
            candidate_config: Some(config),
        },
    )
    .await?
    .validation;

    assert_eq!(validation.status, ValidationStatus::Failed);
    assert!(validation.checks.iter().any(|check| {
        check.name == "funding_policy" && check.status == ValidationStatus::Failed
    }));
    Ok(())
}

/// Restore-mode validation opens no socket.
///
/// A restore-mode process validates an archive it was handed. Before ruling
/// W, `validate_restored_state` ran the full setup validator, which dials the
/// gateway and chain-observer URLs *named by that archive* and sends the
/// archive's own gateway admin credential to the archive's own URL, with no
/// endpoint policy applied. None of it constructs a `DaemonContext`, so the
/// confinement record's predicate could not see it.
///
/// The second half is the control. Asserting only that the four network
/// checks are absent would pass just as well against a build that deleted
/// them outright, which would silently stop validating a live setup.
#[tokio::test]
async fn restore_validation_runs_local_checks_and_opens_no_socket() -> anyhow::Result<()> {
    const NETWORK_CHECKS: [&str; 4] = [
        "gateway_reachable",
        "chain_observer_reachable",
        "gateway_wallet_api",
        "chain_observer_api",
    ];

    let database = Database::connect(test_sqlite_path("restore-local-validation")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    // The stored config points at a port that refuses immediately, so the
    // control half below stays fast. The assertions are on which checks ran,
    // not on what they returned.
    let config = test_setup_config();
    let view = setup_config_to_view(&config, false)?;
    let mut tx = database.begin_write().await?;
    upsert_setup_state_tx(
        &mut tx,
        0,
        SetupStatus::Ready,
        &view,
        &SetupValidationSummary {
            status: ValidationStatus::NotRun,
            checks: Vec::new(),
        },
    )
    .await?;
    tx.commit().await?;

    let restored = validate_restored_setup(&database, &secret_store).await?;
    for name in NETWORK_CHECKS {
        assert!(
            !restored.checks.iter().any(|check| check.name == name),
            "restore validation must not run {name}"
        );
    }
    // The local checks still run, so the summary is not simply empty.
    assert!(
        restored
            .checks
            .iter()
            .any(|check| check.name == "funding_policy"),
        "restore validation must still run the local checks"
    );

    // The control: the normal path still runs every network check.
    let full = validate_setup(
        &database,
        &secret_store,
        false,
        ValidateSetupRequest {
            candidate_config: None,
        },
    )
    .await?
    .validation;
    for name in NETWORK_CHECKS {
        assert!(
            full.checks.iter().any(|check| check.name == name),
            "normal setup validation must still run {name}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn validate_setup_rejects_empty_accepted_attester_policy() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-empty-attester-policy")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.policy.accepted_attester_policies.clear();

    let validation = validate_setup(
        &database,
        &secret_store,
        false,
        ValidateSetupRequest {
            candidate_config: Some(config),
        },
    )
    .await?
    .validation;

    assert_eq!(validation.status, ValidationStatus::Failed);
    assert!(validation.checks.iter().any(|check| {
        check.name == "provider_policy" && check.status == ValidationStatus::Failed
    }));
    Ok(())
}

#[tokio::test]
async fn validate_setup_rejects_invalid_provider_display() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-invalid-display")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.provider_display = Some(
        fedi_decentralized_service_liquidity_manager::ProviderDisplay {
            name: Some("bad\u{0007}name".to_owned()),
            website: None,
            contact: None,
        },
    );

    let validation = validate_setup(
        &database,
        &secret_store,
        false,
        ValidateSetupRequest {
            candidate_config: Some(config),
        },
    )
    .await?
    .validation;

    assert_eq!(validation.status, ValidationStatus::Failed);
    assert!(validation.checks.iter().any(|check| {
        check.name == "provider_display" && check.status == ValidationStatus::Failed
    }));
    Ok(())
}

/// The gateway identity is set once and then fixed.
///
/// `admin_url` decides which wallet an accepted allocation pays, and it was
/// replaceable at any time — including between an item's admission and its
/// send, where the only thing a worker compared afterwards was `network`.
///
/// The rest of the gateway stays editable, and the control below is what
/// makes that assertion mean something: a test that only checked refusals
/// would pass just as well against a guard that froze the whole verb.
#[tokio::test]
async fn the_gateway_identity_is_fixed_after_first_setup() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-gateway-frozen")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let config = test_setup_config();
    let apply = |config: SetupConfig| {
        let database = database.clone();
        let secret_store = secret_store.clone();
        async move {
            apply_setup_config(
                &database,
                &secret_store,
                false,
                None,
                ApplySetupConfigRequest { config },
            )
            .await
        }
    };

    apply(config.clone()).await?;

    let mut retargeted = config.clone();
    retargeted.gateway.admin_url = "http://192.0.2.9:8175".to_owned();
    let error = apply(retargeted)
        .await
        .expect_err("retargeting the gateway must be refused");
    assert!(
        error.message().contains("admin_url is fixed"),
        "unexpected error: {error:?}"
    );

    let mut renamed_id = config.clone();
    renamed_id.gateway.gateway_id = Some(GatewayId("gateway-2".to_owned()));
    let error = apply(renamed_id)
        .await
        .expect_err("changing the gateway id must be refused");
    assert!(
        error.message().contains("gateway_id is fixed"),
        "unexpected error: {error:?}"
    );

    // Control: everything that is not identity still applies. The credential
    // authenticates to the same gateway rather than choosing another, so
    // freezing it would block rotation and protect nothing.
    let mut rotated = config.clone();
    rotated.gateway.gateway_name = GatewayName("Renamed Gateway".to_owned());
    apply(rotated)
        .await
        .expect("credential rotation and display changes must still apply");

    Ok(())
}

/// The in-transaction guard sees work that appeared after the first check.
///
/// This is the window the pool-side guard cannot cover. Between its two counts
/// and the write that persists the config, `apply_setup_config` normalises the
/// advertised endpoint and runs the reachability probes, so the interval is
/// seconds. A request admitted there passes the admission path's
/// setup-revision fence — that fence refuses an allocation whose revision
/// changed *before* it commits, and here the config change commits after — so
/// without the in-transaction guard nothing covers it.
#[tokio::test]
async fn the_policy_guard_in_the_transaction_sees_work_that_appeared_after_the_first_check()
-> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-policy-window")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let config = test_setup_config();
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;
    let stored = load_setup_state(&database)
        .await?
        .config
        .expect("the config just applied is stored");

    let mut candidate = stored.funding_policy.clone();
    candidate.fee_reserve = Sats(9_999);

    // The first check, with nothing in flight: it passes, which is what lets
    // the operator hear a refusal before the probes rather than after them.
    ensure_funding_policy_settled(&database, Some(&stored), &candidate)
        .await
        .expect("nothing is in flight at the first check");

    // The window: a request is admitted while those probes run.
    sqlx::query(
        "INSERT INTO allocations \
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
          request_json, verification_json, target_json, committed_amount_sats, \
          reserved_amount_sats) \
         VALUES ('fed-window', 'requester', 'provider', 'regtest', X'01', '{}', '{}', '{}', 1, 1)",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO allocation_items \
         (item_id, federation_id, source_type, status, committed_amount_sats, \
          reserved_amount_sats) \
         VALUES ('fed-window:gateway', 'fed-window', 'gateway', 'running', 1, 1)",
    )
    .execute(database.pool())
    .await?;

    // The second check, inside the transaction that would persist the config.
    let mut tx = database.begin_write().await?;
    let refused = ensure_funding_policy_settled_tx(&mut tx, Some(&stored), &candidate).await;
    tx.rollback().await?;

    let error = refused.expect_err("the in-transaction guard must see the admitted item");
    assert!(
        error.to_string().contains("still in flight"),
        "unexpected refusal: {error}"
    );
    Ok(())
}

/// A config write cannot revert one that landed while it was validating.
///
/// Entered through `apply_setup_config` rather than through the guard it calls,
/// and that distinction is the whole point. A test that calls the guard directly
/// does not notice when the guard is not wired in: deleting both of its call
/// sites leaves such a test green, and an end-to-end test is answered by the
/// pool-side guard before the in-transaction one runs.
///
/// The race is real rather than constructed. `apply_setup_config` reads the
/// setup row, then normalises the advertised endpoint and runs reachability
/// and API probes — network calls, so seconds — and only then opens the
/// transaction that writes. Anything that commits inside that interval used
/// to be overwritten by a caller that never saw it.
///
/// The interleaving here is causal, not timed. The competing write is
/// performed by the listener that the second `apply_setup_config` is
/// probing, so it necessarily happens after that call read the setup row
/// and before that call writes.
#[tokio::test]
async fn a_config_write_cannot_revert_one_that_landed_while_it_was_validating() -> anyhow::Result<()>
{
    let database = Database::connect(test_sqlite_path("setup-revision-fence")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    let config = test_setup_config();
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;

    // The candidate points its chain observer at a listener this test owns,
    // so the probe is a rendezvous. The gateway identity is fixed at first
    // setup, which is why the chain observer carries this and not the
    // gateway.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let probe_addr = listener.local_addr()?;
    let mut candidate = config.clone();
    candidate.chain_observer.backend = ChainObserverBackend::Bitcoind {
        url: Url(format!("http://{probe_addr}")),
        username: Some("bitcoin".to_owned()),
    };
    candidate.funding_policy.fee_reserve = Sats(9_999);

    let competing_database = database.clone();
    let competing_secret_store = secret_store.clone();
    let rendezvous = tokio::spawn(async move {
        let (first, _) = listener.accept().await?;
        // The second `apply_setup_config` is now inside validation: it has
        // read the setup row and cannot yet have written it.
        set_config_secret(
            &competing_database,
            &competing_secret_store,
            SetConfigSecretRequest {
                secret: ConfigSecret::ChainObserverPassword,
                update: SecretUpdate::Set(SecretString("landed-mid-validation".to_owned())),
            },
        )
        .await?;
        drop(first);
        // Later probes of the same URL are closed at once, so validation
        // finishes on a refusal rather than on a timeout.
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
        Ok::<(), anyhow::Error>(())
    });

    let refused = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config: candidate },
    )
    .await
    .expect_err("a write built on a superseded read must not land");
    assert!(
        refused
            .message()
            .contains("changed while this update was being prepared"),
        "unexpected refusal: {refused:?}"
    );

    rendezvous.abort();

    // The competing write survives, which is the property. Without the
    // fence the stale candidate overwrites the whole config view, and this
    // flag goes back to false with nothing recording that it moved.
    let stored = load_setup_state(&database)
        .await?
        .config
        .expect("the first config is still stored");
    assert!(
        matches!(
            stored.chain_observer.backend,
            ChainObserverBackendView::Bitcoind {
                has_password: true,
                ..
            }
        ),
        "the competing write was reverted: {:?}",
        stored.chain_observer.backend
    );
    assert_eq!(
        stored.funding_policy.fee_reserve, config.funding_policy.fee_reserve,
        "the refused candidate must not have applied its funding policy"
    );

    Ok(())
}

/// Control for the test above: the same candidate applies when nothing
/// competes with it.
///
/// Without this, a refusal caused by the probe listener rather than by the
/// fence would read as a pass.
#[tokio::test]
async fn the_revision_fence_does_not_refuse_an_uncontested_write() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-revision-fence-control")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    let config = test_setup_config();
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let probe_addr = listener.local_addr()?;
    let rendezvous = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });

    let mut candidate = config.clone();
    candidate.chain_observer.backend = ChainObserverBackend::Bitcoind {
        url: Url(format!("http://{probe_addr}")),
        username: Some("bitcoin".to_owned()),
    };
    candidate.funding_policy.fee_reserve = Sats(9_999);

    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config: candidate },
    )
    .await
    .expect("an uncontested write must apply");

    rendezvous.abort();

    let stored = load_setup_state(&database)
        .await?
        .config
        .expect("the candidate is stored");
    assert_eq!(stored.funding_policy.fee_reserve, Sats(9_999));
    Ok(())
}

/// Funding policy cannot change while work accepted under the current one is
/// still in flight.
///
/// Either the later effects use acceptance-time snapshots, or the update is
/// refused until the work ends. FLIP takes the second route.
#[tokio::test]
async fn funding_policy_cannot_change_under_work_in_flight() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-policy-in-flight")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let config = test_setup_config();
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;

    // Control: with nothing in flight the change applies.
    let mut changed = config.clone();
    changed.funding_policy.fee_reserve = Sats(9_999);
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: changed.clone(),
        },
    )
    .await
    .expect("a policy change with no work in flight must apply");

    sqlx::query(
        "INSERT INTO allocations \
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
          request_json, verification_json, target_json, committed_amount_sats, \
          reserved_amount_sats) \
         VALUES ('fed-1', 'requester', 'provider', 'regtest', X'00', '{}', '{}', '{}', 1, 1)",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO allocation_items \
         (item_id, federation_id, source_type, status, committed_amount_sats, \
          reserved_amount_sats) \
         VALUES ('fed-1:gateway', 'fed-1', 'gateway', 'running', 1, 1)",
    )
    .execute(database.pool())
    .await?;

    let mut changed_again = changed.clone();
    changed_again.funding_policy.fee_reserve = Sats(1);
    let error = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: changed_again,
        },
    )
    .await
    .expect_err("a policy change under an active item must be refused");
    assert!(
        error.message().contains("funding policy cannot change"),
        "unexpected error: {error:?}"
    );

    // Everything else still applies while work is in flight: only the policy
    // an accepted item depends on is frozen.
    let mut renamed = changed.clone();
    renamed.gateway.gateway_name = GatewayName("Still Editable".to_owned());
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config: renamed },
    )
    .await
    .expect("non-policy changes must still apply while work is in flight");

    Ok(())
}

#[tokio::test]
async fn apply_setup_config_encrypts_persisted_secrets() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-encrypts-secrets")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::ChainObserverPassword,
            update: SecretUpdate::Set(SecretString("bitcoind-secret".to_owned())),
        },
    )
    .await?;
    let config = test_setup_config();

    let response = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;

    assert_eq!(response.status, SetupStatus::PendingValidation);

    let config_json: String =
        sqlx::query_scalar("SELECT config_view_json FROM setup_state WHERE id = 1")
            .fetch_one(database.pool())
            .await?;
    assert!(!config_json.contains("gateway-secret"));
    assert!(!config_json.contains("bitcoind-secret"));

    let secret_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secret_records")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(secret_count, 2);

    let gateway_record = load_secret_record(&database, GATEWAY_ADMIN_SECRET)
        .await?
        .expect("gateway secret is stored");
    assert_eq!(
        secret_store.decrypt(GATEWAY_ADMIN_SECRET, &gateway_record)?,
        "gateway-secret"
    );
    assert_ne!(gateway_record.ciphertext, b"gateway-secret");

    let validation = validate_setup(
        &database,
        &secret_store,
        false,
        ValidateSetupRequest {
            candidate_config: None,
        },
    )
    .await?
    .validation;
    assert!(validation.checks.iter().any(
        |check| check.name == GATEWAY_ADMIN_SECRET && check.status == ValidationStatus::Passed
    ));

    Ok(())
}

#[tokio::test]
async fn mainnet_config_accepted_without_fixtures() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-prod-mainnet")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.network = BitcoinNetwork::Bitcoin;
    config.funding_policy = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Bitcoin);

    // Gateway/observer endpoints are unreachable here; the assertion is
    // only that the mainnet gate does not fire without trust fixtures.
    let response = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;
    assert_eq!(response.status, SetupStatus::PendingValidation);
    Ok(())
}

#[tokio::test]
async fn trust_fixtures_reject_mainnet_config() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-fixtures-mainnet")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.network = BitcoinNetwork::Bitcoin;
    config.funding_policy = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Bitcoin);

    let error = apply_setup_config(
        &database,
        &secret_store,
        true,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await
    .expect_err("trust fixtures must reject a mainnet setup config");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);

    let stored = load_setup_state(&database).await?;
    assert!(
        stored.config.is_none(),
        "a rejected mainnet config must not be persisted"
    );

    let error = validate_setup(
        &database,
        &secret_store,
        true,
        ValidateSetupRequest {
            candidate_config: Some(config),
        },
    )
    .await
    .expect_err("trust fixtures must reject a mainnet candidate config");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    Ok(())
}

/// A config write cannot touch a stored secret.
///
/// This is the defect the secret verb exists to remove. Carrying the bitcoind
/// password inside the whole-config write puts it where the read shape returns
/// `has_password: true` and never the value, so the dashboard sends it back
/// blank and the daemon reads blank as **delete**. Changing a gateway display
/// name then costs an operator their chain connection, with nothing on any
/// screen saying so.
#[tokio::test]
async fn a_config_write_leaves_stored_secrets_alone() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-secrets-survive")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::ChainObserverPassword,
            update: SecretUpdate::Set(SecretString("bitcoind-secret".to_owned())),
        },
    )
    .await?;

    // A change to a hard field with nothing to say about either secret —
    // exactly the edit that would otherwise destroy the password.
    let mut config = test_setup_config();
    config.gateway.gateway_name = GatewayName("Renamed Gateway".to_owned());
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;

    assert_eq!(
        load_bitcoind_password(&database, &secret_store).await?,
        Some("bitcoind-secret".to_owned()),
        "a config write must not be able to remove a secret"
    );
    assert_eq!(
        load_gateway_admin_credential(&database, &secret_store).await?,
        "gateway-secret"
    );
    assert!(
        matches!(
            load_setup_state(&database).await?.config,
            Some(stored)
                if matches!(
                    stored.chain_observer.backend,
                    ChainObserverBackendView::Bitcoind { has_password: true, .. }
                )
        ),
        "the stored view must still report the password"
    );
    Ok(())
}

/// Removing a secret is an operation the operator asks for by name.
#[tokio::test]
async fn clearing_a_secret_is_explicit_and_moves_the_stored_view() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-secret-clear")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::ChainObserverPassword,
            update: SecretUpdate::Set(SecretString("bitcoind-secret".to_owned())),
        },
    )
    .await?;
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: test_setup_config(),
        },
    )
    .await?;

    let cleared = set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::ChainObserverPassword,
            update: SecretUpdate::Clear,
        },
    )
    .await?;
    assert!(!cleared.present);
    assert_eq!(
        load_bitcoind_password(&database, &secret_store).await?,
        None
    );
    assert!(
        matches!(
            load_setup_state(&database).await?.config,
            Some(stored)
                if matches!(
                    stored.chain_observer.backend,
                    ChainObserverBackendView::Bitcoind { has_password: false, .. }
                )
        ),
        "the stored view follows the secret store without waiting for a config write"
    );

    // The gateway credential authenticates every gateway call, so removing
    // it would stop payouts without any configuration saying so. Replacing
    // it stays available.
    let error = set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::GatewayAdminCredential,
            update: SecretUpdate::Clear,
        },
    )
    .await
    .expect_err("the gateway credential cannot be cleared");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    Ok(())
}

/// An empty value is refused rather than treated as a removal.
///
/// The blank field an operator leaves alone is the input this whole change
/// exists to stop misreading. `Clear` is the only way to remove a secret.
#[tokio::test]
async fn an_empty_secret_is_refused_rather_than_read_as_a_removal() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-secret-empty")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    let error = set_config_secret(
        &database,
        &secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::ChainObserverPassword,
            update: SecretUpdate::Set(SecretString(String::new())),
        },
    )
    .await
    .expect_err("an empty secret is not a removal");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    assert_eq!(
        load_bitcoind_password(&database, &secret_store).await?,
        None
    );
    Ok(())
}

/// Applying a config before any credential is stored says which step is
/// missing, instead of storing a deployment that cannot reach its gateway.
#[tokio::test]
async fn applying_setup_config_without_a_stored_credential_says_so() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-no-credential")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    let error = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: test_setup_config(),
        },
    )
    .await
    .expect_err("a config without a stored credential must not apply");
    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);
    assert!(
        error.to_string().contains("set_config_secret"),
        "the error should name the step that is missing, got: {error}"
    );
    assert!(load_setup_state(&database).await?.config.is_none());
    Ok(())
}

/// The probe reaches a real gateway with the stored credential, so its
/// failures have to name which step went wrong.
///
/// `gateway_id` is frozen at first setup and decides which gateway an
/// accepted allocation pays, so it is read from the gateway rather than
/// typed. That makes reaching the gateway a precondition of setup, and an
/// operator who cannot needs to be told which of the two reasons applies.
#[tokio::test]
async fn probing_a_gateway_reports_the_missing_step() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-probe-gateway")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    // No credential stored yet: the probe authenticates with the stored one
    // rather than carrying a secret in the request.
    let no_credential = probe_gateway(
        &database,
        &secret_store,
        ProbeGatewayRequest {
            admin_url: "http://127.0.0.1:1".to_owned(),
        },
    )
    .await
    .expect_err("a probe without a stored credential cannot authenticate");
    assert_eq!(no_credential.code(), ServiceErrorCode::FailedPrecondition);

    store_test_gateway_credential(&database, &secret_store).await?;

    let empty_url = probe_gateway(
        &database,
        &secret_store,
        ProbeGatewayRequest {
            admin_url: "   ".to_owned(),
        },
    )
    .await
    .expect_err("there is nothing to probe without an address");
    assert_eq!(empty_url.code(), ServiceErrorCode::InvalidArgument);

    // Nothing is listening, which is the common first-setup mistake.
    let unreachable = probe_gateway(
        &database,
        &secret_store,
        ProbeGatewayRequest {
            admin_url: "http://127.0.0.1:1".to_owned(),
        },
    )
    .await
    .expect_err("an unreachable gateway cannot report an identity");
    assert_eq!(unreachable.code(), ServiceErrorCode::FailedPrecondition);
    assert!(
        unreachable.to_string().contains("gateway did not answer"),
        "the error should name the gateway, got: {unreachable}"
    );
    Ok(())
}

/// Stores the gateway credential these tests' `apply_setup_config` calls
/// now require.
///
/// The credential is no longer part of a config write, so a test that
/// applies one has to put it where the daemon reads it from — the same two
/// steps the wizard takes.
async fn store_test_gateway_credential(
    database: &Database,
    secret_store: &SecretStore,
) -> anyhow::Result<()> {
    set_config_secret(
        database,
        secret_store,
        SetConfigSecretRequest {
            secret: ConfigSecret::GatewayAdminCredential,
            update: SecretUpdate::Set(SecretString("gateway-secret".to_owned())),
        },
    )
    .await?;
    Ok(())
}

fn test_setup_config() -> SetupConfig {
    SetupConfig {
        network: BitcoinNetwork::Regtest,
        gateway: GatewayConfig {
            gateway_id: Some(GatewayId("gateway-1".to_owned())),
            gateway_name: GatewayName("primary".to_owned()),
            admin_url: "http://127.0.0.1:1".to_owned(),
            identity_metadata: Vec::new(),
        },
        chain_observer: ChainObserverConfig {
            backend: ChainObserverBackend::Bitcoind {
                url: Url("http://127.0.0.1:2".to_owned()),
                username: Some("bitcoin".to_owned()),
            },
        },
        relays: Vec::new(),
        capacity: CapacityConfig {
            mode: CapacityMode::ExplicitCap,
            explicit_cap: Some(Sats(10_000)),
            supported_sources: vec![SourceType::Gateway],
        },
        funding_policy: FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest),
        replenishment: ReplenishmentConfig {
            warning_threshold: Sats(1_000),
            critical_threshold: Sats(500),
        },
        advertised_endpoint: RpcEndpointConfig {
            endpoint_id: None,
            transport: RpcTransport::Iroh,
            address: RpcEndpointAddress("iroh-node-id".to_owned()),
            discovery_hints: Vec::new(),
            rpc_protocol_name: RpcProtocolName("fedi/flip/public-liquidity/1".to_owned()),
        },
        advertisement: AdvertisementConfig {
            republish_interval: DurationSecs(600),
            ready_advertisement_enabled: false,
        },
        provider_display: None,
        policy: ProviderPolicy {
            accepted_attester_policies: vec![AcceptedAttesterPolicy {
                attester_pubkey: Pubkey("attester-1".to_owned()),
                verification_requirement: VerificationRequirement::AllTrusted,
            }],
            supported_networks: vec![BitcoinNetwork::Regtest],
        },
    }
}

/// Both config write paths refuse a republish interval of zero.
///
/// Settings is the path that could set it: it mounts the wizard's interval
/// field, and reaches the daemon through `update_provider_config` rather
/// than `apply_setup_config`, so the two validators are checked separately
/// here. A stored zero would publish an advertisement that expired when it
/// was issued, which no client keeps and no dashboard reports.
#[tokio::test]
async fn a_zero_republish_interval_fails_validation_on_both_write_paths() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-zero-republish")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    let mut config = test_setup_config();
    config.advertisement.republish_interval = DurationSecs(0);
    let applied = apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;
    assert_eq!(applied.status, SetupStatus::PendingValidation);
    assert!(failed_check_details(&applied.validation).any(|detail| {
        detail.contains("advertisement.republish_interval must be greater than zero")
    }));

    // The other validator. Seed a good config first, so the only thing the
    // patch changes is the interval.
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: test_setup_config(),
        },
    )
    .await?;
    let updated = update_provider_config(
        &database,
        &secret_store,
        None,
        UpdateProviderConfigRequest {
            patch: ProviderConfigPatch {
                advertisement: Some(AdvertisementConfig {
                    republish_interval: DurationSecs(0),
                    ready_advertisement_enabled: true,
                }),
                ..ProviderConfigPatch::default()
            },
        },
    )
    .await?;
    assert!(failed_check_details(&updated.validation).any(|detail| {
        detail.contains("advertisement.republish_interval must be greater than zero")
    }));

    Ok(())
}

/// A config whose policy does not serve its own network fails validation.
///
/// The two are edited on different dashboard screens and nothing kept them
/// in agreement, so this is the drift the daemon has to catch: without it a
/// deployment validates clean, publishes, reports ready, and then refuses
/// every request `public_service` receives.
#[tokio::test]
async fn a_policy_that_excludes_the_configured_network_fails_validation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-network-drift")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;

    let mut config = test_setup_config();
    config.network = BitcoinNetwork::Signet;
    config.policy.supported_networks = vec![BitcoinNetwork::Regtest];
    let applied = apply_setup_config(
        &database,
        &secret_store,
        true,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;
    assert_eq!(applied.status, SetupStatus::PendingValidation);
    assert!(
        failed_check_details(&applied.validation)
            .any(|detail| detail.contains("policy.supported_networks must contain"))
    );

    // And the patch path, which is how the dashboard edits the policy.
    apply_setup_config(
        &database,
        &secret_store,
        true,
        None,
        ApplySetupConfigRequest {
            config: test_setup_config(),
        },
    )
    .await?;
    let updated = update_provider_config(
        &database,
        &secret_store,
        None,
        UpdateProviderConfigRequest {
            patch: ProviderConfigPatch {
                policy: Some(ProviderPolicy {
                    accepted_attester_policies: test_setup_config()
                        .policy
                        .accepted_attester_policies,
                    supported_networks: vec![BitcoinNetwork::Signet],
                }),
                ..ProviderConfigPatch::default()
            },
        },
    )
    .await?;
    assert!(
        failed_check_details(&updated.validation)
            .any(|detail| detail.contains("policy.supported_networks must contain"))
    );

    Ok(())
}

fn failed_check_details(validation: &SetupValidationSummary) -> impl Iterator<Item = &str> + '_ {
    validation
        .checks
        .iter()
        .filter(|check| check.status == ValidationStatus::Failed)
        .filter_map(|check| check.detail.as_deref())
}

#[tokio::test]
async fn local_iroh_node_id_is_adopted_as_the_advertised_address() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-adopt-iroh")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let config = test_setup_config();
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;

    // The operator cannot know the node id in advance, so applying config
    // before the transport binds stores no address at all rather than the
    // value they guessed; the daemon settles it after binding.
    assert_eq!(
        load_setup_state(&database)
            .await?
            .config
            .expect("config persists")
            .advertised_endpoint
            .address
            .0,
        ""
    );

    assert!(adopt_local_iroh_endpoint_address(&database, "node-id-after-bind").await?);
    let stored = load_setup_state(&database).await?;
    assert_eq!(
        stored
            .config
            .expect("config persists after adoption")
            .advertised_endpoint
            .address
            .0,
        "node-id-after-bind"
    );

    // Idempotent: a restart with the same derived key must not rewrite the
    // row or emit another audit entry.
    assert!(!adopt_local_iroh_endpoint_address(&database, "node-id-after-bind").await?);
    Ok(())
}

/// The address of an Iroh endpoint is the daemon's transport identity, not
/// operator input. Before this was normalized on the way in, an operator
/// could set it, get a success back, and have the value silently replaced
/// at the next bind.
#[tokio::test]
async fn an_operator_supplied_iroh_address_is_replaced_by_the_local_node_id() -> anyhow::Result<()>
{
    let database = Database::connect(test_sqlite_path("setup-normalize-iroh")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.advertised_endpoint.address = RpcEndpointAddress("operator-invented".to_owned());

    apply_setup_config(
        &database,
        &secret_store,
        false,
        Some("bound-node-id"),
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;

    assert_eq!(stored_endpoint_address(&database).await?, "bound-node-id");

    // The patch path is the one the settings page uses, and it sends the
    // endpoint whole whenever any part of it changes.
    let mut endpoint = setup_config_to_view(&config, false)?.advertised_endpoint;
    endpoint.address = RpcEndpointAddress("operator-invented-again".to_owned());
    update_provider_config(
        &database,
        &secret_store,
        Some("bound-node-id"),
        UpdateProviderConfigRequest {
            patch: ProviderConfigPatch {
                advertised_endpoint: Some(endpoint),
                ..ProviderConfigPatch::default()
            },
        },
    )
    .await?;
    assert_eq!(stored_endpoint_address(&database).await?, "bound-node-id");

    // Nothing was left for the bind-time adoption to correct.
    assert!(!adopt_local_iroh_endpoint_address(&database, "bound-node-id").await?);
    Ok(())
}

async fn stored_endpoint_address(database: &Database) -> anyhow::Result<String> {
    Ok(load_setup_state(database)
        .await?
        .config
        .expect("config persists")
        .advertised_endpoint
        .address
        .0)
}

/// Restore's openability gate: records written under another key must stop
/// the archive before it lands, not surface as an advisory check on a
/// daemon that is already locked out.
#[tokio::test]
async fn secret_records_written_under_another_key_are_rejected() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-secret-readability")).await?;
    let writing_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &writing_store).await?;
    let config = test_setup_config();
    apply_setup_config(
        &database,
        &writing_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;

    ensure_secret_records_decryptable(&database, &writing_store)
        .await
        .expect("the key that wrote the records must read them");

    let other_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    let error = ensure_secret_records_decryptable(&database, &other_store)
        .await
        .expect_err("a foreign key must be rejected");
    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);
    assert!(
        error.to_string().contains(GATEWAY_ADMIN_SECRET),
        "the error should name the unreadable record, got: {error}"
    );
    Ok(())
}

/// A config write can land in the window between a restart and the
/// transport rebinding — the live harness hits it, because the endpoint
/// address file survives the restart unchanged and so the client reconfigures
/// immediately. Clearing the address there would strand the daemon
/// advertising nothing: `adopt_local_iroh_endpoint_address` runs once per
/// generation right after the bind, so it has already passed and cannot
/// repair a later write.
#[tokio::test]
async fn a_config_write_before_the_transport_binds_keeps_the_stored_address() -> anyhow::Result<()>
{
    let database = Database::connect(test_sqlite_path("setup-endpoint-rebind-race")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let config = test_setup_config();

    // A daemon that has bound before: the stored address is its node id.
    apply_setup_config(
        &database,
        &secret_store,
        false,
        Some("bound-node-id"),
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;
    assert_eq!(stored_endpoint_address(&database).await?, "bound-node-id");

    // Restarted, not yet rebound, and reconfigured in that window.
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest {
            config: config.clone(),
        },
    )
    .await?;
    assert_eq!(
        stored_endpoint_address(&database).await?,
        "bound-node-id",
        "an unbound transport must not clear the address it bound to last time"
    );

    // Same through the patch path.
    let mut endpoint = setup_config_to_view(&config, false)?.advertised_endpoint;
    endpoint.address = RpcEndpointAddress("operator-invented".to_owned());
    update_provider_config(
        &database,
        &secret_store,
        None,
        UpdateProviderConfigRequest {
            patch: ProviderConfigPatch {
                advertised_endpoint: Some(endpoint),
                ..ProviderConfigPatch::default()
            },
        },
    )
    .await?;
    assert_eq!(stored_endpoint_address(&database).await?, "bound-node-id");
    Ok(())
}

/// A data dir with nothing stored yet is readable by definition.
#[tokio::test]
async fn decryptability_holds_when_no_secrets_are_stored() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-secret-readability-empty")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    ensure_secret_records_decryptable(&database, &secret_store).await?;
    Ok(())
}

#[tokio::test]
async fn adopting_an_iroh_node_id_leaves_a_non_iroh_endpoint_alone() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-adopt-non-iroh")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    store_test_gateway_credential(&database, &secret_store).await?;
    let mut config = test_setup_config();
    config.advertised_endpoint.transport = RpcTransport::HttpJson;
    config.advertised_endpoint.address = RpcEndpointAddress("https://provider.example".to_owned());
    apply_setup_config(
        &database,
        &secret_store,
        false,
        None,
        ApplySetupConfigRequest { config },
    )
    .await?;

    assert!(!adopt_local_iroh_endpoint_address(&database, "node-id-after-bind").await?);
    assert_eq!(
        load_setup_state(&database)
            .await?
            .config
            .expect("config persists")
            .advertised_endpoint
            .address
            .0,
        "https://provider.example"
    );
    Ok(())
}

#[tokio::test]
async fn adopting_an_iroh_node_id_without_setup_config_is_a_no_op() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("setup-adopt-unconfigured")).await?;
    assert!(!adopt_local_iroh_endpoint_address(&database, "node-id-after-bind").await?);
    Ok(())
}
