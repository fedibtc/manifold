use super::*;
use crate::transfer_native::{ValidatedGuardian, validate_material};
use crate::transfer_state::{IncomingState, TransferMove, TransferPhase};
use fedi_decentralized_service_manifold_guardian::{
    Locator, MgSignedRequest, SeatTransferGrant, SignedRequest, Timestamp, TransferSeatRequest,
};

struct SourceCall {
    request: TransferSeatRequest,
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
        request: MgSignedRequest<TransferSeatRequest>,
    ) -> Result<
        fedi_decentralized_service_manifold_guardian::FmResult<
            fedi_decentralized_service_manifold_guardian::TransferSeatResponse,
        >,
        crate::transfer_source::TransferSourceUnavailable,
    > {
        let now = Timestamp((crate::db::now_ms() / 1000) as u64);
        let request = request.verify(now).unwrap();
        let verified = request.grant.verify(now).unwrap();
        assert_eq!(source, &verified.source);
        assert_eq!(
            verified.destination_service_pubkey,
            request.destination_service_pubkey
        );
        let (reply, done) = tokio::sync::oneshot::channel();
        self.0
            .send(SourceCall { request, reply })
            .map_err(|_| crate::transfer_source::TransferSourceUnavailable)?;
        done.await
            .map(Ok)
            .map_err(|_| crate::transfer_source::TransferSourceUnavailable)
    }
}

