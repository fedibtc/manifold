use super::*;
use crate::transfer_native::{ValidatedGuardian, validate_material};
use crate::transfer_state::{IncomingState, TransferMove, TransferPhase};
use fedi_decentralized_service_manifold_guardian::{
    Locator, SeatTransferGrant, SignedRequest, Timestamp,
};

struct SourceCall {
    grant: SignedRequest<SeatTransferGrant>,
    reply: tokio::sync::oneshot::Sender<
        fedi_decentralized_service_manifold_guardian::TransferSeatResponse,
    >,
}

struct ControlledSource(tokio::sync::mpsc::UnboundedSender<SourceCall>);

#[async_trait::async_trait]
impl crate::transfer_source::TransferSource for ControlledSource {
    async fn transfer(
        &self,
        source: &Locator,
        grant: SignedRequest<SeatTransferGrant>,
    ) -> Result<
        fedi_decentralized_service_manifold_guardian::FmResult<
            fedi_decentralized_service_manifold_guardian::TransferSeatResponse,
        >,
        crate::transfer_source::TransferSourceUnavailable,
    > {
        let verified = grant
            .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
            .unwrap();
        assert_eq!(source, &verified.source);
        let (reply, done) = tokio::sync::oneshot::channel();
        self.0
            .send(SourceCall { grant, reply })
            .map_err(|_| crate::transfer_source::TransferSourceUnavailable)?;
        done.await
            .map(Ok)
            .map_err(|_| crate::transfer_source::TransferSourceUnavailable)
    }
}

fn respond(d: &Destination, call: SourceCall) {
    let grant = call
        .grant
        .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
        .unwrap();
    let sealed_package = crate::transfer_package::seal(
        &d.material,
        &grant,
        &secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[83; 32]).unwrap(),
    )
    .unwrap();
    call.reply
        .send(fedi_decentralized_service_manifold_guardian::TransferSeatResponse { sealed_package })
        .unwrap();
}

async fn wait_phase(seat: &Seat, phase: TransferPhase) {
    tokio::time::timeout(
        Duration::from_secs(15),
        seat.wait_for_transfer_phase_for_test(phase),
    )
    .await
    .unwrap();
}

struct Destination {
    _pause: Option<crate::transfer_worker::TransferPause>,
    _root: TempDir,
    fleet: Arc<Fleet>,
    seat: Arc<Seat>,
    sink: Arc<FakeBackupSink>,
    spawner: Arc<FakeSeatProcessSpawner>,
    grant: SeatTransferGrant,
    fa: secp256k1::Keypair,
    material: crate::transfer_package::TransferMaterial,
}

impl Destination {
    async fn publication(&self) -> crate::backup::SeatPublication {
        crate::backup::seat_publication_plan(
            &self.fleet.db,
            &self.fleet.config.process,
            self.seat.facts(),
            None,
        )
        .await
        .unwrap()
    }

    async fn assert_pending(&self) {
        assert!(matches!(
            self.seat.report().await.unwrap(),
            SeatReport::Active {
                phase: SeatPhase::Transfer {
                    status: ServiceStatus::AwaitingTransfer
                },
                ..
            }
        ));
    }
    async fn admit(&self) -> crate::transfer_state::IncomingTransfer {
        self.seat
            .admit_transfer(
                SignedRequest::create(&self.grant, &self.fa).unwrap(),
                self.fleet.identity.derive_service_pubkey(),
                None,
            )
            .await
            .unwrap()
    }

    async fn validate(&self) -> ValidatedGuardian {
        validate_material(
            &self.fleet.db,
            &self.fleet.config.process,
            self.seat.facts().seat_no,
            crate::transfer_package::TransferMaterial::decode(&self.material.encode().unwrap())
                .unwrap(),
            self.grant.federation_id.clone(),
            Arc::new(crate::transfer_native::tests::CoreConfigValidator),
        )
        .await
        .unwrap()
    }
}

