use super::*;
use crate::transfer_state::OutgoingTransfer;
use fedi_decentralized_service_manifold_guardian::{
    Locator, SeatTransferGrant, SignedRequest, Timestamp,
};

struct Source {
    _pause: Option<crate::transfer_worker::TransferPause>,
    _root: TempDir,
    fleet: Arc<Fleet>,
    sink: Arc<FakeBackupSink>,
    grant: SeatTransferGrant,
    fa: secp256k1::Keypair,
    destination: secp256k1::Keypair,
    seat: Arc<Seat>,
}

impl Source {
    fn signed(&self) -> SignedRequest<SeatTransferGrant> {
        SignedRequest::create(&self.grant, &self.fa).unwrap()
    }
}

#[tokio::test]
async fn worker_resumes_removal_when_manual_rpc_is_cancelled_during_relay_outage() {
    use crate::transfer_state::TransferPhase;
    let mut s = source(32_201, Arc::new(FakeSeatProcessSpawner::default())).await;
    s.seat
        .retire_for_transfer(s.signed(), s.fleet.identity.derive_service_signing_key())
        .await
        .unwrap();
    s.sink.fails.store(true, Ordering::SeqCst);
    let mut attempts = s.sink.attempted.subscribe();
    // Only the RPC can advance Removing while the fixture holds this pause.
    let fleet = s.fleet.clone();
    let seat = s.seat.clone();
    let removal = tokio::spawn(async move {
        fleet
            .remove_transferred_seat(&seat, Duration::from_secs(60))
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        attempts.wait_for(|document| {
            document
                .as_ref()
                .and_then(|document| document.transfer.as_ref())
                .and_then(|transfer| transfer.outgoing.as_ref())
                .is_some_and(|outgoing| outgoing.removed)
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        !removal.is_finished(),
        "manual RPC is waiting for exact relay confirmation"
    );
    removal.abort();
    assert!(removal.await.unwrap_err().is_cancelled());
    assert!(seat_data_dir(&s.fleet.config.process, s.seat.facts().seat_no).exists());
    assert_eq!(
        s.fleet
            .db
            .outgoing_transfer(&s.seat.facts().seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Removing)
    );
    s.sink.fails.store(false, Ordering::SeqCst);
    drop(s._pause.take());
    tokio::time::timeout(
        Duration::from_secs(5),
        s.seat
            .wait_for_transfer_phase_for_test(TransferPhase::Outgoing(OutgoingTransfer::Removed)),
    )
    .await
    .unwrap();
    assert!(!seat_data_dir(&s.fleet.config.process, s.seat.facts().seat_no).exists());
    s.fleet.shutdown().await;
}

#[tokio::test]
async fn worker_resumes_source_stop_and_removal_after_restart() {
    use crate::transfer_state::TransferPhase;
    let s = source(32_161, Arc::new(FakeSeatProcessSpawner::default())).await;
    s.fleet.shutdown().await;
    assert!(
        s.fleet
            .db
            .retire_for_transfer(&s.grant.source_seat_id)
            .await
            .unwrap()
    );
    let reopened = Fleet::open_with_wallet(
        s.fleet.db.clone(),
        s.fleet.config.clone(),
        async |_| Ok(Arc::new(NoWallet) as _),
        async |_| Ok(s.sink.clone() as _),
    )
    .await
    .unwrap();
    let seat = reopened.seat_by_id(&s.grant.source_seat_id).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        seat.wait_for_transfer_phase_for_test(TransferPhase::Outgoing(OutgoingTransfer::Stopped)),
    )
    .await
    .unwrap();
    assert_eq!(reopened.db.active_seat_count().await.unwrap(), 0);
    reopened.shutdown().await;
    assert!(
        reopened
            .db
            .advance_retirement(
                &s.grant.source_seat_id,
                OutgoingTransfer::Stopped,
                OutgoingTransfer::Removing
            )
            .await
            .unwrap()
    );
    let again = Fleet::open_with_wallet(
        s.fleet.db.clone(),
        s.fleet.config.clone(),
        async |_| Ok(Arc::new(NoWallet) as _),
        async |_| Ok(s.sink.clone() as _),
    )
    .await
    .unwrap();
    let seat = again.seat_by_id(&s.grant.source_seat_id).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        seat.wait_for_transfer_phase_for_test(TransferPhase::Outgoing(OutgoingTransfer::Removed)),
    )
    .await
    .unwrap();
    assert!(!seat_data_dir(&s.fleet.config.process, seat.facts().seat_no).exists());
    assert!(
        again
            .db
            .formed_federation_invite(&s.grant.source_seat_id)
            .await
            .unwrap()
            .is_some()
    );
    again.shutdown().await;
}

async fn source(port: u16, spawner: Arc<FakeSeatProcessSpawner>) -> Source {
    let root = TempDir::new().unwrap();
    let mut config = config(&root, 1, port).await;
    config.process_spawner = SeatProcessSpawner::Fake(spawner);
    let db = Db::open(root.path()).await.unwrap();
    let identity = db.load_identity().await.unwrap().unwrap();
    let fa = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[61; 32]).unwrap();
    let destination =
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[62; 32]).unwrap();
    let seat_id = SeatId::from(QuoteId([150; 32]));
    let facts = crate::test_support::insert_test_seat(
        &db,
        NewSeat {
            seat_id: seat_id.clone(),
            fa_id: FaId(fa.x_only_public_key().0),
            plan: Plan::InfiniteBestEffort { price_msats: 0 },
            federation_size: FederationSize(1),
            payment: None,
        },
    )
    .await;
    let keys = identity.derive_seat_keys(&seat_id);
    let mut native = crate::transfer_native::tests::native_config();
    native.private.iroh_api_sk =
        Some(serde_json::from_value(serde_json::to_value(&keys.iroh_api).unwrap()).unwrap());
    native.private.iroh_p2p_sk =
        Some(serde_json::from_value(serde_json::to_value(&keys.iroh_p2p).unwrap()).unwrap());
    let endpoints = native
        .consensus
        .iroh_endpoints
        .get_mut(&native.local.identity)
        .unwrap();
    endpoints.api_pk = keys.iroh_api.public().to_string().parse().unwrap();
    endpoints.p2p_pk = keys.iroh_p2p.public().to_string().parse().unwrap();
    let package = crate::transfer_native::tests::material_with_auth(&native, keys.api_auth);
    crate::backup::write_restored_seat_dir(
        &seat_data_dir(&config.process, facts.seat_no),
        &package.archive,
        &package.archive.digest(),
        &package.api_auth,
    )
    .await
    .unwrap();
    let invite = fedimint_core::invite_code::InviteCode::new(
        native.consensus.api_endpoints()[&native.local.identity]
            .url
            .clone(),
        native.local.identity,
        native.calculate_federation_id(),
        None,
    );
    db.record_formed(&seat_id, &InviteCode(invite.to_string()))
        .await
        .unwrap();
    let now = (crate::db::now_ms() / 1000) as u64;
    let grant = SeatTransferGrant {
        ts: Timestamp(now),
        expires_at: Timestamp(now + 3600),
        fa_id: facts.fa_id,
        federation_id: FederationId(native.calculate_federation_id().to_string()),
        source: Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[63; 32]).public(),
            ),
            identity.derive_service_pubkey(),
        ),
        source_seat_id: seat_id.clone(),
        destination_service_pubkey: destination.x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([151; 32])),
    };
    let sink = FakeBackupSink::new();
    let fleet = Arc::new(
        Fleet::open_with_wallet(
            db,
            config,
            async |_| Ok(Arc::new(NoWallet) as _),
            async |_| Ok(sink.clone() as _),
        )
        .await
        .unwrap(),
    );
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let pause = fleet.transfers.quiesce(&seat_id).await;
    Source {
        _pause: Some(pause),
        _root: root,
        fleet,
        sink,
        grant,
        fa,
        destination,
        seat,
    }
}

