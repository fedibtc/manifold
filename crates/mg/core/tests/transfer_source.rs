use super::*;
use crate::transfer_state::OutgoingTransfer;
use fedi_decentralized_service_manifold_guardian::{
    Locator, SeatTransferGrant, SignedRequest, Timestamp, TransferSeatRequest, TransferSeatResponse,
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
    verification: Arc<DestinationAdmission>,
}

/// A mutable external trust observation, not an FA-supplied assertion.
struct DestinationAdmission {
    accepted: std::sync::atomic::AtomicBool,
    calls: std::sync::Mutex<Vec<secp256k1::XOnlyPublicKey>>,
    /// Simulate discovery that stalls until the caller drops its request.
    stalled: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl crate::replacement_guardian::ReplacementGuardianVerifier for DestinationAdmission {
    async fn verify(
        &self,
        destination: secp256k1::XOnlyPublicKey,
    ) -> Result<(), crate::replacement_guardian::ReplacementGuardianUnverified> {
        self.calls.lock().unwrap().push(destination);
        if self.stalled.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.accepted.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(crate::replacement_guardian::ReplacementGuardianUnverified)
        }
    }
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

    /// The destination's pull before the guardian has stopped at a session
    /// boundary: refused as awaiting the session end, which the destination
    /// retries.
    async fn pull_awaits_session_end(&self) {
        assert!(matches!(
            self.fleet.transfer_seat(self.pull(), true).await,
            Err(SeatVerbError::AwaitingSessionEnd)
        ));
    }

    /// The destination's retries once the guardian has stopped: refused
    /// until the relays confirm a backup that records the stop.
    async fn pull_after_backup(&self) -> TransferSeatResponse {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match self.fleet.transfer_seat(self.pull(), true).await {
                    Err(SeatVerbError::AwaitingBackup) => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    response => return response.unwrap(),
                }
            }
        })
        .await
        .expect("the relays confirm the stopped seat")
    }

    async fn wait_for_stopped(&self) {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.seat.wait_for_transfer_phase_for_test(
                crate::transfer_state::TransferPhase::Outgoing(OutgoingTransfer::Stopped),
            ),
        )
        .await
        .expect("the boundary exit commits the fence");
    }

    /// The whole source handover of a running guardian, at a session
    /// boundary.
    async fn hand_over(&self) {
        self.pull_awaits_session_end().await;
        self.child().await.stop_at_session_boundary();
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
                        .is_some_and(|outgoing| outgoing.phase == OutgoingTransfer::Removed)
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
    let verification = Arc::new(DestinationAdmission {
        accepted: std::sync::atomic::AtomicBool::new(true),
        calls: std::sync::Mutex::new(Vec::new()),
        stalled: std::sync::atomic::AtomicBool::new(false),
    });
    config.replacement_guardian_verifier = verification.clone();
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
        verification,
    }
}

/// Slow destination discovery must not turn concurrent transfer calls into a
/// queue that consumes every shared RPC handler permit.
#[tokio::test]
async fn saturated_replacement_admission_leaves_other_rpcs_available() {
    use fedi_decentralized_service_manifold_guardian::{
        GetStatusRequest, ManifoldGuardianError, ManifoldGuardianService, MgSignedRequest,
    };
    let source = running_source(35_610).await;
    source.verification.stalled.store(true, Ordering::SeqCst);
    let rpc = Arc::new(crate::service::ManifoldGuardianRpc::new(
        source.fleet.clone(),
        tokio::sync::watch::channel(None).1,
    ));
    let mut requests = tokio::task::JoinSet::new();
    // More simultaneous authorized calls than the daemon's 128 shared permits.
    // Only the small source-wide admitted set may remain pending.
    for _ in 0..132 {
        let rpc = rpc.clone();
        let request = MgSignedRequest::create(&source.pull(), &source.destination).unwrap();
        requests.spawn(async move { rpc.transfer_seat(request).await });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..128 {
            assert!(matches!(
                requests.join_next().await.unwrap().unwrap(),
                Err(ManifoldGuardianError::Unauthorized)
            ));
        }
        let request = SignedRequest::create(
            &GetStatusRequest {
                ts: source.grant.ts,
                fa_id: source.grant.fa_id,
                seat_id: source.grant.source_seat_id.clone(),
            },
            &source.fa,
        )
        .unwrap();
        assert_eq!(
            rpc.get_status(request).await.unwrap().status,
            fedi_decentralized_service_manifold_guardian::ServiceStatus::Running
        );
    })
    .await
    .expect("excess admissions fail fast and unrelated RPC completes");
    assert_eq!(source.verification.calls.lock().unwrap().len(), 4);
    assert!(source.outgoing().await.is_none());
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    source.verification.stalled.store(false, Ordering::SeqCst);
    source.pull_awaits_session_end().await;
}