async fn destination(port: u16) -> Destination {
    destination_with_capabilities(
        port,
        Arc::new(crate::transfer_source::UnavailableTransferSource),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await
}

async fn destination_with_capabilities(
    port: u16,
    source: Arc<dyn crate::transfer_source::TransferSource>,
    validator: Arc<dyn crate::transfer_native::GuardianConfigValidator>,
) -> Destination {
    let root = TempDir::new().unwrap();
    let mut config = config(&root, 1, port).await;
    config.guardian_config_validator = validator;
    config.transfer_source = source;
    let native = crate::transfer_native::tests::native_config();
    let material = crate::transfer_native::tests::material(&native);
    let invite = fedimint_core::invite_code::InviteCode::new(
        native.consensus.api_endpoints()[&native.local.identity]
            .url
            .clone(),
        native.local.identity,
        native.calculate_federation_id(),
        None,
    )
    .to_string();
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![
        vec![FakeDkgStep::Hang],
        // First import start falsely claims config absent. It must never DKG.
        vec![FakeDkgStep::Hang],
        vec![
            FakeDkgStep::Message(ChildMessage::Hello {
                proto: PROTOCOL_VERSION,
                code_version: "fake-fedimintd".to_owned(),
                state: ChildState::AlreadyConfigured {
                    invite_code: invite,
                },
            }),
            FakeDkgStep::Hang,
        ],
    ]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let db = Db::open(root.path()).await.unwrap();
    let identity = db.load_identity().await.unwrap().unwrap();
    let fa = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[81; 32]).unwrap();
    let facts = crate::test_support::insert_test_seat(
        &db,
        NewSeat {
            seat_id: SeatId::from(QuoteId([181; 32])),
            fa_id: FaId(fa.x_only_public_key().0),
            plan: Plan::InfiniteBestEffort { price_msats: 0 },
            federation_size: FederationSize(1),
            payment: None,
        },
    )
    .await;
    let now = (crate::db::now_ms() / 1000) as u64;
    let grant = SeatTransferGrant {
        ts: Timestamp(now),
        expires_at: Timestamp(now + 3600),
        fa_id: facts.fa_id,
        federation_id: FederationId(native.calculate_federation_id().to_string()),
        source: Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[82; 32]).public(),
            ),
            secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[83; 32])
                .unwrap()
                .x_only_public_key()
                .0,
        ),
        source_seat_id: SeatId::from(QuoteId([182; 32])),
        destination_service_pubkey: identity.derive_service_pubkey(),
        destination_seat_id: facts.seat_id.clone(),
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
    let seat = fleet.seat_by_id(&facts.seat_id).unwrap();
    let pause = fleet.transfers.quiesce(&facts.seat_id).await;
    Destination {
        _pause: Some(pause),
        _root: root,
        fleet,
        seat,
        sink,
        spawner,
        grant,
        fa,
        material,
    }
}

#[tokio::test]
async fn worker_pull_authenticates_imports_and_retries_without_refetching_accepted_material() {
    use fedi_decentralized_service_manifold_guardian::{ManifoldGuardianService, PullSeatRequest};
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let mut d = destination_with_capabilities(
        32_145,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    let rpc = crate::service::ManifoldGuardianRpc::new(
        d.fleet.clone(),
        tokio::sync::watch::channel(None).1,
    );
    let request = PullSeatRequest {
        ts: Timestamp((crate::db::now_ms() / 1000) as u64),
        fa_id: d.grant.fa_id,
        seat_id: d.grant.destination_seat_id.clone(),
        grant: SignedRequest::create(&d.grant, &d.fa).unwrap(),
        completion_callback: None,
    };
    rpc.pull_seat(SignedRequest::create(&request, &d.fa).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        d.fleet
            .db
            .incoming_transfer(&request.seat_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        IncomingState::Waiting { .. }
    ));
    drop(d._pause.take());
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.grant, request.grant);
    respond(&d, call);
    wait_phase(&d.seat, TransferPhase::Imported).await;
    assert_eq!(
        d.spawner.spawn_count(),
        3,
        "wrong configured hello was retried, never used for DKG"
    );
    assert!(
        calls.try_recv().is_err(),
        "accepted retries never ask the source again"
    );
    rpc.pull_seat(SignedRequest::create(&request, &d.fa).unwrap())
        .await
        .unwrap();
    assert_eq!(
        d.spawner.spawn_count(),
        3,
        "same-move replay leaves running child alone"
    );
    d.fleet.shutdown().await;
}