fn respond(d: &Destination, call: SourceCall) {
    call.reply
        .send(
            fedi_decentralized_service_manifold_guardian::TransferSeatResponse {
                material: d.material.clone().into_wire(),
            },
        )
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

async fn wait_spawns(spawner: &FakeSeatProcessSpawner, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while spawner.spawn_count() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {count} spawns, saw {}", spawner.spawn_count()));
    assert_eq!(spawner.spawn_count(), count);
}

struct Destination {
    _root: TempDir,
    fleet: Arc<Fleet>,
    seat: Arc<Seat>,
    sink: Arc<FakeBackupSink>,
    spawner: Arc<FakeSeatProcessSpawner>,
    grant: SeatTransferGrant,
    fa: secp256k1::Keypair,
    material: crate::transfer_native::TransferMaterial,
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
            &self.fleet.config.process,
            self.material.clone(),
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
    destination_in(
        TempDir::new().unwrap(),
        port,
        source,
        validator,
        fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
        vec![FakeDkgStep::Hang],
    )
    .await
}

async fn destination_in(
    root: TempDir,
    port: u16,
    source: Arc<dyn crate::transfer_source::TransferSource>,
    validator: Arc<dyn crate::transfer_native::GuardianConfigValidator>,
    environment: fedi_decentralized_manifold_environment::ManifoldEnvironment,
    first_child: Vec<FakeDkgStep>,
) -> Destination {
    let mut config = config(&root, 1, port).await;
    config.manifold_environment = environment;
    config.guardian_config_validator = validator;
    config.transfer_source = source;
    // The scripted first imported child wrongly says NeedsParams; the respawn
    // that replaces it must not stretch the tests.
    config.respawn.initial_backoff = Duration::from_millis(10);
    config.respawn.max_backoff = Duration::from_millis(20);
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
        first_child,
        // First import start falsely claims config absent. It must never DKG,
        // and only the respawn's exact AlreadyConfigured hello runs the import.
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
    Destination {
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
    let d = destination_with_capabilities(
        32_145,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    let rpc = crate::service::ManifoldGuardianRpc::new(d.fleet.clone(), transfer_policy());
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
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.request.grant, request.grant);
    respond(&d, call);
    wait_phase(&d.seat, TransferPhase::Imported).await;
    // The wrong configured hello was replaced by respawn, never used for DKG.
    wait_spawns(&d.spawner, 3).await;
    assert_eq!(d.spawner.request_count(), 0);
    assert!(
        calls.try_recv().is_err(),
        "an accepted import never asks the source again"
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
async fn a_grant_signed_again_for_the_same_move_keeps_the_outstanding_rpc() {
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let mut d = destination_with_capabilities(
        32_149,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    let first = d.admit().await;
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    // A restored FA signs the move again. The seat keeps its admission, so
    // the source call in flight still installs the guardian.
    d.grant.ts = Timestamp(d.grant.ts.0 + 1);
    let again = d.admit().await;
    assert_eq!(again.binding, first.binding);
    respond(&d, call);
    wait_phase(&d.seat, TransferPhase::Imported).await;
    assert!(calls.try_recv().is_err());
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
    let d = destination_with_capabilities(
        32_153,
        Arc::new(ControlledSource(source)),
        validator.clone(),
    )
    .await;
    d.admit().await;
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
}

#[tokio::test]
async fn worker_reopens_accepted_import_without_source_authority() {
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let d = destination_with_capabilities(
        32_157,
        Arc::new(ControlledSource(source)),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
    )
    .await;
    let guardian = d.validate().await;
    let record = d.admit().await;
    d.seat
        .install_transfer(record.binding, guardian)
        .await
        .unwrap();
    d.fleet.shutdown().await;
    // The worker may have asked the source before the manual install won.
    while calls.try_recv().is_ok() {}
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

/// Installation is the whole import: acceptance, the formed row and the child
/// start happen in one command, and the backup follows asynchronously as it
/// does for a natively formed seat. A failing relay delays nothing.
#[tokio::test]
async fn install_accepts_and_starts_the_imported_guardian_without_a_backup_barrier() {
    let d = destination(32_137).await;
    let record = d.admit().await;
    assert_eq!(d.spawner.spawn_count(), 1);
    d.sink.fails.store(true, Ordering::SeqCst);
    d.seat
        .install_transfer(record.binding.clone(), d.validate().await)
        .await
        .unwrap();
    let accepted = d
        .fleet
        .db
        .incoming_transfer(&d.grant.destination_seat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.phase(), TransferPhase::Imported);
    assert!(
        d.fleet
            .db
            .formed_federation_invite(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .is_some()
    );
    let data = seat_data_dir(&d.fleet.config.process, d.seat.facts().seat_no);
    assert_eq!(
        tokio::fs::read_to_string(data.join("private.json"))
            .await
            .unwrap(),
        d.material.archive.private_json
    );
    // The scripted first imported child wrongly says NeedsParams; it is
    // stopped and replaced, never fed DKG parameters.
    wait_spawns(&d.spawner, 3).await;
    assert_eq!(
        d.spawner.request_count(),
        0,
        "NeedsParams is not permission to re-DKG"
    );
    assert!(matches!(
        d.seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running { .. },
            ..
        }
    ));
    assert!(
        d.seat
            .install_transfer(record.binding, d.validate().await)
            .await
            .is_err(),
        "an accepted import is never installed twice"
    );
    d.admit().await;
    assert_eq!(
        d.spawner.spawn_count(),
        3,
        "same-move replay keeps imported child"
    );
    d.sink.fails.store(false, Ordering::SeqCst);
    let expected = d.publication().await.doc_sha256();
    tokio::time::timeout(Duration::from_secs(5), async {
        while d
            .fleet
            .db
            .backup_publication(
                &d.grant.destination_seat_id,
                d.fleet.backup.format_version(),
            )
            .await
            .unwrap()
            .is_none_or(|record| record.doc_sha256 != expected)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the imported seat's backup follows once the relay recovers");
    d.fleet.shutdown().await;
}

#[tokio::test]
async fn import_owner_rejects_wrong_local_identity_and_another_move() {
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
    d.admit().await;
    let mut wrong = TransferMove::from(&d.grant);
    wrong.source_seat_id = SeatId::from(QuoteId([183; 32]));
    assert!(
        d.seat
            .install_transfer(wrong, d.validate().await)
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

/// A crash between the staging rename and the acceptance commit leaves the
/// complete directory and a `waiting` row. The retry finds its own archive
/// there and completes without touching the files.
#[tokio::test]
async fn install_accepts_the_directory_a_crashed_attempt_left_in_place() {
    let d = destination(32_169).await;
    let record = d.admit().await;
    let process = &d.fleet.config.process;
    let data = seat_data_dir(process, d.seat.facts().seat_no);
    crate::backup::write_restored_seat_dir(
        &data,
        &d.material.archive,
        &d.material.archive.digest(),
    )
    .await
    .unwrap();
    // Stale staging debris from that attempt is disposable.
    let staging =
        crate::seat_process::seat_dir(process, d.seat.facts().seat_no).join("data.staging");
    tokio::fs::create_dir_all(&staging).await.unwrap();
    tokio::fs::write(staging.join("private.json"), "half")
        .await
        .unwrap();
    d.seat
        .install_transfer(record.binding, d.validate().await)
        .await
        .unwrap();
    assert!(!staging.exists());
    assert!(
        d.fleet
            .db
            .formed_federation_invite(&d.grant.destination_seat_id)
            .await
            .unwrap()
            .is_some(),
    );
    for (name, bytes) in [
        ("private.json", d.material.archive.private_json.as_str()),
        ("local.json", d.material.archive.local_json.as_str()),
        ("consensus.json", d.material.archive.consensus_json.as_str()),
    ] {
        assert_eq!(
            tokio::fs::read_to_string(data.join(name)).await.unwrap(),
            bytes
        );
    }
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
    tokio::fs::write(data.join("private.json"), b"conflict")
        .await
        .unwrap();
    assert!(
        d.seat
            .install_transfer(record.binding, d.validate().await,)
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
    assert_eq!(
        tokio::fs::read(data.join("private.json")).await.unwrap(),
        b"conflict"
    );
    assert_eq!(d.spawner.spawn_count(), 1);
}

/// The setup-payment policy gates only the start of a transfer. A production
/// MG that holds no policy that enables transfer still resumes a transfer it
/// stored: across a restart, its worker asks the source again.
#[tokio::test]
async fn a_stored_incoming_transfer_resumes_without_a_policy_that_enables_transfer() {
    use fedi_decentralized_manifold_environment::ManifoldEnvironment;
    let (source, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let source: Arc<dyn crate::transfer_source::TransferSource> =
        Arc::new(ControlledSource(source));
    let d = destination_in(
        TempDir::new().unwrap(),
        32_171,
        source.clone(),
        Arc::new(crate::transfer_native::tests::CoreConfigValidator),
        ManifoldEnvironment::Production,
        vec![FakeDkgStep::Hang],
    )
    .await;
    assert!(d.fleet.transfers.is_running_for_test());
    // The seat owner stores the transfer as an admitted pull would.
    d.admit().await;
    let seat_id = d.grant.destination_seat_id.clone();
    let Destination {
        _root: root, fleet, ..
    } = d;
    fleet.shutdown().await;
    drop(fleet);
    while calls.try_recv().is_ok() {}

    let mut config = config(&root, 1, 32_171).await;
    config.manifold_environment = ManifoldEnvironment::Production;
    config.transfer_source = source;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    assert!(fleet.transfers.is_running_for_test());
    assert!(matches!(
        fleet
            .db
            .incoming_transfer(&seat_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        IncomingState::Waiting { .. }
    ));
    tokio::time::timeout(Duration::from_secs(10), calls.recv())
        .await
        .expect("the reopened MG asks its source again")
        .expect("source channel open");
    fleet.shutdown().await;
}