/// A fully authorized FA cannot stop or extract a guardian for an unverified
/// replacement. Admission is repeated even after retirement and a prior release.
#[tokio::test]
async fn replacement_admission_fences_stop_and_every_package_release() {
    use fedi_decentralized_service_manifold_guardian::{
        ManifoldGuardianError, ManifoldGuardianService, MgSignedRequest,
    };
    let source = running_source(35_600).await;
    let rpc = crate::service::ManifoldGuardianRpc::new(
        source.fleet.clone(),
        tokio::sync::watch::channel(None).1,
    );
    let request = || MgSignedRequest::create(&source.pull(), &source.destination).unwrap();
    let child = source.child().await;
    eventually("the guardian is configured", || {
        source.seat.child_configured_for_test()
    })
    .await;
    source.verification.accepted.store(false, Ordering::SeqCst);
    assert!(matches!(
        rpc.transfer_seat(request()).await,
        Err(ManifoldGuardianError::Unauthorized)
    ));
    assert_eq!(source.outgoing().await, None);
    assert!(child.state().shutdown_after_current_session.is_empty());
    assert!(source.seat.child_configured_for_test());
    assert_eq!(
        *source.verification.calls.lock().unwrap(),
        vec![source.grant.destination_service_pubkey]
    );

    source.verification.accepted.store(true, Ordering::SeqCst);
    source.hand_over().await;
    source.pull_after_backup().await;
    source.verification.accepted.store(false, Ordering::SeqCst);
    assert!(matches!(
        rpc.transfer_seat(request()).await,
        Err(ManifoldGuardianError::Unauthorized)
    ));
    assert_eq!(source.outgoing().await, Some(OutgoingTransfer::Stopped));
    source.verification.accepted.store(true, Ordering::SeqCst);
    assert!(rpc.transfer_seat(request()).await.is_ok());
    source.fleet.shutdown().await;
}

/// Without a policy that enables transfer, a running source with no stored
/// transfer refuses the pull and keeps running its guardian.
#[tokio::test]
async fn a_source_without_the_policy_starts_no_transfer() {
    let source = running_source(35_500).await;
    assert!(matches!(
        source.fleet.transfer_seat(source.pull(), false).await,
        Err(SeatVerbError::TransferDisabled)
    ));
    assert_eq!(source.outgoing().await, None);
    assert_eq!(source.fleet.db.active_seat_count().await.unwrap(), 1);
    source.fleet.shutdown().await;
}