#[tokio::test]
async fn worker_expires_an_outstanding_rpc_and_renewal_gets_a_new_job() {
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let mut d = destination_with_capabilities(
        32_149,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    d.grant.expires_at = Timestamp((crate::db::now_ms() / 1000) as u64 + 2);
    let old = d.admit().await;
    drop(d._pause.take());
    let mut call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    wait_phase(&d.seat, TransferPhase::Expired).await;
    tokio::time::timeout(Duration::from_secs(1), call.reply.closed())
        .await
        .unwrap();
    d.grant.ts = Timestamp((crate::db::now_ms() / 1000) as u64);
    d.grant.expires_at = Timestamp(d.grant.ts.0 + 3600);
    let new = d.admit().await;
    assert!(new.generation > old.generation);
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    respond(&d, call);
    wait_phase(&d.seat, TransferPhase::Imported).await;
    d.fleet.shutdown().await;
}

struct GatedValidator {
    entered: Notify,
    released: std::sync::Mutex<bool>,
    release: std::sync::Condvar,
}

impl crate::transfer_native::GuardianConfigValidator for GatedValidator {
    fn validate(
        &self,
        config: &fedimint_server::config::ServerConfig,
        network: bitcoin::Network,
    ) -> anyhow::Result<()> {
        self.entered.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.release.wait(released).unwrap();
        }
        crate::transfer_native::tests::CoreConfigValidator.validate(config, network)
    }
}

impl GatedValidator {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

struct ReleaseValidator(Arc<GatedValidator>);

impl Drop for ReleaseValidator {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn worker_shutdown_joins_native_validation_before_returning_or_accepting() {
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let validator = Arc::new(GatedValidator {
        entered: Notify::new(),
        released: std::sync::Mutex::new(false),
        release: std::sync::Condvar::new(),
    });
    let _release_on_panic = ReleaseValidator(validator.clone());
    let mut d = destination_with_capabilities(
        32_153,
        Arc::new(ControlledSource(source)),
        validator.clone(),
    )
    .await;
    d.admit().await;
    drop(d._pause.take());
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    respond(&d, call);
    tokio::time::timeout(Duration::from_secs(5), validator.entered.notified())
        .await
        .unwrap();
    let fleet = d.fleet.clone();
    let mut shutdown = tokio::spawn(async move { fleet.shutdown().await });
    let still_joining = tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
        .await
        .is_err();
    *validator.released.lock().unwrap() = true;
    validator.release.notify_all();
    assert!(still_joining, "shutdown must join, not drop, native work");
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        IncomingState::Waiting { .. }
    ));
    assert!(!seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no).exists());
    assert_eq!(
        std::fs::read_dir(crate::transfer_native::transfer_work_dir(
            &d.fleet.config.process,
            d.seat.facts().seat_no
        ))
        .unwrap()
        .count(),
        0
    );
}

