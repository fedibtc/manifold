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
    spawner: Arc<FakeSeatProcessSpawner>,
}

impl Source {
    fn signed(&self) -> SignedRequest<SeatTransferGrant> {
        SignedRequest::create(&self.grant, &self.fa).unwrap()
    }

    /// The pull as the grant's destination makes it.
    fn pull(&self) -> TransferSeatRequest {
        pull_as(self.signed(), &self.destination)
    }

    async fn outgoing(&self) -> Option<OutgoingTransfer> {
        self.fleet
            .db
            .outgoing_transfer(&self.grant.source_seat_id)
            .await
            .unwrap()
    }

    async fn child(&self) -> FakeSeatChildHandle {
        self.spawner.child(&self.grant.source_seat_id).await
    }

    /// The destination's pull while the guardian still runs: refused as a
    /// running seat, which the destination retries.
    async fn pull_refused_while_running(&self) {
        assert!(matches!(
            self.fleet
                .transfer_seat(self.pull(), Duration::from_secs(5))
                .await,
            Err(SeatVerbError::WrongState {
                status: ServiceStatus::Running
            })
        ));
    }

    /// The live child once it has accepted `requests` shutdown requests.
    async fn asked_child(&self, requests: usize) -> FakeSeatChildHandle {
        let child = self.child().await;
        eventually("the MG asks fedimintd to stop after its session", || {
            child.state().shutdown_after_current_session.len() >= requests
        })
        .await;
        child
    }

    async fn wait_for_stopped(&self) {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.seat.wait_for_transfer_phase_for_test(
                crate::transfer_state::TransferPhase::Outgoing(OutgoingTransfer::Stopped),
            ),
        )
        .await
        .expect("the handover commits the fence");
    }

    /// The whole source handover of a running guardian, at a session
    /// boundary.
    async fn hand_over(&self) {
        self.pull_refused_while_running().await;
        self.asked_child(1).await.stop_between_sessions().await;
        self.wait_for_stopped().await;
    }
}

/// Poll a condition the MG reaches through its own timers.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
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
    let s = running_source(32_201).await;
    s.hand_over().await;
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
    source_with(port, Duration::from_millis(10), |_| spawner).await
}

async fn running_source(port: u16) -> Source {
    running_source_with_backoff(port, Duration::from_millis(10)).await
}

/// A source whose every child is the formed guardian serving consensus.
async fn running_source_with_backoff(port: u16, backoff: Duration) -> Source {
    source_with(port, backoff, |invite| {
        Arc::new(FakeSeatProcessSpawner::configured(invite))
    })
    .await
}

async fn source_with(
    port: u16,
    respawn_backoff: Duration,
    spawner: impl FnOnce(&str) -> Arc<FakeSeatProcessSpawner>,
) -> Source {
    let root = TempDir::new().unwrap();
    let mut config = config(&root, 1, port).await;
    config.respawn.initial_backoff = respawn_backoff;
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
    let spawner = spawner(&invite.to_string());
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
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
        spawner,
    }
}

#[tokio::test]
async fn source_stops_before_export_and_requires_exact_tombstone_not_old_archive() {
    let source = running_source(31_237).await;
    let spawner = source.spawner.clone();
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
    source.hand_over().await;
    assert!(
        source
            .fleet
            .transfer_seat(source.pull(), Duration::from_millis(40))
            .await
            .is_err()
    );
    assert_eq!(source.outgoing().await, Some(OutgoingTransfer::Stopped));
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
    let source = running_source(33_100).await;
    source.hand_over().await;
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
    let source = running_source(31_285).await;
    source.hand_over().await;
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

// Session-boundary handover (SPEC-guardian-transfer step 3). The fake child
// stands in for fedimintd's process and API; the database it leaves behind
// is a real one, so the MG's proof reads what fedimintd would write.

/// The guardian keeps running, and the destination receives nothing, until
/// it stops between sessions; it is then never started again.
#[tokio::test]
async fn source_runs_its_guardian_until_it_stops_between_sessions_and_never_restarts_it() {
    let s = running_source(33_000).await;
    for _ in 0..3 {
        s.pull_refused_while_running().await;
    }
    let child = s.asked_child(1).await;
    let api_auth = s
        .fleet
        .identity
        .derive_seat_keys(&s.grant.source_seat_id)
        .api_auth;
    assert_eq!(
        child.state().shutdown_after_current_session,
        vec![Some(api_auth)],
        "one authenticated request, however often the destination pulls"
    );
    assert_eq!(s.outgoing().await, None);
    assert_eq!(s.spawner.spawn_count(), 1, "the guardian was not stopped");
    assert!(matches!(
        s.seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running { .. },
            ..
        }
    ));

    child.stop_between_sessions().await;
    s.wait_for_stopped().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    s.seat.watchdog_tick_for_test().await;
    assert_eq!(
        s.spawner.spawn_count(),
        1,
        "the supervisor never restarts the source after its boundary exit"
    );
    let response = s
        .fleet
        .transfer_seat(s.pull(), Duration::from_secs(5))
        .await
        .unwrap();
    crate::transfer_native::TransferMaterial::from_wire(response.material).unwrap();
    s.fleet.shutdown().await;
}

/// fedimintd dies inside the session, taking its in-memory schedule along.
/// The respawned guardian resumes that session from its own database, which
/// is no fork, and the MG asks it again.
#[tokio::test]
async fn guardian_crash_inside_the_session_respawns_and_asks_again() {
    let s = running_source(33_012).await;
    s.pull_refused_while_running().await;
    s.asked_child(1).await.crash_inside_session().await;
    eventually("the crashed guardian respawns", || {
        s.spawner.spawn_count() == 2
    })
    .await;
    assert_eq!(s.outgoing().await, None);
    s.pull_refused_while_running().await;
    s.asked_child(1).await.stop_between_sessions().await;
    s.wait_for_stopped().await;
    assert_eq!(s.spawner.spawn_count(), 2);
    s.fleet.shutdown().await;
}

