use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationSize, InviteCode, Plan, QuoteId,
};
use tempfile::TempDir;

use super::*;
use crate::backup::{GuardianArchive, GuardianArchiveRef, PaymentBackup, SeatBackupDocument};
use crate::facts::{SeatFacts, SeatNo};
use crate::seat_process::BitcoindConfig;

const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A `consensus.json` bigger than the storage adapter's single-event budget:
/// the real file runs to a hundred kilobytes, and a restore of it exercises
/// the reassembled-archive path end to end.
fn big_consensus_json() -> String {
    format!(r#"{{"api_endpoints":"{}"}}"#, "a".repeat(60 * 1024))
}

fn test_identity() -> RootMnemonic {
    RootMnemonic::parse(TEST_MNEMONIC).unwrap()
}

/// A seat's creation facts. Its seat number stays the default: a backup
/// document does not carry it.
fn facts(seat: u8) -> SeatFacts {
    let mut seed = [0_u8; 32];
    seed[0] = 7;
    let fa_id = FaId(
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
            .unwrap()
            .x_only_public_key()
            .0,
    );
    SeatFacts {
        seat_id: SeatId::from(QuoteId([seat; 32])),
        seat_no: SeatNo(0),
        fa_id,
        plan: Plan::InfiniteBestEffort { price_msats: 1_000 },
        federation_size: FederationSize(7),
        created_at_ms: 1_700_000_000_000,
    }
}

fn guardian_archive(consensus_json: &str) -> GuardianArchive {
    GuardianArchive {
        private_json: r#"{"shares":"deadbeef"}"#.into(),
        local_json: r#"{"api_bind":"127.0.0.1:1"}"#.into(),
        consensus_json: consensus_json.to_owned(),
    }
}

fn guardian_ref(archive: &GuardianArchive) -> GuardianArchiveRef {
    GuardianArchiveRef {
        archive_sha256: archive.digest(),
        federation_invite: Some(InviteCode("fed11-restored".into())),
    }
}

fn payment() -> PaymentBackup {
    PaymentBackup {
        evidence: crate::wallet::EcashClaimEvidence::test(1),
    }
}

/// What a [`crate::backup::BackupArchive`] hands back: the decode side is the
/// adapter's and is tested with it (`mg-nostr`); these tests exercise what
/// the install does with the result.
fn recovered(
    seats: Vec<SeatBackupDocument>,
    archives: Vec<(SeatId, GuardianArchive)>,
) -> RecoveredFleet {
    RecoveredFleet {
        seats,
        archives: archives.into_iter().collect(),
    }
}

fn process(temp: &TempDir) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: temp.path().join("fedimintd"),
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind {
            primary: BitcoindConfig {
                url: "http://127.0.0.1:18443".to_owned(),
                username: "user".to_owned(),
                password: "pass".to_owned(),
            },
            esplora_fallback: None,
        },
    }
}

async fn test_db(temp: &TempDir) -> Db {
    Db::open(temp.path()).await.unwrap()
}

fn transferred_document(seat: u8) -> (SeatBackupDocument, GuardianArchive) {
    use crate::backup::{IncomingTransferBackup, TransferBackup};
    let identity = test_identity();
    let facts = facts(seat);
    let archive = guardian_archive("{}");
    let federation_id = "ab".repeat(32);
    let invite = fedimint_core::invite_code::InviteCode::new(
        "ws://127.0.0.1:8174".parse().unwrap(),
        fedimint_core::PeerId::from(0),
        federation_id.parse().unwrap(),
        None,
    );
    let mut document = SeatBackupDocument::new(
        &facts,
        Some(payment()),
        Some(GuardianArchiveRef {
            archive_sha256: archive.digest(),
            federation_invite: Some(InviteCode(invite.to_string())),
        }),
        None,
    );
    document.transfer = Some(TransferBackup {
        incoming: Some(IncomingTransferBackup::Accepted {
            binding: crate::transfer_state::TransferMove {
                fa_id: facts.fa_id,
                federation_id: fedi_decentralized_service_manifold_guardian::FederationId(
                    federation_id,
                ),
                source: fedi_decentralized_service_manifold_guardian::Locator::new(
                    fedi_iroh_rpc::iroh::EndpointAddr::new(
                        fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[5; 32]).public(),
                    ),
                    identity.derive_service_pubkey(),
                ),
                source_seat_id: SeatId::from(QuoteId([99; 32])),
                destination_service_pubkey: identity.derive_service_pubkey(),
                destination_seat_id: facts.seat_id,
            },
            iroh_api_sk: [71; 32],
            iroh_p2p_sk: [72; 32],
            callback_commitment: None,
        }),
        outgoing: None,
    });
    (document, archive)
}