#[tokio::test]
async fn worker_cancelled_quiesce_resumes_only_after_joining_native_work() {
    let validator = Arc::new(GatedValidator {
        entered: Notify::new(),
        released: std::sync::Mutex::new(false),
        release: std::sync::Condvar::new(),
    });
    let _release_on_panic = ReleaseValidator(validator.clone());
    let mut d = destination_with_capabilities(
        32_193,
        Arc::new(crate::transfer_source::UnavailableTransferSource),
        validator.clone(),
    )
    .await;
    // Consume the fixture's completed pause request before watching the
    // cancelled pause below.
    d.fleet.transfers.quiesce_requested_for_test().await;
    let record = d.admit().await;
    d.seat
        .install_transfer(record.generation, record.binding, d.validate().await)
        .await
        .unwrap();
    drop(d._pause.take());
    tokio::time::timeout(Duration::from_secs(5), validator.entered.notified())
        .await
        .unwrap();
    let fleet = d.fleet.clone();
    let seat = d.seat.clone();
    let pause = tokio::spawn(async move { fleet.transfers.quiesce(&seat.facts().seat_id).await });
    tokio::time::timeout(
        Duration::from_secs(5),
        d.fleet.transfers.quiesce_requested_for_test(),
    )
    .await
    .unwrap();
    pause.abort();
    assert!(matches!(pause.await, Err(error) if error.is_cancelled()));
    assert!(seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no).exists());
    let work_dir =
        crate::transfer_native::transfer_work_dir(&d.fleet.config.process, d.seat.facts().seat_no);
    assert_ne!(
        std::fs::read_dir(&work_dir).unwrap().count(),
        0,
        "native work is still owned while cancellation drains"
    );
    validator.release();
    wait_phase(&d.seat, TransferPhase::Imported).await;
    assert_eq!(std::fs::read_dir(work_dir).unwrap().count(), 0);
    d.fleet.shutdown().await;
}

struct CountingValidator(Notify);

impl crate::transfer_native::GuardianConfigValidator for CountingValidator {
    fn validate(
        &self,
        config: &fedimint_server::config::ServerConfig,
        network: bitcoin::Network,
    ) -> anyhow::Result<()> {
        self.0.notify_one();
        crate::transfer_native::tests::CoreConfigValidator.validate(config, network)
    }
}

#[tokio::test]
async fn worker_parks_conflicting_accepted_files_until_same_seat_replay() {
    let validator = Arc::new(CountingValidator(Notify::new()));
    let mut d = destination_with_capabilities(
        32_197,
        Arc::new(crate::transfer_source::UnavailableTransferSource),
        validator.clone(),
    )
    .await;
    let record = d.admit().await;
    d.seat
        .install_transfer(record.generation, record.binding, d.validate().await)
        .await
        .unwrap();
    let local = seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no).join("local.json");
    tokio::fs::write(&local, "conflicting local config")
        .await
        .unwrap();
    drop(d._pause.take());
    tokio::time::timeout(Duration::from_secs(5), validator.0.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), validator.0.notified())
            .await
            .is_err(),
        "local conflict must not repeatedly decrypt on a timer"
    );
    assert!(matches!(
        d.fleet
            .db
            .incoming_transfer(&d.seat.facts().seat_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        IncomingState::Importing { .. }
    ));
    tokio::fs::write(&local, &d.material.archive.local_json)
        .await
        .unwrap();
    d.admit().await;
    wait_phase(&d.seat, TransferPhase::Imported).await;
    d.fleet.shutdown().await;
}