/// An exit that leaves no readable database proves nothing and counts as a
/// crash.
#[tokio::test]
async fn exit_without_a_readable_database_counts_as_a_crash() {
    let s = running_source(33_024).await;
    s.pull_refused_while_running().await;
    s.asked_child(1).await.exit_without_database();
    eventually("the guardian respawns", || s.spawner.spawn_count() == 2).await;
    assert_eq!(s.outgoing().await, None);
    s.fleet.shutdown().await;
}

/// The handover lives in memory: a daemon restart forgets it with the
/// child's schedule, the guardian simply runs on, and the destination's
/// next pull starts the handover again.
#[tokio::test]
async fn mg_restart_while_waiting_forgets_the_handover_until_the_next_pull() {
    let mut s = running_source(33_036).await;
    s.pull_refused_while_running().await;
    s.asked_child(1).await;
    s.fleet.shutdown().await;
    let reopened = Fleet::open_with_wallet(
        s.fleet.db.clone(),
        s.fleet.config.clone(),
        async |_| Ok(Arc::new(NoWallet) as _),
        async |_| Ok(s.sink.clone() as _),
    )
    .await
    .unwrap();
    s.seat = reopened.seat_by_id(&s.grant.source_seat_id).unwrap();
    s.fleet = Arc::new(reopened);
    eventually("the guardian starts again", || s.spawner.spawn_count() == 2).await;
    let child = s.child().await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(child.state().shutdown_after_current_session.is_empty());
    assert_eq!(s.outgoing().await, None);
    s.hand_over().await;
    assert_eq!(s.spawner.spawn_count(), 2);
    s.fleet.shutdown().await;
}

/// Too many peers are offline for the session to end. The handover waits
/// with the guardian running and never falls back to a mid-session stop;
/// once the session ends, it completes.
#[tokio::test]
async fn session_that_cannot_end_keeps_the_guardian_running_until_it_does() {
    let s = running_source(33_048).await;
    s.pull_refused_while_running().await;
    let child = s.asked_child(1).await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    s.pull_refused_while_running().await;
    assert_eq!(s.outgoing().await, None);
    assert_eq!(s.spawner.spawn_count(), 1);
    assert_eq!(
        child.state().shutdown_after_current_session.len(),
        1,
        "an accepted request is not repeated"
    );
    child.stop_between_sessions().await;
    s.wait_for_stopped().await;
    s.fleet.shutdown().await;
}

/// The pull arrives while the source guardian is down between sessions, as
/// after an earlier scheduled stop. Its database proves it, so it hands over
/// without ever running again.
#[tokio::test]
async fn pull_while_the_guardian_is_down_between_sessions_hands_over_at_once() {
    let s = running_source_with_backoff(33_060, Duration::from_secs(600)).await;
    s.child().await.stop_between_sessions().await;
    eventually("the MG sees the guardian down", || {
        s.seat.child_slot_empty_for_test()
    })
    .await;
    s.pull_refused_while_running().await;
    s.wait_for_stopped().await;
    assert_eq!(s.spawner.spawn_count(), 1);
    s.fleet
        .transfer_seat(s.pull(), Duration::from_secs(5))
        .await
        .unwrap();
    s.fleet.shutdown().await;
}

/// The pull arrives while the source guardian is down inside a session. It
/// restarts as usual and hands over once that session ends.
#[tokio::test]
async fn pull_while_the_guardian_is_down_inside_a_session_waits_for_its_restart() {
    let s = running_source_with_backoff(33_072, Duration::from_secs(3)).await;
    s.child().await.crash_inside_session().await;
    eventually("the MG sees the guardian down", || {
        s.seat.child_slot_empty_for_test()
    })
    .await;
    s.pull_refused_while_running().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(s.outgoing().await, None);
    eventually("the guardian restarts", || s.spawner.spawn_count() == 2).await;
    s.asked_child(1).await.stop_between_sessions().await;
    s.wait_for_stopped().await;
    s.fleet.shutdown().await;
}

/// The connection between the MG and its child is cut when the MG asks. It
/// asks again until the request lands; the guardian keeps running.
#[tokio::test]
async fn request_retries_while_the_local_api_is_cut() {
    let s = running_source(33_084).await;
    let child = s.child().await;
    child.modify_state(|state| state.api_cut = true);
    s.pull_refused_while_running().await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(child.state().shutdown_after_current_session.is_empty());
    assert_eq!(s.outgoing().await, None);
    child.modify_state(|state| state.api_cut = false);
    s.asked_child(1).await.stop_between_sessions().await;
    s.wait_for_stopped().await;
    assert_eq!(s.spawner.spawn_count(), 1);
    s.fleet.shutdown().await;
}

/// A cut after the request landed does not matter: the proof comes from the
/// stopped database, not from the API.
#[tokio::test]
async fn api_cut_after_the_request_still_hands_over_at_the_boundary() {
    let s = running_source(33_096).await;
    s.pull_refused_while_running().await;
    let child = s.asked_child(1).await;
    child.modify_state(|state| state.api_cut = true);
    child.stop_between_sessions().await;
    s.wait_for_stopped().await;
    assert_eq!(s.spawner.spawn_count(), 1);
    s.fleet.shutdown().await;
}