#[tokio::test]
async fn transfer_recovery_preserves_callback_choice_and_matching_fi_retry_restores_bearer() {
    use crate::backup::IncomingTransferBackup;
    use fedi_decentralized_service_manifold_guardian::{
        DkgCompletionCallback, DkgCompletionCallbackInput, SeatTransferGrant, SignedRequest,
        Timestamp,
    };
    {
        let (document, archive) = transferred_document(4);
        let Some(IncomingTransferBackup::Accepted { binding, .. }) =
            document.transfer.as_ref().unwrap().incoming.as_ref()
        else {
            unreachable!()
        };
        let now = (crate::db::now_ms() / 1000) as u64;
        let grant = SeatTransferGrant {
            ts: Timestamp(now),
            fa_id: binding.fa_id,
            federation_id: binding.federation_id.clone(),
            source: binding.source.clone(),
            source_seat_id: binding.source_seat_id.clone(),
            destination_service_pubkey: binding.destination_service_pubkey,
            destination_seat_id: binding.destination_seat_id.clone(),
        };
        let mut seed = [0; 32];
        seed[0] = 7;
        let key = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed).unwrap();
        let signed = SignedRequest::create(&grant, &key).unwrap();
        let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
            callback_url: "https://push.example/hooks/dkg/opaque-bearer".into(),
            idempotency_key: "transfer-complete".into(),
        })
        .unwrap();
        let original = TempDir::new().unwrap();
        let db = test_db(&original).await;
        let facts = crate::test_support::insert_test_seat(
            &db,
            crate::db::NewSeat {
                seat_id: document.seat_id.clone(),
                fa_id: document.fa_id,
                plan: document.plan.clone(),
                federation_size: document.federation_size,
                payment: None,
            },
        )
        .await;
        let admitted = db
            .admit_transfer(&signed, Some(&callback))
            .await
            .unwrap()
            .unwrap();
        crate::backup::write_restored_seat_dir(
            &crate::seat_process::seat_data_dir(&process(&original), facts.seat_no),
            &archive,
            &archive.digest(),
        )
        .await
        .unwrap();
        assert!(
            db.accept_transfer(
                &facts.seat_id,
                &admitted.binding,
                &crate::transfer_state::ImportedKeys::from_bytes(&[71; 32], &[72; 32]),
                document
                    .guardian
                    .as_ref()
                    .unwrap()
                    .federation_invite
                    .as_ref()
                    .unwrap(),
            )
            .await
            .unwrap()
        );
        let plan = crate::backup::seat_publication_plan(&db, &process(&original), &facts, None)
            .await
            .unwrap();
        let archives = plan
            .archive
            .map(|archive| (facts.seat_id.clone(), archive))
            .into_iter()
            .collect();
        let fleet = recovered(vec![plan.document], archives);
        let restored = TempDir::new().unwrap();
        let db = test_db(&restored).await;
        install(&db, &process(&restored), &test_identity(), &fleet)
            .await
            .unwrap();
        assert!(
            db.completion_callback(&facts.seat_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(db.admit_transfer(&signed, None).await.unwrap().is_none());
        assert!(
            db.admit_transfer(&signed, Some(&callback))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            db.completion_callback(&facts.seat_id)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            db.incoming_transfer(&facts.seat_id)
                .await
                .unwrap()
                .unwrap()
                .callback_commitment,
            admitted.callback_commitment
        );
    }
}