#[tokio::test]
async fn worker_reopens_accepted_import_after_expiry_without_source_authority() {
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let mut d = destination_with_capabilities(
        32_157,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    let guardian = d.validate().await;
    d.grant.expires_at = Timestamp((crate::db::now_ms() / 1000) as u64 + 2);
    let record = d.admit().await;
    d.seat
        .install_transfer(record.generation, record.binding, guardian)
        .await
        .unwrap();
    d.fleet.shutdown().await;
    let remaining = ((d.grant.expires_at.0 + 1) * 1000).saturating_sub(crate::db::now_ms() as u64);
    tokio::time::sleep(Duration::from_millis(remaining)).await;
    let reopened = Fleet::open_with_wallet(
        d.fleet.db.clone(),
        d.fleet.config.clone(),
        async |_| Ok(Arc::new(NoWallet) as _),
        async |_| Ok(d.sink.clone() as _),
    )
    .await
    .unwrap();
    let seat = reopened.seat_by_id(&d.grant.destination_seat_id).unwrap();
    wait_phase(&seat, TransferPhase::Imported).await;
    assert!(calls.try_recv().is_err());
    reopened.shutdown().await;
}

#[tokio::test]
async fn worker_bounds_outbound_attempts_and_cancels_permit_waiters() {
    let root = TempDir::new().unwrap();
    let mut config = config(&root, 5, 32_169).await;
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    config.transfer_source = Arc::new(ControlledSource(source));
    let db = Db::open(root.path()).await.unwrap();
    let identity = db.load_identity().await.unwrap().unwrap();
    let fa = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[81; 32]).unwrap();
    let now = (crate::db::now_ms() / 1000) as u64;
    let mut grants = Vec::new();
    for index in 0..5 {
        let facts = crate::test_support::insert_test_seat(
            &db,
            NewSeat {
                seat_id: SeatId::from(QuoteId([190 + index; 32])),
                fa_id: FaId(fa.x_only_public_key().0),
                plan: Plan::InfiniteBestEffort { price_msats: 0 },
                federation_size: FederationSize(1),
                payment: None,
            },
        )
        .await;
        grants.push(SeatTransferGrant {
            ts: Timestamp(now),
            expires_at: Timestamp(now + 3600),
            fa_id: facts.fa_id,
            federation_id: FederationId(format!("{index:064x}")),
            source: Locator::new(
                fedi_iroh_rpc::iroh::EndpointAddr::new(
                    fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[82; 32]).public(),
                ),
                secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[83; 32])
                    .unwrap()
                    .x_only_public_key()
                    .0,
            ),
            source_seat_id: SeatId::from(QuoteId([200 + index; 32])),
            destination_service_pubkey: identity.derive_service_pubkey(),
            destination_seat_id: facts.seat_id,
        });
    }
    let fleet = Fleet::open(db, config, Arc::new(NoWallet)).await.unwrap();
    for grant in grants {
        fleet
            .seat_by_id(&grant.destination_seat_id)
            .unwrap()
            .admit_transfer(
                SignedRequest::create(&grant, &fa).unwrap(),
                identity.derive_service_pubkey(),
                None,
            )
            .await
            .unwrap();
    }
    let mut outstanding = Vec::new();
    for _ in 0..4 {
        outstanding.push(
            tokio::time::timeout(Duration::from_secs(5), calls.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), calls.recv())
            .await
            .is_err(),
        "fifth source attempt must wait for a permit"
    );
    tokio::time::timeout(Duration::from_secs(5), fleet.shutdown())
        .await
        .unwrap();
    for mut call in outstanding {
        tokio::time::timeout(Duration::from_secs(1), call.reply.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn import_requires_exact_backup_and_configured_child_before_completion() {
    let d = destination(32_137).await;
    let record = d.admit().await;
    assert_eq!(d.spawner.spawn_count(), 1);
    d.sink.fails.store(true, Ordering::SeqCst);
    let publication = d
        .seat
        .install_transfer(
            record.generation,
            record.binding.clone(),
            d.validate().await,
        )
        .await
        .unwrap();
    assert!(matches!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        IncomingState::Importing { .. }
    ));
    d.assert_pending().await;
    assert_eq!(
        d.spawner.spawn_count(),
        1,
        "install alone cannot start child"
    );
    assert!(
        d.seat
            .complete_transfer(record.generation, d.publication().await)
            .await
            .is_err()
    );
    assert_eq!(
        d.spawner.spawn_count(),
        1,
        "backup failure cannot start child"
    );
    let data = seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no);
    assert_eq!(
        tokio::fs::read_to_string(data.join("password.private"))
            .await
            .unwrap(),
        d.material.api_auth
    );
    d.sink.fails.store(false, Ordering::SeqCst);
    d.fleet
        .backup
        .confirm_transfer_publication(&d.fleet.db, &publication, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        d.seat
            .complete_transfer(record.generation, publication)
            .await
            .is_err()
    );
    assert_eq!(
        d.spawner.request_count(),
        0,
        "NeedsParams is not permission to re-DKG"
    );
    assert_eq!(d.spawner.spawn_count(), 2);
    d.assert_pending().await;
    // Existing auxiliary databases from a prior start do not invalidate adoption.
    tokio::fs::write(data.join("consensus.db"), b"retained")
        .await
        .unwrap();
    let publication = d
        .seat
        .install_transfer(record.generation, record.binding, d.validate().await)
        .await
        .unwrap();
    d.fleet
        .backup
        .confirm_transfer_publication(&d.fleet.db, &publication, Duration::from_secs(5))
        .await
        .unwrap();
    d.seat
        .complete_transfer(record.generation, publication)
        .await
        .unwrap();
    assert_eq!(d.spawner.spawn_count(), 3);
    let completed = d
        .fleet
        .db
        .incoming_transfer(&d.grant.destination_seat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.phase(), TransferPhase::Imported);
    d.admit().await;
    assert_eq!(
        d.spawner.spawn_count(),
        3,
        "same-move replay keeps imported child"
    );
    assert_eq!(
        tokio::fs::read(data.join("consensus.db")).await.unwrap(),
        b"retained"
    );
}