#[tokio::test]
async fn source_releases_the_guardian_only_after_the_relays_confirm_its_stop() {
    let source = running_source(31_237).await;
    let spawner = source.spawner.clone();
    // A confirmed backup of the running seat does not stand in for one that
    // records the stop.
    eventually("the running seat's backup is confirmed", || {
        !source.sink.published().is_empty()
    })
    .await;
    source.sink.fails.store(true, Ordering::SeqCst);
    source.hand_over().await;
    assert!(matches!(
        source.fleet.transfer_seat(source.pull(), true).await,
        Err(SeatVerbError::AwaitingBackup)
    ));
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
    let response = source.pull_after_backup().await;
    // The guardian stopped for this transfer, so the source hands it over
    // even when its policy no longer lets it start one.
    assert!(
        source
            .fleet
            .transfer_seat(source.pull(), false)
            .await
            .is_ok(),
        "a stopped source answers without the policy"
    );
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
            .transfer_seat(pull_as(source.signed(), &other), true)
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
            true,
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
    source.pull_after_backup().await;
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
            .transfer_seat(source.pull(), true)
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
                            .is_some_and(|outgoing| outgoing.phase == OutgoingTransfer::Removed)
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
    // A configured, running guardian: a valid pull would ask it to stop.
    let source = running_source(31_261).await;
    let child = source.child().await;
    // Only a configured guardian is asked to stop; checking against one that
    // is still starting would make "no stop was requested" vacuous.
    eventually("the guardian is configured", || {
        source.seat.child_configured_for_test()
    })
    .await;
    let untouched = |what: &str| {
        assert!(
            child.state().shutdown_after_current_session.is_empty(),
            "{what}: no boundary stop was requested"
        );
        assert_eq!(
            source.spawner.spawn_count(),
            1,
            "{what}: the guardian kept running"
        );
        assert!(
            source.seat.child_configured_for_test(),
            "{what}: the guardian kept running"
        );
    };
    for case in 0..4 {
        let mut grant = source.grant.clone();
        match case {
            0 => grant.source.service_pubkey = source.destination.x_only_public_key().0,
            1 => grant.federation_id = FederationId("wrong".into()),
            2 => grant.fa_id = FaId(source.destination.x_only_public_key().0),
            // A grant signed in the future beyond the clock allowance.
            3 => grant.ts = Timestamp((crate::db::now_ms() / 1000) as u64 + 2 * 60 * 60),
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
                    true
                )
                .await
                .is_err()
        );
        assert_eq!(source.outgoing().await, None);
        untouched(&format!("mismatch {case}"));
    }
    // Valid JSON whitespace alone can push a healthy guardian's archive past
    // the transfer cap. The source refuses before it asks for the stop.
    tokio::fs::write(
        seat_data_dir(&source.fleet.config.process, source.seat.facts().seat_no)
            .join("consensus.json"),
        vec![b' '; fedi_decentralized_service_manifold_guardian::SEAT_TRANSFER_MATERIAL_MAX_BYTES],
    )
    .await
    .unwrap();
    assert!(matches!(
        source.fleet.transfer_seat(source.pull(), true).await,
        Err(SeatVerbError::Internal(_))
    ));
    assert_eq!(source.outgoing().await, None);
    untouched("oversize archive");
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
            .transfer_seat(source.pull(), true)
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
            .transfer_seat(source.pull(), true)
            .await
            .is_err()
    );
    source.fleet.shutdown().await;
}

// Session-boundary handover (SPEC-guardian-transfer step 3). The fake child
// stands in for fedimintd's process and API.

/// The guardian keeps running, and the destination receives nothing, until
/// fedimintd exits at the session boundary; it is then never started again.
#[tokio::test]
async fn source_runs_its_guardian_until_its_boundary_exit_and_never_restarts_it() {
    let s = running_source(33_000).await;
    s.pull_awaits_session_end().await;
    s.pull_awaits_session_end().await;
    let child = s.child().await;
    let api_auth = s
        .fleet
        .identity
        .derive_seat_keys(&s.grant.source_seat_id)
        .api_auth;
    assert_eq!(
        child.state().shutdown_after_current_session,
        vec![Some(api_auth.clone()), Some(api_auth)],
        "every pull asks again; fedimintd keeps the earliest boundary"
    );
    assert_eq!(s.outgoing().await, None);
    assert_eq!(s.spawner.spawn_count(), 1, "the guardian was not stopped");

    child.stop_at_session_boundary();
    s.wait_for_stopped().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    s.seat.watchdog_tick_for_test().await;
    assert_eq!(
        s.spawner.spawn_count(),
        1,
        "the supervisor never restarts the source after its boundary exit"
    );
    let response = s.pull_after_backup().await;
    crate::transfer_native::TransferMaterial::from_wire(response.material).unwrap();
    s.fleet.shutdown().await;
}