#[tokio::test]
async fn source_stops_before_export_and_requires_exact_tombstone_not_old_archive() {
    let spawner = Arc::new(FakeSeatProcessSpawner::default());
    let source = source(31_237, spawner.clone()).await;
    // Confirm the ordinary archive first. That confirmation cannot stand in
    // for a retired-source document when the relay subsequently goes offline.
    let original = crate::backup::seat_publication_plan(
        &source.fleet.db,
        &source.fleet.config.process,
        source.seat.facts(),
        None,
    )
    .await
    .unwrap();
    source
        .fleet
        .backup
        .confirm_transfer_publication(&source.fleet.db, &original, Duration::from_secs(5))
        .await
        .unwrap();
    source.sink.fails.store(true, Ordering::SeqCst);
    assert!(
        source
            .fleet
            .transfer_seat(source.signed(), Duration::from_millis(40))
            .await
            .is_err()
    );
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Stopped)
    );
    assert_eq!(source.fleet.db.active_seat_count().await.unwrap(), 0);
    assert!(matches!(
        source.seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Transfer {
                status: ServiceStatus::Transferred
            },
            ..
        }
    ));
    let spawns = spawner.spawn_count();
    source.seat.watchdog_tick_for_test().await;
    assert_eq!(spawner.spawn_count(), spawns);
    let proof_statement = peer_binding_signed_by(
        &fixture_client_config(),
        0,
        &peer_mg_keys(0),
        source
            .fleet
            .identity
            .derive_guardian_fee_account_key(&source.grant.source_seat_id)
            .account(),
    )
    .attestation;
    assert!(
        source
            .seat
            .sign_endpoint_proof(proof_statement)
            .await
            .is_err()
    );
    source.sink.fails.store(false, Ordering::SeqCst);
    let response = source
        .fleet
        .transfer_seat(source.signed(), Duration::from_secs(5))
        .await
        .unwrap();
    let opened =
        crate::transfer_package::open(&response.sealed_package, &source.grant, &source.destination)
            .unwrap();
    assert_eq!(
        opened.api_auth,
        source
            .fleet
            .identity
            .derive_seat_keys(&source.grant.source_seat_id)
            .api_auth
    );
    assert_eq!(
        Some(opened.archive.digest()),
        original.archive_digest().map(str::to_owned)
    );
    assert!(source.sink.published().iter().any(|document| {
        document
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.outgoing.is_some())
    }));

    // Deliberately no destination pin. Another valid FA-signed destination
    // receives a package sealed only to that destination.
    let other = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[64; 32]).unwrap();
    let mut grant = source.grant.clone();
    grant.destination_service_pubkey = other.x_only_public_key().0;
    grant.destination_seat_id = SeatId::from(QuoteId([152; 32]));
    let response = source
        .fleet
        .transfer_seat(
            SignedRequest::create(&grant, &source.fa).unwrap(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(
        crate::transfer_package::open(&response.sealed_package, &grant, &source.destination)
            .is_err()
    );
    assert!(crate::transfer_package::open(&response.sealed_package, &grant, &other).is_ok());
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn removal_fences_exports_before_relay_and_only_deletes_guardian_paths() {
    let source = source(31_249, Arc::new(FakeSeatProcessSpawner::default())).await;
    source
        .fleet
        .transfer_seat(source.signed(), Duration::from_secs(5))
        .await
        .unwrap();
    let process = &source.fleet.config.process;
    let seat_no = source.seat.facts().seat_no;
    let seat_dir = crate::seat_process::seat_dir(process, seat_no);
    for path in [
        seat_dir.join("safe-events/retained"),
        process.data_root.join("wallet/retained"),
        process.data_root.join("guardian-fee/retained"),
        crate::transfer_native::transfer_work_dir(process, seat_no).join("secret-staging"),
    ] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, "retained unless guardian staging")
            .await
            .unwrap();
    }
    source.sink.fails.store(true, Ordering::SeqCst);
    assert!(
        source
            .fleet
            .remove_transferred_seat(&source.seat, Duration::from_millis(40))
            .await
            .is_err()
    );
    assert!(seat_data_dir(process, seat_no).exists());
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Removing)
    );
    assert!(
        source
            .fleet
            .transfer_seat(source.signed(), Duration::from_millis(40))
            .await
            .is_err()
    );
    source.sink.fails.store(false, Ordering::SeqCst);
    assert!(
        !source
            .fleet
            .remove_transferred_seat(&source.seat, Duration::from_secs(5))
            .await
            .unwrap()
    );
    assert!(
        source
            .fleet
            .remove_transferred_seat(&source.seat, Duration::from_secs(5))
            .await
            .unwrap()
    );
    assert!(!seat_data_dir(process, seat_no).exists());
    assert!(!crate::transfer_native::transfer_work_dir(process, seat_no).exists());
    assert!(seat_dir.join("safe-events/retained").exists());
    assert!(process.data_root.join("wallet/retained").exists());
    assert!(process.data_root.join("guardian-fee/retained").exists());
    assert!(
        source
            .fleet
            .db
            .formed_federation_invite(&source.grant.source_seat_id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(source.fleet.db.list_seats().await.unwrap().len(), 1);
    assert!(
        source
            .sink
            .published()
            .iter()
            .any(|document| document.guardian.is_none()
                && document.transfer.as_ref().is_some_and(|transfer| transfer
                    .outgoing
                    .as_ref()
                    .is_some_and(|outgoing| outgoing.removed)))
    );
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn mismatched_and_oversize_source_requests_have_no_retirement_effect() {
    let source = source(31_261, Arc::new(FakeSeatProcessSpawner::default())).await;
    for case in 0..4 {
        let mut grant = source.grant.clone();
        match case {
            0 => grant.source.service_pubkey = source.destination.x_only_public_key().0,
            1 => grant.federation_id = FederationId("wrong".into()),
            2 => grant.fa_id = FaId(source.destination.x_only_public_key().0),
            3 => {
                grant.ts = Timestamp(1);
                grant.expires_at = Timestamp(2);
            }
            _ => unreachable!(),
        }
        assert!(
            source
                .fleet
                .transfer_seat(
                    SignedRequest::create(&grant, &source.fa).unwrap(),
                    Duration::from_millis(40),
                )
                .await
                .is_err()
        );
        assert_eq!(
            source
                .fleet
                .db
                .outgoing_transfer(&source.grant.source_seat_id)
                .await
                .unwrap(),
            None
        );
    }
    tokio::fs::write(
        seat_data_dir(&source.fleet.config.process, source.seat.facts().seat_no)
            .join("consensus.json"),
        vec![b' '; fedi_decentralized_service_manifold_guardian::SEALED_SEAT_PACKAGE_MAX_BYTES],
    )
    .await
    .unwrap();
    assert!(
        source
            .fleet
            .transfer_seat(source.signed(), Duration::from_millis(40))
            .await
            .is_err()
    );
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        None
    );
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn unproved_source_exit_keeps_capacity_and_forbids_export_and_removal() {
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![
        FakeDkgStep::FailStop,
    ]]]));
    let source = source(31_273, spawner.clone()).await;
    assert!(
        source
            .fleet
            .transfer_seat(source.signed(), Duration::from_millis(40))
            .await
            .is_err()
    );
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Stopping),
    );
    assert_eq!(source.fleet.db.active_seat_count().await.unwrap(), 1);
    assert!(
        source
            .fleet
            .remove_transferred_seat(&source.seat, Duration::from_millis(40))
            .await
            .is_err()
    );
    assert!(seat_data_dir(&source.fleet.config.process, source.seat.facts().seat_no).exists());
    let spawns = spawner.spawn_count();
    source.seat.watchdog_tick_for_test().await;
    assert_eq!(spawner.spawn_count(), spawns);
    assert!(
        source
            .fleet
            .transfer_seat(source.signed(), Duration::from_millis(40))
            .await
            .is_err()
    );
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn removal_wins_over_a_delayed_export_after_its_publication_wait() {
    let source = source(31_285, Arc::new(FakeSeatProcessSpawner::default())).await;
    let key = source.fleet.identity.derive_service_signing_key();
    let expected = source
        .seat
        .retire_for_transfer(source.signed(), key)
        .await
        .unwrap();
    source
        .fleet
        .backup
        .confirm_transfer_publication(&source.fleet.db, &expected, Duration::from_secs(5))
        .await
        .unwrap();
    let removal = source.seat.begin_transfer_removal().await.unwrap().unwrap();
    assert!(
        source
            .seat
            .export_transfer(source.signed(), key, expected)
            .await
            .is_err()
    );
    // Even an actor command cannot skip the external publication barrier.
    source.sink.fails.store(true, Ordering::SeqCst);
    // The worker may already have published this removal. Use an unconfirmed
    // exact-document mutation to exercise the command's recheck deterministically.
    let mut stale = removal;
    stale.document.decommissioned_at_ms = Some(1);
    assert!(source.seat.finish_transfer_removal(stale).await.is_err());
    assert!(seat_data_dir(&source.fleet.config.process, source.seat.facts().seat_no).exists());
    source.fleet.shutdown().await;
}
