use super::*;
use crate::transfer_state::OutgoingTransfer;
use fedi_decentralized_service_manifold_guardian::{
    Locator, SeatTransferGrant, SignedRequest, Timestamp, TransferSeatRequest,
};

struct Source {
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

    /// The pull as the grant's destination makes it.
    fn pull(&self) -> TransferSeatRequest {
        pull_as(self.signed(), &self.destination)
    }
}

fn pull_as(
    grant: SignedRequest<SeatTransferGrant>,
    destination: &secp256k1::Keypair,
) -> TransferSeatRequest {
    TransferSeatRequest {
        ts: Timestamp((crate::db::now_ms() / 1000) as u64),
        destination_service_pubkey: destination.x_only_public_key().0,
        grant,
    }
}

/// Removal deletes the guardian material in one command and reports done.
/// The archive-less tombstone is ordinary asynchronous backup work: a relay
/// outage delays the publication, never the removal.
#[tokio::test]
async fn removal_deletes_material_at_once_and_publishes_the_tombstone_afterwards() {
    let s = source(32_201, Arc::new(FakeSeatProcessSpawner::default())).await;
    s.seat
        .retire_for_transfer(s.signed(), s.fleet.identity.derive_service_signing_key())
        .await
        .unwrap();
    s.sink.fails.store(true, Ordering::SeqCst);
    let mut attempts = s.sink.attempted.subscribe();
    assert!(!s.fleet.remove_transferred_seat(&s.seat).await.unwrap());
    assert!(!seat_data_dir(&s.fleet.config.process, s.seat.facts().seat_no).exists());
    assert_eq!(
        s.fleet
            .db
            .outgoing_transfer(&s.seat.facts().seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Removed)
    );
    assert!(
        s.fleet.remove_transferred_seat(&s.seat).await.unwrap(),
        "a retry reports the removal as already done"
    );
    s.sink.fails.store(false, Ordering::SeqCst);
    tokio::time::timeout(
        Duration::from_secs(5),
        attempts.wait_for(|document| {
            document.as_ref().is_some_and(|document| {
                document.guardian.is_none()
                    && document
                        .transfer
                        .as_ref()
                        .and_then(|transfer| transfer.outgoing.as_ref())
                        .is_some_and(|outgoing| outgoing.removed)
            })
        }),
    )
    .await
    .unwrap()
    .unwrap();
    s.fleet.shutdown().await;
}