/// Any other exit is a crash, including fedimintd's ordinary shutdown code:
/// the guardian restarts from its own database, which continues its session
/// rather than forking it, and the next pull asks the new process.
#[tokio::test]
async fn any_other_exit_respawns_and_the_next_pull_asks_again() {
    let s = running_source(33_012).await;
    for (n, code) in [1, 255, 101].into_iter().enumerate() {
        s.pull_awaits_session_end().await;
        s.child().await.exit(code);
        eventually("the guardian respawns", || s.spawner.spawn_count() == n + 2).await;
        assert_eq!(s.outgoing().await, None, "exit code {code}");
    }
    s.pull_awaits_session_end().await;
    let child = s.child().await;
    assert_eq!(child.state().shutdown_after_current_session.len(), 1);
    child.stop_at_session_boundary();
    s.wait_for_stopped().await;
    assert_eq!(s.spawner.spawn_count(), 4);
    s.fleet.shutdown().await;
}

/// A boundary exit hands over only a seat that still qualifies; a
/// decommissioned one restarts as any other exit would.
#[tokio::test]
async fn boundary_exit_of_a_decommissioned_seat_is_no_handover() {
    let s = running_source(33_024).await;
    s.pull_awaits_session_end().await;
    s.fleet
        .db
        .decommission_seat(&s.grant.source_seat_id)
        .await
        .unwrap();
    s.child().await.stop_at_session_boundary();
    eventually("the guardian respawns", || s.spawner.spawn_count() == 2).await;
    assert_eq!(s.outgoing().await, None);
    s.fleet.shutdown().await;
}

/// An operator's shutdown scheduled through fedimintd's older `shutdown`
/// endpoint exits with fedimintd's ordinary code (-1, seen as 255). It is no
/// handover: the guardian restarts and its seat stays here.
#[tokio::test]
async fn legacy_scheduled_shutdown_restarts_the_guardian() {
    let s = running_source(33_036).await;
    s.child().await.exit(255);
    eventually("the guardian respawns", || s.spawner.spawn_count() == 2).await;
    assert_eq!(s.outgoing().await, None);
    s.fleet.shutdown().await;
}

/// The pull arrives while the source guardian is down. Nothing is asked and
/// nothing handed over; the restarted guardian is asked on the next pull.
#[tokio::test]
async fn pull_while_the_guardian_is_down_waits_for_its_restart() {
    let s = running_source_with_backoff(33_072, Duration::from_secs(2)).await;
    s.child().await.exit(1);
    eventually("the MG sees the guardian down", || {
        s.seat.child_slot_empty_for_test()
    })
    .await;
    s.pull_awaits_session_end().await;
    assert_eq!(s.outgoing().await, None);
    eventually("the guardian restarts", || s.spawner.spawn_count() == 2).await;
    s.hand_over().await;
    assert_eq!(s.spawner.spawn_count(), 2);
    s.fleet.shutdown().await;
}

/// The connection between the MG and its child is cut when the MG asks. The
/// guardian keeps running and the next pull asks again.
#[tokio::test]
async fn pull_while_the_local_api_is_cut_asks_again_on_the_next_pull() {
    let s = running_source(33_084).await;
    let child = s.child().await;
    child.modify_state(|state| state.api_cut = true);
    s.pull_awaits_session_end().await;
    assert!(child.state().shutdown_after_current_session.is_empty());
    child.modify_state(|state| state.api_cut = false);
    s.hand_over().await;
    assert_eq!(child.state().shutdown_after_current_session.len(), 1);
    assert_eq!(s.spawner.spawn_count(), 1);
    s.fleet.shutdown().await;
}