#[tokio::test]
async fn import_owner_rejects_wrong_local_identity_and_stale_generation() {
    let d = destination(32_147).await;
    let signed = SignedRequest::create(&d.grant, &d.fa).unwrap();
    assert!(
        d.seat
            .admit_transfer(signed, d.fa.x_only_public_key().0, None)
            .await
            .is_err()
    );
    assert!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .is_none()
    );
    let record = d.admit().await;
    assert!(
        d.seat
            .install_transfer(
                record.generation + 1,
                record.binding.clone(),
                d.validate().await,
            )
            .await
            .is_err()
    );
    let mut wrong = TransferMove::from(&d.grant);
    wrong.source_seat_id = SeatId::from(QuoteId([183; 32]));
    assert!(
        d.seat
            .install_transfer(record.generation, wrong, d.validate().await)
            .await
            .is_err()
    );
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Waiting
    );
    assert!(!seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no).exists());
    assert_eq!(d.spawner.spawn_count(), 1);
}

#[tokio::test]
async fn adoption_finishes_every_sync_before_recording_formed_or_imported() {
    let d = destination(32_169).await;
    let record = d.admit().await;
    let guardian = d.validate().await;
    assert!(
        d.fleet
            .db
            .accept_transfer(
                &d.grant.destination_seat_id,
                record.generation,
                &record.binding,
                &d.material,
                &guardian.invite,
            )
            .await
            .unwrap()
    );
    d.admit().await; // Rebuild the runtime's Importing fence from durable state.
    let process = &d.fleet.config.process;
    let parent = crate::seat_process::seat_dir(process, d.seat.facts().seat_no);
    let staged = parent.join("interrupted-install");
    tokio::fs::create_dir_all(&staged).await.unwrap();
    // Equivalent to process death after rename but before fsync and formed
    // commit. These writes deliberately do not use the durable restore writer.
    let files = [
        (
            "private.encrypt",
            d.material.archive.private_encrypt.as_str(),
        ),
        ("private.salt", d.material.archive.private_salt.as_str()),
        ("local.json", d.material.archive.local_json.as_str()),
        ("consensus.json", d.material.archive.consensus_json.as_str()),
        ("password.private", d.material.api_auth.as_str()),
    ];
    for (name, bytes) in files {
        tokio::fs::write(staged.join(name), bytes).await.unwrap();
    }
    let data = seat_data_dir(process, d.seat.facts().seat_no);
    tokio::fs::rename(staged, &data).await.unwrap();
    let paths = files.iter().map(|(name, _)| data.join(name)).chain([
        data.clone(),
        parent,
        process.data_root.join("seats"),
        process.data_root.clone(),
    ]);
    for path in paths {
        let fault = crate::seat::import_sync_fault::at(path);
        assert!(
            d.seat
                .install_transfer(
                    record.generation,
                    record.binding.clone(),
                    d.validate().await,
                )
                .await
                .is_err(),
            "every file and directory sync must finish before adoption"
        );
        drop(fault);
        assert!(
            d.fleet
                .db
                .formed_federation_invite(&d.grant.destination_seat_id)
                .await
                .unwrap()
                .is_none(),
        );
        assert_eq!(
            d.fleet
                .db
                .incoming_transfer(&d.grant.destination_seat_id)
                .await
                .unwrap()
                .unwrap()
                .phase(),
            TransferPhase::Importing,
        );
        assert_eq!(d.spawner.spawn_count(), 1);
    }
    let publication = d
        .seat
        .install_transfer(record.generation, record.binding, d.validate().await)
        .await
        .unwrap();
    for (name, bytes) in files {
        assert_eq!(
            tokio::fs::read_to_string(data.join(name)).await.unwrap(),
            bytes
        );
    }
    d.fleet
        .backup
        .confirm_transfer_publication(&d.fleet.db, &publication, Duration::from_secs(5))
        .await
        .unwrap();
    // This fixture's first child wrongly says NeedsParams; only its second
    // exact AlreadyConfigured hello may complete the accepted import.
    assert!(
        d.seat
            .complete_transfer(record.generation, publication)
            .await
            .is_err()
    );
    d.seat
        .complete_transfer(record.generation, d.publication().await)
        .await
        .unwrap();
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Imported,
    );
}