#[tokio::test]
async fn accepted_import_restores_its_iroh_identity_and_remains_fenced() {
    let (document, archive) = transferred_document(1);
    assert!(format!("{document:?}").contains("IncomingTransferBackup(redacted)"));
    let recovered = recovered(
        vec![document.clone()],
        vec![(document.seat_id.clone(), archive)],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &test_identity(), &recovered)
        .await
        .unwrap();
    let imported = db
        .incoming_transfer(&document.seat_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        imported
            .accepted()
            .unwrap()
            .same_as(&crate::transfer_state::ImportedKeys::from_bytes(
                &[71; 32], &[72; 32]
            )),
        "the imported Iroh identity is restored from the document"
    );
    assert_eq!(
        db.transfer_phase(&document.seat_id).await.unwrap(),
        Some(crate::transfer_state::TransferPhase::Imported)
    );
    let facts = db.list_seats().await.unwrap().remove(0).facts;
    let publication = crate::backup::seat_publication_plan(&db, &process, &facts, None)
        .await
        .unwrap();
    assert_eq!(publication.document, document);
    assert_eq!(
        publication.archive.unwrap(),
        recovered.archives[&facts.seat_id]
    );
}

#[tokio::test]
async fn removed_source_restores_formed_accounting_without_guardian_material() {
    use crate::backup::{OutgoingTransferBackup, TransferBackup};
    let (mut document, _) = transferred_document(3);
    let invite = document.guardian.take().unwrap().federation_invite.unwrap();
    document.transfer = Some(TransferBackup {
        incoming: None,
        outgoing: Some(OutgoingTransferBackup {
            phase: crate::transfer_state::OutgoingTransfer::Removed,
            federation_invite: invite.clone(),
        }),
    });
    let recovered = recovered(vec![document.clone()], vec![]);
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &test_identity(), &recovered)
        .await
        .unwrap();
    assert_eq!(
        db.outgoing_transfer(&document.seat_id).await.unwrap(),
        Some(crate::transfer_state::OutgoingTransfer::Removed)
    );
    assert_eq!(
        db.formed_federation_invite(&document.seat_id)
            .await
            .unwrap(),
        Some(invite)
    );
    assert!(db.payment(&document.seat_id).await.unwrap().is_some());
    assert!(!crate::seat_process::seat_data_dir(&process, SeatNo(0)).exists());
    let facts = db.list_seats().await.unwrap().remove(0).facts;
    let plan =
        crate::backup::seat_publication_plan(&db, &process, &facts, Some("obsolete-archive"))
            .await
            .unwrap();
    assert_eq!(plan.document, document);
    assert!(plan.archive.is_none());
}

/// A source that stopped its guardian for a transfer still holds the config,
/// so its backup carries the archive; the restore keeps the seat fenced.
#[tokio::test]
async fn stopped_source_restores_its_archive_and_stays_fenced() {
    use crate::backup::{OutgoingTransferBackup, TransferBackup};
    let (mut document, archive) = transferred_document(4);
    let invite = document
        .guardian
        .as_ref()
        .and_then(|guardian| guardian.federation_invite.clone())
        .unwrap();
    document.transfer = Some(TransferBackup {
        incoming: None,
        outgoing: Some(OutgoingTransferBackup {
            phase: crate::transfer_state::OutgoingTransfer::Stopped,
            federation_invite: invite.clone(),
        }),
    });
    let outgoing = &document.transfer.as_ref().unwrap().outgoing;
    assert_eq!(
        serde_json::to_value(outgoing).unwrap()["phase"],
        "stopped",
        "the backup carries the phase the database stores"
    );
    let recovered = recovered(
        vec![document.clone()],
        vec![(document.seat_id.clone(), archive)],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &test_identity(), &recovered)
        .await
        .unwrap();
    assert_eq!(
        db.transfer_phase(&document.seat_id).await.unwrap(),
        Some(crate::transfer_state::TransferPhase::Outgoing(
            crate::transfer_state::OutgoingTransfer::Stopped
        )),
        "the restored source never starts the moved guardian"
    );
    assert_eq!(
        db.formed_federation_invite(&document.seat_id)
            .await
            .unwrap(),
        Some(invite)
    );
    let facts = db.list_seats().await.unwrap().remove(0).facts;
    let plan = crate::backup::seat_publication_plan(&db, &process, &facts, None)
        .await
        .unwrap();
    assert_eq!(plan.document, document);
    assert_eq!(plan.archive.unwrap(), recovered.archives[&facts.seat_id]);
}

