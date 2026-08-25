use fedi_decentralized_service_fleet_manager::{FederationSize, FiId, InviteCode, Plan, QuoteId};
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

fn facts(seat: u8, seat_no: u32) -> SeatFacts {
    let mut seed = [0_u8; 32];
    seed[0] = 7;
    let fi_id = FiId(
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
            .unwrap()
            .x_only_public_key()
            .0,
    );
    SeatFacts {
        seat_id: SeatId::from(QuoteId([seat; 32])),
        seat_no: SeatNo(seat_no),
        fi_id,
        plan: Plan::InfiniteBestEffort { price_msats: 1_000 },
        federation_size: FederationSize(7),
        created_at_ms: 1_700_000_000_000,
    }
}

fn guardian_archive(consensus_json: &str) -> GuardianArchive {
    GuardianArchive {
        private_encrypt: "deadbeef".into(),
        private_salt: "c2FsdA".into(),
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
/// adapter's and is tested with it (`fman-nostr`); these tests exercise what
/// the install does with the result.
fn recovered(
    seats: Vec<SeatBackupDocument>,
    archives: Vec<(SeatId, GuardianArchive)>,
) -> RecoveredFleet {
    RecoveredFleet {
        seats,
        archives: archives.into_iter().collect(),
        format_version: 1,
    }
}

fn process(temp: &TempDir) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: temp.path().join("fedimintd"),
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(BitcoindConfig {
            url: "http://127.0.0.1:18443".to_owned(),
            username: "user".to_owned(),
            password: "pass".to_owned(),
        }),
    }
}

async fn test_db(temp: &TempDir) -> Db {
    Db::open(temp.path()).await.unwrap()
}

/// The whole capability, end to end: a recovered backup produces a fleet with
/// its guardian configs on disk and its facts in the database.
#[tokio::test]
async fn a_recovered_backup_rebuilds_the_fleet() {
    let identity = test_identity();
    let consensus_json = big_consensus_json();

    let archive = guardian_archive(&consensus_json);
    let formed = SeatBackupDocument::new(
        &facts(1, 0),
        Some(payment()),
        Some(guardian_ref(&archive)),
        None,
    );
    let unformed = SeatBackupDocument::new(&facts(2, 1), Some(payment()), None, None);

    let recovered = recovered(
        vec![formed.clone(), unformed.clone()],
        vec![(formed.seat_id.clone(), archive)],
    );
    assert_eq!(recovered.formed(), 1);

    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;
    install(&db, &process, &identity, &recovered).await.unwrap();

    // The install now *is* the FMan the backup described.
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );

    // The formed seat's guardian is on disk, byte for byte, with the consensus
    // config the backup carried.
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    assert_eq!(
        tokio::fs::read_to_string(dir.join("private.encrypt"))
            .await
            .unwrap(),
        "deadbeef"
    );
    assert_eq!(
        tokio::fs::read_to_string(dir.join("consensus.json"))
            .await
            .unwrap(),
        consensus_json
    );
    // The password is re-derived, not restored: without it fedimintd refuses
    // to load the config and would enter a fresh ceremony over live shares.
    assert_eq!(
        tokio::fs::read_to_string(dir.join("password.private"))
            .await
            .unwrap(),
        identity.derive_seat_keys(&formed.seat_id).api_auth
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

    // Seat numbers are restored, not reallocated: they name the data directory
    // and the port block, so reassigning them would point a recovered guardian
    // at someone else's.
    let seats = db.list_seats().await.unwrap();
    let mut numbers: Vec<u32> = seats.iter().map(|seat| seat.facts.seat_no.0).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, vec![0, 1]);

    // The money came back with them.
    assert!(db.payment(&formed.seat_id).await.unwrap().is_some());
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
/// `CLAIM-fleet-manager-preserves-published-guardian-data`.
#[tokio::test]
async fn a_restore_never_writes_into_an_existing_seat_directory() {
    let identity = test_identity();
    let temp = TempDir::new().unwrap();
    let process = process(&temp);

    let seat = SeatBackupDocument::new(&facts(3, 0), None, None, None);
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
    let seat = SeatBackupDocument::new(&facts(4, 0), None, None, None);
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
        &facts(5, 0),
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
    tokio::fs::write(staging.join("private.encrypt"), "half-writ")
        .await
        .unwrap();

    // Crash after the rename: the final directory holds exactly what this
    // backup's staged write renamed into place, because the rename is atomic.
    write_restored_seat_dir(
        &crate::seat_process::seat_data_dir(&process, SeatNo(0)),
        &archive,
        &archive.digest(),
        &identity.derive_seat_keys(&seat.seat_id).api_auth,
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
    let seat = SeatBackupDocument::new(&facts(6, 0), None, Some(guardian_ref(&archive)), None);
    let recovered = recovered(
        vec![seat.clone()],
        vec![(seat.seat_id.clone(), archive.clone())],
    );
    let temp = TempDir::new().unwrap();
    let process = process(&temp);
    let db = test_db(&temp).await;

    let mut other = archive.clone();
    other.private_encrypt = "someone else's shares".to_owned();
    let dir = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    write_restored_seat_dir(
        &dir,
        &other,
        &other.digest(),
        &identity.derive_seat_keys(&seat.seat_id).api_auth,
    )
    .await
    .unwrap();

    assert!(matches!(
        install(&db, &process, &identity, &recovered).await,
        Err(RestoreError::SeatDirectoryExists(seat_id)) if seat_id == seat.seat_id
    ));
    assert!(db.load_identity().await.unwrap().is_none());
    assert_eq!(
        tokio::fs::read_to_string(dir.join("private.encrypt"))
            .await
            .unwrap(),
        "someone else's shares",
        "a foreign directory is refused, never rewritten"
    );
}

/// Adoption's shape check is as narrow as the writer: an interrupted restore
/// leaves exactly the five files the staged write renames into place, so a
/// directory holding anything more is not this restore's debris — it is
/// somebody's data, and the restore refuses rather than bury it under a fleet
/// whose database claims the directory.
#[tokio::test]
async fn an_existing_directory_with_extra_files_refuses_adoption() {
    let identity = test_identity();
    let archive = guardian_archive(&big_consensus_json());
    let seat = SeatBackupDocument::new(&facts(6, 0), None, Some(guardian_ref(&archive)), None);
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
    write_restored_seat_dir(
        &dir,
        &archive,
        &archive.digest(),
        &identity.derive_seat_keys(&seat.seat_id).api_auth,
    )
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
    let seat = SeatBackupDocument::new(&facts(4, 0), None, Some(guardian_ref(&archive)), None);

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
    let seat = SeatBackupDocument::new(&facts(5, 0), None, None, None);
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
    // Two distinct seats claiming one seat number: the second insert violates
    // the seats table's uniqueness, standing in for anything that can stop the
    // fleet being written whole.
    let recovered = recovered(
        vec![
            SeatBackupDocument::new(&facts(8, 3), Some(payment()), None, None),
            SeatBackupDocument::new(&facts(9, 3), None, None, None),
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
        let boxed = anyhow::Error::new(error).context("restoring this Fleet Manager");
        let on_the_wire = AdminError::from_error(&boxed);

        assert_eq!(on_the_wire.kind, expected, "{sentence}");
        // The sentence the operator reads is unchanged by the discriminant
        // riding beside it.
        assert!(on_the_wire.message.contains(&sentence), "{on_the_wire:?}");
    }
}
