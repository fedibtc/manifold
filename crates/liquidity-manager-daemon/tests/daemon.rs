use fedi_decentralized_manifold_environment::ManifoldEnvironment;

use super::*;

fn args(mode: DaemonMode, environment: ManifoldEnvironment) -> DaemonArgs {
    let data_dir = std::env::temp_dir().join("unused-flip-boot-mode-test");
    DaemonArgs {
        manifold_environment: environment,
        sqlite_path: data_dir.join("flip.sqlite"),
        data_dir,
        admin_bind_address: "127.0.0.1:0".parse().expect("test address"),
        public_bind_address: "127.0.0.1:0".parse().expect("test address"),
        bootstrap_admin_token: None,
        secret_store_key: None,
        allow_bootstrap_token_fallback: false,
        mode,
        provider_nostr_secret_key: None,
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    }
}

fn verifier(environment: ManifoldEnvironment) -> PeerBadgeVerifier {
    PeerBadgeVerifier::try_from_profile(&environment.profile().expect("test profile resolves"))
        .expect("test environment has a placeholder issuer")
}

/// The worker names and their order are what an operator reads in Admin API
/// health, so both are pinned.
///
/// `worker_health_component` formats the map in iteration order, and
/// `WorkerHealthMap` is a `BTreeMap` keyed by this enum, so the derived `Ord`
/// — declaration order — decides the order of that line. Declaring a variant
/// out of alphabetical order would reorder it without any other test noticing.
#[test]
fn worker_names_and_their_health_order_are_stable() {
    use std::collections::BTreeMap;

    let expected = [
        "advertisement_publisher",
        "gateway_allocation",
        "gateway_observation",
        "holder_authorization_initial_read",
        "stability_pool_allocation",
        "wallet_operation_sync",
    ];
    let workers = [
        Worker::AdvertisementPublisher,
        Worker::GatewayAllocation,
        Worker::GatewayObservation,
        Worker::HolderAuthorizationInitialRead,
        Worker::StabilityPoolAllocation,
        Worker::WalletOperationSync,
    ];
    for (worker, name) in workers.iter().zip(expected) {
        assert_eq!(worker.to_string(), name);
    }

    // Inserted back to front; the map must still yield them in the order above.
    let map: BTreeMap<Worker, ()> = workers.iter().rev().map(|worker| (*worker, ())).collect();
    assert_eq!(
        map.keys().map(ToString::to_string).collect::<Vec<_>>(),
        expected
    );
}

