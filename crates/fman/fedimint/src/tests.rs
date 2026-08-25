use super::*;

use fedimint_client::db::{
    ClientInitStateKey, ClientModuleRecoveryState, InitMode, InitModeComplete, InitState,
};
use fedimint_core::config::{ClientConfig, ClientModuleConfig, GlobalClientConfig};
use fedimint_core::db::mem_impl::MemDatabase;
use fedimint_core::db::{Database, IDatabaseTransactionOpsCore, IDatabaseTransactionOpsCoreTyped};
use fedimint_core::encoding::DynRawFallback;
use fedimint_core::module::registry::ModuleRegistry;
use fedimint_core::module::{AmountUnit, CoreConsensusVersion};
use locked_payments::standard_module_root_secret;
use std::collections::BTreeMap;
use std::sync::Arc;

async fn serve_chunked_body(body: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        assert_ne!(socket.read(&mut request).await.unwrap(), 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        for chunk in body.chunks(4096) {
            socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(chunk).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
    });
    format!("http://{address}/")
}

#[tokio::test]
async fn lnurl_body_cap_accepts_exact_chunked_boundary_and_rejects_next_byte() {
    let client = lnurl::Builder::default().timeout(1).build_async().unwrap();
    let exact = vec![b'x'; MAX_LNURL_RESPONSE_BYTES];
    let exact_url = serve_chunked_body(exact.clone()).await;
    assert_eq!(
        bounded_lnurl_get(&client, &exact_url, None).await.unwrap(),
        exact
    );

    let oversized_url = serve_chunked_body(vec![b'x'; MAX_LNURL_RESPONSE_BYTES + 1]).await;
    let error = bounded_lnurl_get(&client, &oversized_url, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exceeds 65536 bytes"));
}

#[tokio::test]
async fn canceled_wallet_client_open_is_fenced_until_restart() {
    for scope in [payment(1), guardian(1, 1)] {
        let attempted = Arc::new(Mutex::new(HashSet::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let task = {
            let attempted = attempted.clone();
            let started = started.clone();
            let scope = scope.clone();
            tokio::spawn(async move {
                client_open_once(&attempted, scope, async move {
                    started.notify_one();
                    std::future::pending::<anyhow::Result<()>>().await
                })
                .await
            })
        };
        started.notified().await;
        task.abort();
        let _ = task.await;

        let error = client_open_once(&attempted, scope, async { Ok(()) })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("restart before retrying"));
    }
}
#[tokio::test]
async fn wallet_client_open_fence_is_isolated_by_scope() {
    let attempted = Mutex::new(HashSet::new());
    client_open_once(&attempted, guardian(1, 1), async { Ok(()) })
        .await
        .unwrap();
    client_open_once(&attempted, guardian(1, 2), async { Ok(()) })
        .await
        .unwrap();
    client_open_once(&attempted, payment(1), async { Ok(()) })
        .await
        .unwrap();
}

#[test]
fn v1_gateway_diagnostic_separates_absence_from_expiry() {
    assert_eq!(
        v1_gateway_unavailable(std::iter::empty()).to_string(),
        "federation announced no Lightning v1 gateways"
    );
    assert_eq!(
        v1_gateway_unavailable([std::time::Duration::ZERO, std::time::Duration::ZERO,].into_iter())
            .to_string(),
        "federation announced 2 Lightning v1 gateways, but all announcements have expired"
    );
    assert_eq!(
        v1_gateway_unavailable(
            [std::time::Duration::ZERO, std::time::Duration::from_secs(1),].into_iter()
        )
        .to_string(),
        "federation announced 2 Lightning v1 gateways, but none remains usable for 30 seconds"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_scope_payout_starts_are_serialized() {
    use std::future::Future as _;
    use std::task::Poll;

    let temp = tempfile::tempdir().unwrap();
    let wallet = Arc::new(
        Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
            .await
            .unwrap(),
    );
    let first = wallet.payout_exclusion(payment(1)).await;
    let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
    let second = tokio::spawn({
        let wallet = wallet.clone();
        async move {
            let mut acquisition = Box::pin(wallet.payout_exclusion(payment(1)));
            let mut pending_tx = Some(pending_tx);
            std::future::poll_fn(move |context| match acquisition.as_mut().poll(context) {
                Poll::Pending => {
                    if let Some(pending_tx) = pending_tx.take() {
                        pending_tx.send(()).unwrap();
                    }
                    Poll::Pending
                }
                Poll::Ready(guard) => Poll::Ready(guard),
            })
            .await
        }
    });
    pending_rx.await.unwrap();
    assert!(!second.is_finished());
    drop(wallet.payout_exclusion(payment(2)).await);
    drop(first);
    tokio::time::timeout(std::time::Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap();
}

fn federation_id(byte: u8) -> FederationId {
    format!("{byte:02x}").repeat(32).parse().unwrap()
}
fn payment(byte: u8) -> ClientScope {
    ClientScope::Payment(federation_id(byte))
}
fn guardian(byte: u8, seat: u8) -> ClientScope {
    ClientScope::Guardian {
        federation_id: federation_id(byte),
        seat_id: format!("{seat:02x}").repeat(32),
    }
}

#[test]
fn raw_mint_v2_preview_config_is_decoded_before_validation() {
    let config = raw_mint_v2_config(1, 1);

    validate_payment_config(&config).unwrap();
}

#[test]
fn mismatched_raw_mint_v2_module_id_is_rejected() {
    let config = raw_mint_v2_config(1, 2);

    assert!(validate_payment_config(&config).is_err());
}

#[tokio::test]
async fn required_mint_recovery_does_not_treat_client_completion_as_mint_completion() {
    let config = raw_mint_v2_config(1, 1);
    let required_mints = required_mint_modules(&config).unwrap();
    let database = Database::new(MemDatabase::new(), ModuleRegistry::default());

    // The upstream client can declare a whole recovery complete after skipping
    // this mint. A missing per-mint record must remain a local hard failure.
    assert!(
        ensure_mint_recoveries_started(&database, &required_mints)
            .await
            .is_err()
    );

    let module_instance_id = *required_mints.first().unwrap();
    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(
        &ClientModuleRecovery { module_instance_id },
        &ClientModuleRecoveryState {
            progress: fedimint_client_module::module::recovery::RecoveryProgress::none(),
        },
    )
    .await;
    dbtx.commit_tx().await;
    ensure_mint_recoveries_started(&database, &required_mints)
        .await
        .unwrap();
    assert!(
        ensure_mint_recoveries_finished(&database, &required_mints)
            .await
            .is_err()
    );

    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(
        &ClientModuleRecovery { module_instance_id },
        &ClientModuleRecoveryState {
            progress: fedimint_client_module::module::recovery::RecoveryProgress::none()
                .to_complete(),
        },
    )
    .await;
    dbtx.commit_tx().await;
    ensure_mint_recoveries_finished(&database, &required_mints)
        .await
        .unwrap();
}

#[tokio::test]
async fn completed_recovery_stays_unready_until_fman_marks_its_outputs_settled() {
    let database = Database::new(MemDatabase::new(), ModuleRegistry::default());
    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(
        &ClientInitStateKey,
        &InitState::Pending(InitMode::Recover { snapshot: None }),
    )
    .await;
    dbtx.commit_tx().await;
    assert!(recovery_needs_ready_marker(&database).await.unwrap());

    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(
        &ClientInitStateKey,
        &InitState::Complete(InitModeComplete::Recover),
    )
    .await;
    dbtx.commit_tx().await;
    // Fedimint changes this to Complete before the FMan has observed the
    // required mint records and settled their output state machines.
    assert!(recovery_needs_ready_marker(&database).await.unwrap());
    mark_recovery_ready(&database).await.unwrap();
    assert!(!recovery_needs_ready_marker(&database).await.unwrap());

    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(
        &ClientInitStateKey,
        &InitState::Complete(InitModeComplete::Fresh),
    )
    .await;
    dbtx.commit_tx().await;
    assert!(!recovery_needs_ready_marker(&database).await.unwrap());

    let mut dbtx = database.begin_transaction().await;
    dbtx.insert_entry(&ClientInitStateKey, &InitState::Pending(InitMode::Fresh))
        .await;
    dbtx.commit_tx().await;
    assert!(recovery_needs_ready_marker(&database).await.is_err());
}

fn raw_mint_v2_config(module_id: u16, embedded_module_id: u16) -> ClientConfig {
    let mint = fedimint_mintv2_common::config::MintClientConfig {
        tbs_agg_pks: BTreeMap::new(),
        tbs_pks: BTreeMap::new(),
        fee_consensus: fedimint_mintv2_common::config::FeeConsensus::zero(),
        amount_unit: AmountUnit::BITCOIN,
    };
    ClientConfig {
        global: GlobalClientConfig {
            api_endpoints: BTreeMap::new(),
            broadcast_public_keys: None,
            consensus_version: CoreConsensusVersion::new(2, 1),
            meta: BTreeMap::new(),
        },
        modules: BTreeMap::from([(
            module_id,
            ClientModuleConfig {
                kind: fedimint_mintv2_common::KIND,
                version: fedimint_mintv2_common::MODULE_CONSENSUS_VERSION,
                config: DynRawFallback::Raw {
                    module_instance_id: embedded_module_id,
                    raw: mint.consensus_encode_to_vec(),
                },
            },
        )]),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn private_invite_is_rejected_before_fedimint_client_use() {
    let invite = InviteCode::new(
        fedimint_core::util::SafeUrl::parse("https://example.com").unwrap(),
        fedimint_core::PeerId::from(0),
        FederationId::dummy(),
        Some("must-not-be-logged".to_owned()),
    );
    let temp = tempfile::tempdir().unwrap();
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();

    assert!(matches!(
        wallet.join(&invite).await,
        Err(WalletError::PrivateInviteUnsupported)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_timeout_does_not_fence_same_process_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let invite = InviteCode::new(
        fedimint_core::util::SafeUrl::parse(&format!("http://{}", listener.local_addr().unwrap()))
            .unwrap(),
        fedimint_core::PeerId::from(0),
        FederationId::dummy(),
        None,
    );
    let temp = tempfile::tempdir().unwrap();
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    let scope = ClientScope::Payment(invite.federation_id());

    for _ in 0..2 {
        assert!(matches!(
            wallet
                .join_inner(
                    &invite,
                    scope.clone(),
                    std::time::Duration::from_millis(10),
                    None,
                )
                .await,
            Err(WalletError::JoinTimedOut { .. })
        ));
        assert!(!wallet.open_attempted.lock().await.contains(&scope));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wallet_root_excludes_concurrent_openers() {
    let temp = tempfile::tempdir().unwrap();
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert!(
        Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
            .await
            .is_err()
    );
    drop(wallet);
    Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn wallet_root_lock_open_does_not_truncate_existing_contents() {
    let temp = tempfile::tempdir().unwrap();
    let lock_path = temp.path().join(WALLET_LOCK_FILE);
    std::fs::write(&lock_path, b"operator sentinel").unwrap();
    let _wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert_eq!(std::fs::read(lock_path).unwrap(), b"operator sentinel");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn wallet_root_lock_does_not_follow_or_truncate_a_symlink() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("lock-target");
    std::fs::write(&target, b"must remain intact").unwrap();
    symlink(&target, temp.path().join(WALLET_LOCK_FILE)).unwrap();
    assert!(
        Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(target).unwrap(), b"must remain intact");
}

#[tokio::test(flavor = "multi_thread")]
async fn same_federation_join_is_excluded_while_unrelated_join_progresses() {
    let temp = tempfile::tempdir().unwrap();
    let wallet = Arc::new(
        Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
            .await
            .unwrap(),
    );
    let first = wallet.join_exclusion(payment(1)).await;
    let same = tokio::spawn({
        let wallet = wallet.clone();
        async move { wallet.join_exclusion(payment(1)).await }
    });
    tokio::task::yield_now().await;
    assert!(!same.is_finished());
    drop(wallet.join_exclusion(payment(2)).await);
    drop(first);
    tokio::time::timeout(std::time::Duration::from_secs(1), same)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn standard_double_derive_module_root_is_stable() {
    let global = DerivableSecret::new_root(&[42; 64], WALLET_SECRET_SALT);
    let derived = standard_module_root_secret(&global, federation_id(1), 4).to_random_bytes();
    assert_eq!(
        derived,
        [
            60, 212, 153, 17, 42, 106, 9, 134, 79, 190, 250, 74, 13, 242, 70, 188, 61, 132, 193,
            130, 31, 77, 39, 104, 17, 184, 92, 15, 95, 201, 132, 144,
        ]
    );
    // The guardian client's root is a separate identity label, not a variation
    // on the payment root; pinned so the two can never converge on one module
    // secret and collide their mint indices.
    let guardian = DerivableSecret::new_root(&[43; 64], WALLET_SECRET_SALT);
    assert_eq!(
        standard_module_root_secret(&guardian, federation_id(1), 4).to_random_bytes::<32>(),
        [
            81, 84, 13, 216, 49, 108, 109, 140, 151, 37, 237, 108, 130, 69, 123, 247, 148, 175,
            248, 252, 30, 196, 119, 146, 9, 204, 90, 32, 62, 33, 38, 224
        ]
    );
    assert_ne!(
        standard_module_root_secret(
            &guardian_scope_root(&guardian, "seat-a"),
            federation_id(1),
            4
        )
        .to_random_bytes::<32>(),
        standard_module_root_secret(
            &guardian_scope_root(&guardian, "seat-b"),
            federation_id(1),
            4
        )
        .to_random_bytes::<32>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_mappings_persist_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let first = payment(1);
    let second = guardian(1, 2);
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert_eq!(reserve_prefix(&wallet.database, &first).await.unwrap(), 1);
    assert_eq!(reserve_prefix(&wallet.database, &second).await.unwrap(), 2);
    drop(wallet);
    let reopened = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert_eq!(reopened.prefixes.read().await.get(&first), Some(&1));
    assert_eq!(reopened.prefixes.read().await.get(&second), Some(&2));
}

#[tokio::test(flavor = "multi_thread")]
async fn reopen_keeps_guardian_scope_dormant_until_fee_operation() {
    let temp = tempfile::tempdir().unwrap();
    let scope = guardian(1, 2);
    let wallet = Wallet::open_guarding(
        temp.path().to_owned(),
        &WalletSecret([42; 64]),
        &WalletSecret([43; 64]),
    )
    .await
    .unwrap();
    assert_eq!(reserve_prefix(&wallet.database, &scope).await.unwrap(), 1);
    drop(wallet);

    let reopened = Wallet::open_guarding(
        temp.path().to_owned(),
        &WalletSecret([42; 64]),
        &WalletSecret([43; 64]),
    )
    .await
    .unwrap();
    assert_eq!(reopened.prefixes.read().await.get(&scope), Some(&1));
    assert!(reopened.federations.read().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn reopen_keeps_removed_payment_scope_dormant() {
    let temp = tempfile::tempdir().unwrap();
    let scope = payment(1);
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert_eq!(reserve_prefix(&wallet.database, &scope).await.unwrap(), 1);
    drop(wallet);

    let reopened = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    assert_eq!(reopened.prefixes.read().await.get(&scope), Some(&1));
    assert!(reopened.federation_ids().await.is_empty());
    assert_eq!(
        reopened.retained_federation_ids().await,
        vec![scope.federation_id()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reserved_failed_join_prefix_is_never_reused() {
    let temp = tempfile::tempdir().unwrap();
    let wallet = Wallet::open(temp.path().to_owned(), &WalletSecret([42; 64]))
        .await
        .unwrap();
    // Joining can fail after reservation; the next federation must not get its state.
    assert_eq!(
        reserve_prefix(&wallet.database, &payment(1)).await.unwrap(),
        1
    );
    assert_eq!(
        reserve_prefix(&wallet.database, &payment(2)).await.unwrap(),
        2
    );
}

async fn test_database() -> (tempfile::TempDir, Database) {
    let temp = tempfile::tempdir().unwrap();
    let database = fedimint_rocksdb::RocksDb::build(temp.path().join("client.db"))
        .open()
        .await
        .unwrap()
        .into();
    (temp, database)
}

#[tokio::test(flavor = "multi_thread")]
async fn load_prefixes_rejects_reserved_zero_and_duplicates() {
    let (_temp, database) = test_database().await;
    let global = global_db(&database);
    let mut tx = global.begin_transaction().await;
    tx.raw_insert_bytes(&scope_key(0), &payment(1).consensus_encode_to_vec())
        .await
        .unwrap();
    tx.commit_tx().await;
    assert!(format!("{:#}", load_prefixes(&database).await.unwrap_err()).contains("prefix 0"));

    let (_temp, database) = test_database().await;
    let global = global_db(&database);
    let mut tx = global.begin_transaction().await;
    for prefix in [1, 2] {
        tx.raw_insert_bytes(&scope_key(prefix), &payment(1).consensus_encode_to_vec())
            .await
            .unwrap();
    }
    tx.commit_tx().await;
    assert!(format!("{:#}", load_prefixes(&database).await.unwrap_err()).contains("duplicate"));
}

#[test]
fn mismatched_prefix_federation_is_rejected() {
    let error = validate_scope_prefix(1, &payment(1), federation_id(2)).unwrap_err();
    assert!(format!("{error:#}").contains("client scope prefix 1 contains a different federation"));
}