#[tokio::test]
async fn accepted_import_does_not_overwrite_conflicting_installed_identity() {
    let d = destination(32_157).await;
    let record = d.admit().await;
    let data = seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no);
    tokio::fs::create_dir_all(&data).await.unwrap();
    tokio::fs::write(data.join("private.encrypt"), b"conflict")
        .await
        .unwrap();
    assert!(
        d.seat
            .install_transfer(record.generation, record.binding, d.validate().await,)
            .await
            .is_err()
    );
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Importing
    );
    assert_eq!(
        tokio::fs::read(data.join("private.encrypt")).await.unwrap(),
        b"conflict"
    );
    assert_eq!(d.spawner.spawn_count(), 1);
}

#[tokio::test]
async fn accepted_import_completes_after_grant_expiry_but_waiting_renewal_fences_old_work() {
    let mut d = destination(32_167).await;
    let validated = d.validate().await;
    d.grant.ts = Timestamp((crate::db::now_ms() / 1000) as u64);
    d.grant.expires_at = Timestamp(d.grant.ts.0 + 2);
    let first = d.admit().await;
    tokio::time::sleep(Duration::from_millis(
        ((d.grant.expires_at.0 + 1) * 1000).saturating_sub(crate::db::now_ms() as u64),
    ))
    .await;
    d.seat.expire_transfer(first.generation).await.unwrap();
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Expired
    );
    let renewed_guardian = d.validate().await;
    d.grant.ts = Timestamp((crate::db::now_ms() / 1000) as u64);
    d.grant.expires_at = Timestamp(d.grant.ts.0 + 2);
    let renewed = d.admit().await;
    assert!(first.generation < renewed.generation);
    assert!(
        d.seat
            .install_transfer(first.generation, first.binding, validated,)
            .await
            .is_err()
    );
    let expected = d
        .seat
        .install_transfer(renewed.generation, renewed.binding, renewed_guardian)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(
        ((d.grant.expires_at.0 + 1) * 1000).saturating_sub(crate::db::now_ms() as u64),
    ))
    .await;
    d.seat.expire_transfer(renewed.generation).await.unwrap();
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Importing
    );
    d.fleet
        .backup
        .confirm_transfer_publication(&d.fleet.db, &expected, Duration::from_secs(5))
        .await
        .unwrap();
    // Scripted first child wrongly says NeedsParams, the second confirms config.
    assert!(
        d.seat
            .complete_transfer(renewed.generation, expected)
            .await
            .is_err()
    );
    d.seat
        .complete_transfer(renewed.generation, d.publication().await)
        .await
        .unwrap();
    assert_eq!(
        d.fleet
            .db
            .incoming_transfer(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .unwrap()
            .phase(),
        TransferPhase::Imported
    );
}