/// The whole capability, end to end: a recovered backup produces a fleet with
/// its guardian configs on disk and its facts in the database.
#[tokio::test]
async fn a_recovered_backup_rebuilds_the_fleet() {
    let identity = test_identity();
    let consensus_json = big_consensus_json();

    let archive = guardian_archive(&consensus_json);
    let formed = SeatBackupDocument::new(
        &facts(1),
        Some(payment()),
        Some(guardian_ref(&archive)),
        None,
    );
    let unformed = SeatBackupDocument::new(&facts(2), Some(payment()), None, None);

    let recovered = recovered(
        vec![formed.clone(), unformed.clone()],
        vec![(formed.seat_id.clone(), archive)],
    );
    assert_eq!(recovered.formed(), 1);

    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &identity, &recovered).await.unwrap();

    // The install now *is* the MG the backup described.
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );

    // The formed seat's guardian is on disk, byte for byte, with the consensus
    // config the backup carried.
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    assert_eq!(
        tokio::fs::read_to_string(dir.join("private.json"))
            .await
            .unwrap(),
        r#"{"shares":"deadbeef"}"#
    );
    assert_eq!(
        tokio::fs::read_to_string(dir.join("consensus.json"))
            .await
            .unwrap(),
        consensus_json
    );

    // And it refuses RestartDKG, which is the guard the backup carries no
    // ceremony state to re-establish: the config's presence is the fact.
    assert!(
        db.formed_federation_invite(&formed.seat_id)
            .await
            .unwrap()
            .is_some(),
        "a restored guardian must not be wipeable by a later RestartDKG"
    );
    assert!(
        db.formed_federation_invite(&unformed.seat_id)
            .await
            .unwrap()
            .is_none(),
        "a paid-but-unformed seat is meant to run the ceremony"
    );

    // Seat numbers are assigned in creation order; the seat id breaks the tie.
    let seats = db.list_seats().await.unwrap();
    let mut numbers: Vec<u32> = seats.iter().map(|seat| seat.facts.seat_no.0).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, vec![0, 1]);

    // The money came back with them.
    assert!(db.payment(&formed.seat_id).await.unwrap().is_some());
}