#[tokio::test]
async fn restored_retirement_fences_survive_restart_and_keep_the_formed_row() {
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
    seat.wait_for_transfer_phase_for_test(TransferPhase::Outgoing(OutgoingTransfer::Stopped))
        .await;
    assert_eq!(reopened.db.active_seat_count().await.unwrap(), 0);
    reopened.shutdown().await;
    assert!(
        reopened
            .db
            .record_transfer_removal(&s.grant.source_seat_id)
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
    assert_eq!(again.db.active_seat_count().await.unwrap(), 0);
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
    let package = crate::transfer_native::tests::material(&native);
    crate::backup::write_restored_seat_dir(
        &seat_data_dir(&config.process, facts.seat_no),
        &package.archive,
        &package.archive.digest(),
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
    Source {
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
            .transfer_seat(source.pull(), Duration::from_millis(40))
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
        .transfer_seat(source.pull(), Duration::from_secs(5))
        .await
        .unwrap();
    let opened = crate::transfer_native::TransferMaterial::from_wire(response.material).unwrap();
    let source_keys = source
        .fleet
        .identity
        .derive_seat_keys(&source.grant.source_seat_id);
    assert_eq!(
        opened.keys.iroh_api.to_bytes(),
        source_keys.iroh_api.to_bytes(),
        "the destination receives the Iroh identity the source ran under"
    );
    assert_eq!(
        opened.keys.iroh_p2p.to_bytes(),
        source_keys.iroh_p2p.to_bytes()
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

    // Only the destination the grant names receives the guardian: a caller
    // holding the grant but another key is refused, and a later valid grant
    // for another destination hands the same guardian to that destination.
    let other = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[64; 32]).unwrap();
    assert!(matches!(
        source
            .fleet
            .transfer_seat(pull_as(source.signed(), &other), Duration::from_secs(5))
            .await,
        Err(SeatVerbError::TransferGrantMismatch)
    ));
    let mut grant = source.grant.clone();
    grant.destination_service_pubkey = other.x_only_public_key().0;
    grant.destination_seat_id = SeatId::from(QuoteId([152; 32]));
    let response = source
        .fleet
        .transfer_seat(
            pull_as(SignedRequest::create(&grant, &source.fa).unwrap(), &other),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let again = crate::transfer_native::TransferMaterial::from_wire(response.material).unwrap();
    assert_eq!(again.archive.digest(), opened.archive.digest());
    assert!(again.keys.same_as(&opened.keys));
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn removal_only_deletes_guardian_paths_and_fences_later_exports() {
    let source = source(31_249, Arc::new(FakeSeatProcessSpawner::default())).await;
    source
        .fleet
        .transfer_seat(source.pull(), Duration::from_secs(5))
        .await
        .unwrap();
    let process = &source.fleet.config.process;
    let seat_no = source.seat.facts().seat_no;
    let seat_dir = crate::seat_process::seat_dir(process, seat_no);
    for path in [
        seat_dir.join("safe-events/retained"),
        process.data_root.join("wallet/retained"),
        process.data_root.join("guardian-fee/retained"),
    ] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, "retained").await.unwrap();
    }
    source.sink.fails.store(true, Ordering::SeqCst);
    assert!(
        !source
            .fleet
            .remove_transferred_seat(&source.seat)
            .await
            .unwrap()
    );
    assert!(!seat_data_dir(process, seat_no).exists());
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        Some(OutgoingTransfer::Removed)
    );
    assert!(
        source
            .fleet
            .transfer_seat(source.pull(), Duration::from_millis(40))
            .await
            .is_err()
    );
    assert!(
        source
            .fleet
            .remove_transferred_seat(&source.seat)
            .await
            .unwrap()
    );
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
    source.sink.fails.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let published = source.sink.published().into_iter().any(|document| {
                document.guardian.is_none()
                    && document.transfer.as_ref().is_some_and(|transfer| {
                        transfer
                            .outgoing
                            .as_ref()
                            .is_some_and(|outgoing| outgoing.removed)
                    })
            });
            if published {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the archive-less tombstone follows the removal asynchronously");
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
                    pull_as(
                        SignedRequest::create(&grant, &source.fa).unwrap(),
                        &source.destination,
                    ),
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
        vec![b' '; fedi_decentralized_service_manifold_guardian::SEAT_TRANSFER_MATERIAL_MAX_BYTES],
    )
    .await
    .unwrap();
    assert!(
        source
            .fleet
            .transfer_seat(source.pull(), Duration::from_millis(40))
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
async fn unproved_source_exit_forbids_export_and_removal() {
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![
        FakeDkgStep::FailStop,
    ]]]));
    let source = source(31_273, spawner.clone()).await;
    assert!(
        source
            .fleet
            .transfer_seat(source.pull(), Duration::from_millis(40))
            .await
            .is_err()
    );
    // No proven exit, no fence: the seat still counts, the guardian is still
    // this host's, and nothing can be exported or removed.
    assert_eq!(
        source
            .fleet
            .db
            .outgoing_transfer(&source.grant.source_seat_id)
            .await
            .unwrap(),
        None,
    );
    assert_eq!(source.fleet.db.active_seat_count().await.unwrap(), 1);
    assert!(
        source
            .fleet
            .remove_transferred_seat(&source.seat)
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
            .transfer_seat(source.pull(), Duration::from_millis(40))
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
    assert!(
        !source
            .fleet
            .remove_transferred_seat(&source.seat)
            .await
            .unwrap()
    );
    // The export's relay wait finished before the removal; its command
    // rechecks the durable fence and finds nothing left to export.
    assert!(
        source
            .seat
            .export_transfer(source.signed(), key, expected)
            .await
            .is_err()
    );
    assert!(!seat_data_dir(&source.fleet.config.process, source.seat.facts().seat_no).exists());
    source.fleet.shutdown().await;
}