/// The actual guarded queue transition closes one captured generation after
/// its first restore and leaves that first archive pending when a second
/// concurrently staged handler reaches admission.
/// The restore flag spans the whole swap, and only the swap.
///
/// It is what `GET /health` reads to tell `reloading` from `no_runtime` while
/// no generation is installed. The pending slot cannot stand in for it: the
/// generation loop empties that slot before it commits the archive and rebuilds
/// the runtime, which is most of the wait an operator sits through.
#[tokio::test]
async fn the_restore_flag_covers_the_whole_swap() -> anyhow::Result<()> {
    let context = crate::test_support::production_test_context(
        "daemon-restore-flag-span",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;
    let shell = DaemonShell::with_generation(context.clone());
    assert!(
        !shell.is_reloading(),
        "a serving generation is not a restore"
    );

    let staged = crate::backup::StagedRestore::for_test(
        context.paths.data_dir.join("flag-staged-restore"),
        vec![fedi_decentralized_service_liquidity_manager::BackupStateGroup::Database],
    );
    let mut allocation_admission = context.allocation_admission.write().await;
    shell.request_restore(staged, &context, &mut allocation_admission)?;
    drop(allocation_admission);
    assert!(shell.is_reloading(), "the restore is armed");

    shell.uninstall();
    // The loop takes the pending slot before it commits the archive and
    // rebuilds. The operator is still waiting on the restore for all of that.
    let _staged = shell.take_pending_restore();
    assert!(
        shell.is_reloading(),
        "the swap is not over when the pending slot empties"
    );

    shell.install(context);
    assert!(
        !shell.is_reloading(),
        "a generation is serving again, so the wait is over"
    );
    Ok(())
}

#[tokio::test]
async fn one_generation_queues_exactly_one_restore() -> anyhow::Result<()> {
    let context = crate::test_support::production_test_context(
        "daemon-one-restore-per-generation",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;
    let shell = DaemonShell::with_generation(context.clone());
    let first = crate::backup::StagedRestore::for_test(
        context.paths.data_dir.join("first-staged-restore"),
        vec![fedi_decentralized_service_liquidity_manager::BackupStateGroup::Database],
    );
    let second = crate::backup::StagedRestore::for_test(
        context.paths.data_dir.join("second-staged-restore"),
        vec![fedi_decentralized_service_liquidity_manager::BackupStateGroup::OperationHistory],
    );

    let mut allocation_admission = context.allocation_admission.write().await;
    shell.request_restore(first, &context, &mut allocation_admission)?;
    assert_eq!(
        *allocation_admission,
        AllocationAdmission::ClosingForRestore
    );
    let error = shell
        .request_restore(second, &context, &mut allocation_admission)
        .expect_err("the second restore against one generation must be refused");
    assert_eq!(
        error.code(),
        fedi_decentralized_service_liquidity_manager::ServiceErrorCode::Unavailable
    );
    drop(allocation_admission);

    let pending = shell
        .take_pending_restore()
        .expect("the first restore remains pending");
    assert_eq!(
        pending.response().restored_state_groups,
        vec![fedi_decentralized_service_liquidity_manager::BackupStateGroup::Database]
    );
    Ok(())
}

#[tokio::test]
async fn normal_entry_rejects_restore_arguments_before_side_effects() {
    let error = run_daemon(
        args(DaemonMode::Restore, ManifoldEnvironment::Development),
        verifier(ManifoldEnvironment::Development),
    )
    .await
    .expect_err("normal entry rejects restore mode");
    assert!(error.to_string().contains("run_restore_daemon"));
}

#[tokio::test]
async fn restore_entry_rejects_normal_arguments_before_side_effects() {
    let error = run_restore_daemon(args(DaemonMode::Normal, ManifoldEnvironment::Development))
        .await
        .expect_err("restore entry rejects normal mode");
    assert!(error.to_string().contains("requires restore-mode"));
}

#[tokio::test]
async fn normal_entry_rejects_a_verifier_from_another_environment() {
    let error = run_daemon(
        args(DaemonMode::Normal, ManifoldEnvironment::Staging),
        verifier(ManifoldEnvironment::Development),
    )
    .await
    .expect_err("mismatched environment verifier is rejected");
    assert!(error.to_string().contains("provenance does not match"));
}

#[tokio::test]
async fn worker_health_starts_unknown_and_tracks_consecutive_failures() -> anyhow::Result<()> {
    let context = crate::test_support::production_test_context(
        "daemon-worker-health",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;
    let observed_at = crate::now_timestamp();

    // Nothing has run yet: reporting healthy here would claim a guarantee the
    // daemon cannot make.
    let initial = context.worker_health_component(observed_at).await;
    assert_eq!(initial.status, HealthStatus::Unknown);

    context
        .record_worker_success(Worker::GatewayAllocation)
        .await;
    assert_eq!(
        context.worker_health_component(observed_at).await.status,
        HealthStatus::Healthy
    );

    // A blip warns rather than pages.
    context
        .record_worker_failure(Worker::GatewayAllocation, "gatewayd refused".to_owned())
        .await;
    assert_eq!(
        context.worker_health_component(observed_at).await.status,
        HealthStatus::Warning
    );

    // A worker failing every pass must reach Unhealthy on its own, rather than
    // leaving a restart as the only way an operator discovers it.
    for _ in 1..WORKER_UNHEALTHY_AFTER_FAILURES {
        context
            .record_worker_failure(Worker::GatewayAllocation, "gatewayd refused".to_owned())
            .await;
    }
    let degraded = context.worker_health_component(observed_at).await;
    assert_eq!(degraded.status, HealthStatus::Unhealthy);
    assert!(
        degraded
            .detail
            .as_deref()
            .expect("detail names the worker")
            .contains("gateway_allocation"),
        "the detail must identify which worker is stuck: {:?}",
        degraded.detail
    );

    // Recovery clears the count but keeps the last error for diagnosis.
    context
        .record_worker_success(Worker::GatewayAllocation)
        .await;
    let recovered = context.worker_health_component(observed_at).await;
    assert_eq!(recovered.status, HealthStatus::Healthy);
    assert!(
        recovered
            .detail
            .as_deref()
            .expect("detail is present")
            .contains("gatewayd refused"),
        "a recovered-but-flapping worker stays diagnosable"
    );
    Ok(())
}