/// Two hosts that ran one mnemonic at once numbered their new seats
/// independently, so a restore of both of their seats must not trust either
/// host's numbers. The restoring host numbers the whole fleet in creation
/// order, whatever order the relay returned the documents in.
#[tokio::test]
async fn seats_from_two_diverged_hosts_restore_side_by_side() {
    let identity = test_identity();
    let first_archive = guardian_archive(r#"{"api_endpoints":"first"}"#);
    let second_archive = guardian_archive(r#"{"api_endpoints":"second"}"#);
    let created = |seat, created_at_ms| SeatFacts {
        created_at_ms,
        ..facts(seat)
    };
    // The relay order is not the creation order, and the seat ids sort
    // against the creation order too.
    let host_b_seat = SeatBackupDocument::new(
        &created(1, 1_700_000_000_300),
        None,
        Some(guardian_ref(&second_archive)),
        None,
    );
    let unformed = SeatBackupDocument::new(&created(2, 1_700_000_000_200), None, None, None);
    let host_a_seat = SeatBackupDocument::new(
        &created(3, 1_700_000_000_100),
        None,
        Some(guardian_ref(&first_archive)),
        None,
    );
    let recovered = recovered(
        vec![host_b_seat.clone(), unformed.clone(), host_a_seat.clone()],
        vec![
            (host_b_seat.seat_id.clone(), second_archive),
            (host_a_seat.seat_id.clone(), first_archive),
        ],
    );

    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &identity, &recovered).await.unwrap();

    let mut numbers: Vec<(SeatId, u32)> = db
        .list_seats()
        .await
        .unwrap()
        .into_iter()
        .map(|seat| (seat.facts.seat_id, seat.facts.seat_no.0))
        .collect();
    numbers.sort();
    let mut expected = vec![
        (host_a_seat.seat_id.clone(), 0),
        (unformed.seat_id.clone(), 1),
        (host_b_seat.seat_id.clone(), 2),
    ];
    expected.sort();
    assert_eq!(numbers, expected);
    for (seat_no, consensus_json) in [
        (0, r#"{"api_endpoints":"first"}"#),
        (2, r#"{"api_endpoints":"second"}"#),
    ] {
        let dir = crate::seat_process::seat_data_dir(&process, SeatNo(seat_no));
        assert_eq!(
            tokio::fs::read_to_string(dir.join("consensus.json"))
                .await
                .unwrap(),
            consensus_json
        );
    }
    assert!(
        !tokio::fs::try_exists(crate::seat_process::seat_data_dir(&process, SeatNo(1)))
            .await
            .unwrap(),
        "an unformed seat gets a number but no guardian directory"
    );
}

/// The onboarding boundary: an install that already has an identity refuses.
/// Nothing mints one implicitly, so an identity row means an operator onboarded
/// this host, and burying that is never what they meant.
#[tokio::test]
async fn an_install_that_has_been_onboarded_refuses_to_be_restored_into() {
    let identity = test_identity();
    let temp = TempDir::new().unwrap();
    let db = test_db(&temp).await;
    let already = RootMnemonic::generate().unwrap();
    db.install_identity(&already).await.unwrap();

    let recovered = recovered(vec![], vec![]);
    assert!(matches!(
        install(&db, &process(&temp), &identity, &recovered).await,
        Err(RestoreError::AlreadyOnboarded)
    ));
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        already.phrase(),
        "a refused restore leaves the install exactly as it was"
    );
}

/// Restore creates seat directories; it never writes into one. It therefore
/// adds no deletion path relevant to
/// `CLAIM-manifold-guardian-preserves-published-guardian-data`.
#[tokio::test]
async fn a_restore_never_writes_into_an_existing_seat_directory() {
    let identity = test_identity();
    let temp = TempDir::new().unwrap();
    let process = process(&temp);

    let seat = SeatBackupDocument::new(&facts(3), None, None, None);
    let occupied = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    tokio::fs::create_dir_all(&occupied).await.unwrap();
    tokio::fs::write(occupied.join("private.encrypt"), "a live guardian")
        .await
        .unwrap();

    let recovered = recovered(vec![seat], vec![]);
    let db = test_db(&temp).await;
    assert!(matches!(
        install(&db, &process, &identity, &recovered).await,
        Err(RestoreError::SeatDirectoryExists(_))
    ));

    // Refused before anything was written: the live guardian is untouched and
    // the install is still un-onboarded.
    assert_eq!(
        tokio::fs::read_to_string(occupied.join("private.encrypt"))
            .await
            .unwrap(),
        "a live guardian"
    );
    assert!(db.load_identity().await.unwrap().is_none());
}

#[tokio::test]
async fn restore_refuses_a_preexisting_safe_event_journal_sibling() {
    let identity = test_identity();
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let seat = SeatBackupDocument::new(&facts(4), None, None, None);
    let journal = crate::seat_process::seat_dir(&process, SeatNo(0)).join("safe-events");
    tokio::fs::create_dir_all(&journal).await.unwrap();
    tokio::fs::write(journal.join("sentinel"), b"prior install")
        .await
        .unwrap();

    let db = test_db(&temp).await;
    assert!(matches!(
        install(&db, &process, &identity, &recovered(vec![seat], vec![])).await,
        Err(RestoreError::SeatDirectoryExists(_))
    ));
    assert_eq!(
        tokio::fs::read(journal.join("sentinel")).await.unwrap(),
        b"prior install"
    );
    assert!(db.load_identity().await.unwrap().is_none());
}

/// A deterministic reproduction of the states a SIGKILL can leave between the
/// filesystem writes and the fleet transaction. There is no production crash
/// hook: recreating each state here gives the retry the exact durable input
/// it would see after restart. An unchanged retry must complete — the
/// identity row is last precisely so an interrupted install stays retryable
/// without filesystem surgery.
#[tokio::test]
async fn a_crashed_restore_retries_to_completion() {
    let identity = test_identity();
    let consensus_json = big_consensus_json();
    let archive = guardian_archive(&consensus_json);
    let seat = SeatBackupDocument::new(
        &facts(5),
        Some(payment()),
        Some(guardian_ref(&archive)),
        None,
    );
    let recovered = recovered(
        vec![seat.clone()],
        vec![(seat.seat_id.clone(), archive.clone())],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;

    // Crash mid-staging: a half-written archive under the staging root. The
    // final seat directory does not exist — that is the point of staging.
    let staging = temp.path().join("restore-staging").join("0");
    tokio::fs::create_dir_all(&staging).await.unwrap();
    tokio::fs::write(staging.join("private.json"), "half-writ")
        .await
        .unwrap();

    // Crash after the rename: the final directory holds exactly what this
    // backup's staged write renamed into place, because the rename is atomic.
    write_restored_seat_dir(
        &crate::seat_process::seat_data_dir(&process, SeatNo(0)),
        &archive,
        &archive.digest(),
    )
    .await
    .unwrap();

    install(&db, &process, &identity, &recovered).await.unwrap();

    // Adopted, not rewritten or refused: the fleet transaction committed and
    // the guardian's files are the backup's bytes.
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    assert_eq!(
        tokio::fs::read_to_string(dir.join("consensus.json"))
            .await
            .unwrap(),
        consensus_json
    );
    assert!(
        !tokio::fs::try_exists(temp.path().join("restore-staging"))
            .await
            .unwrap(),
        "staging debris is wiped, not accumulated"
    );
}

/// Adoption is by digest, not by existence: a complete directory holding a
/// *different* guardian's archive is foreign, and the restore refuses rather
/// than install a fleet whose database points at keys the backup does not
/// describe.
#[tokio::test]
async fn an_existing_directory_with_a_different_archive_refuses_adoption() {
    let identity = test_identity();
    let archive = guardian_archive(&big_consensus_json());
    let seat = SeatBackupDocument::new(&facts(6), None, Some(guardian_ref(&archive)), None);
    let recovered = recovered(
        vec![seat.clone()],
        vec![(seat.seat_id.clone(), archive.clone())],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;

    let mut other = archive.clone();
    other.private_json = "someone else's shares".to_owned();
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    write_restored_seat_dir(&dir, &other, &other.digest())
        .await
        .unwrap();

    assert!(matches!(
        install(&db, &process, &identity, &recovered).await,
        Err(RestoreError::SeatDirectoryExists(seat_id)) if seat_id == seat.seat_id
    ));
    assert!(db.load_identity().await.unwrap().is_none());
    assert_eq!(
        tokio::fs::read_to_string(dir.join("private.json"))
            .await
            .unwrap(),
        "someone else's shares",
        "a foreign directory is refused, never rewritten"
    );
}

/// Adoption's shape check is as narrow as the writer: an interrupted restore
/// leaves exactly the three files the staged write renames into place, so a
/// directory holding anything more is not this restore's debris — it is
/// somebody's data, and the restore refuses rather than bury it under a fleet
/// whose database claims the directory.
#[tokio::test]
async fn an_existing_directory_with_extra_files_refuses_adoption() {
    let identity = test_identity();
    let archive = guardian_archive(&big_consensus_json());
    let seat = SeatBackupDocument::new(&facts(6), None, Some(guardian_ref(&archive)), None);
    let recovered = recovered(
        vec![seat.clone()],
        vec![(seat.seat_id.clone(), archive.clone())],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;

    // The directory matches the backup byte for byte — and holds one thing
    // more, which no interrupted attempt of this restore could have left.
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    write_restored_seat_dir(&dir, &archive, &archive.digest())
        .await
        .unwrap();
    tokio::fs::write(dir.join("database"), "a consensus db already grew here")
        .await
        .unwrap();

    assert!(matches!(
        install(&db, &process, &identity, &recovered).await,
        Err(RestoreError::SeatDirectoryExists(seat_id)) if seat_id == seat.seat_id
    ));
    assert!(db.load_identity().await.unwrap().is_none());
    assert_eq!(
        tokio::fs::read_to_string(dir.join("database"))
            .await
            .unwrap(),
        "a consensus db already grew here",
        "the refused directory is untouched"
    );
}

/// A formed seat whose guardian archive never made it to the relays cannot be
/// started, so the restore refuses instead of creating a broken guardian.
#[tokio::test]
async fn a_formed_seat_without_its_archive_refuses_the_whole_restore() {
    let identity = test_identity();
    let archive = guardian_archive(&big_consensus_json());
    let seat = SeatBackupDocument::new(&facts(4), None, Some(guardian_ref(&archive)), None);

    // The seat document is there; its archive is not.
    let recovered = recovered(vec![seat], vec![]);
    let temp = TempDir::new().unwrap();
    let db = test_db(&temp).await;
    assert!(matches!(
        install(&db, &process(&temp), &identity, &recovered).await,
        Err(RestoreError::MissingArchive { .. })
    ));
    assert!(db.load_identity().await.unwrap().is_none());
}

/// Relay replacement has already selected one addressable document per seat by
/// the time the archive hands the fleet over. A selected pre-guardian document
/// therefore leaves its otherwise complete archive unused.
#[tokio::test]
async fn a_selected_guardianless_document_ignores_a_recovered_archive() {
    let identity = test_identity();
    let archive = guardian_archive(&big_consensus_json());
    let seat = SeatBackupDocument::new(&facts(5), None, None, None);
    let recovered = recovered(vec![seat.clone()], vec![(seat.seat_id.clone(), archive)]);
    assert_eq!(recovered.formed(), 0);

    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &identity, &recovered).await.unwrap();
    assert!(
        db.list_seats()
            .await
            .unwrap()
            .iter()
            .any(|restored| restored.facts.seat_id == seat.seat_id),
        "the selected guardian-less seat must be installed"
    );

    let directory = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    assert!(
        !tokio::fs::try_exists(directory).await.unwrap(),
        "a guardian-less selected document leaves a recovered archive uninstalled"
    );
    assert!(
        db.formed_federation_invite(&seat.seat_id)
            .await
            .unwrap()
            .is_none(),
        "the restored seat must not claim the ignored guardian config"
    );
}

/// A restore is one durable step, and the identity is its last write. If the
/// seats cannot all be written, the host stays un-onboarded — which is the only
/// state from which an operator can restore again.
#[tokio::test]
async fn a_restore_that_cannot_finish_leaves_the_host_un_onboarded() {
    let identity = test_identity();
    // Two documents for one seat: the second insert violates the seats
    // table's primary key, standing in for anything that can stop the fleet
    // being written whole.
    let recovered = recovered(
        vec![
            SeatBackupDocument::new(&facts(8), Some(payment()), None, None),
            SeatBackupDocument::new(&facts(8), None, None, None),
        ],
        vec![],
    );

    let temp = TempDir::new().unwrap();
    let db = test_db(&temp).await;
    assert!(
        install(&db, &process(&temp), &identity, &recovered)
            .await
            .is_err()
    );
    assert!(
        db.load_identity().await.unwrap().is_none(),
        "a host that could not be restored whole must not look onboarded"
    );
    assert!(
        db.list_seats().await.unwrap().is_empty(),
        "the seats that did get written must roll back with the identity"
    );
}

/// The wire carries the refusal as a value, so a browser can choose a recovery
/// action without reading English.
///
/// The interesting half is the boxing. Every refusal reaches the admin surface
/// as an `anyhow::Error`, usually with context stacked above it, and a
/// classification that only looked at the top of the chain would report every
/// one of them as `Other`.
#[test]
fn a_refusal_keeps_its_discriminant_through_anyhow() {
    use crate::admin::{AdminError, AdminErrorKind};

    let cases = [
        (
            RestoreError::AlreadyOnboarded,
            AdminErrorKind::AlreadyOnboarded,
        ),
        (
            RestoreError::InvalidMnemonic,
            AdminErrorKind::InvalidMnemonic,
        ),
        (
            RestoreError::NotAcknowledged,
            AdminErrorKind::RestoreNotAcknowledged,
        ),
        (
            RestoreError::UnreadableDocument("event-id".to_owned()),
            AdminErrorKind::UnreadableBackupDocument,
        ),
        (
            RestoreError::SeatDirectoryExists(SeatId::from(QuoteId([7u8; 32]))),
            AdminErrorKind::SeatDirectoryExists,
        ),
        (
            RestoreError::MissingArchive {
                seat_id: SeatId::from(QuoteId([7u8; 32])),
            },
            AdminErrorKind::MissingGuardianArchive,
        ),
        (
            RestoreError::Other(anyhow::anyhow!("something else went wrong")),
            AdminErrorKind::Other,
        ),
    ];

    for (error, expected) in cases {
        let sentence = error.to_string();
        let boxed = anyhow::Error::new(error).context("restoring this Manifold Guardian");
        let on_the_wire = AdminError::from_error(&boxed);

        assert_eq!(on_the_wire.kind, expected, "{sentence}");
        // The sentence the operator reads is unchanged by the discriminant
        // riding beside it.
        assert!(on_the_wire.message.contains(&sentence), "{on_the_wire:?}");
    }
}
