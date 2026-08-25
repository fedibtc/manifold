use fedi_decentralized_service_fleet_manager::{
    FederationSize, FiId, InviteCode, Plan, QuoteId, SeatId,
};

use super::*;

fn test_fi_id() -> FiId {
    let mut seed = [0_u8; 32];
    seed[0] = 7;
    FiId(
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
            .unwrap()
            .x_only_public_key()
            .0,
    )
}

fn seat_facts() -> SeatFacts {
    let fi_id = test_fi_id();
    SeatFacts {
        seat_id: SeatId::from(QuoteId([3; 32])),
        seat_no: SeatNo(4),
        fi_id,
        plan: Plan::InfiniteBestEffort { price_msats: 0 },
        federation_size: FederationSize(7),
        created_at_ms: 1_700_000_000_000,
    }
}

fn guardian_archive(private_len: usize) -> GuardianArchive {
    GuardianArchive {
        private_encrypt: "ab".repeat(private_len),
        private_salt: "c2FsdHNhbHRzYWx0c2E".into(),
        local_json: r#"{"api_bind":"127.0.0.1:8174"}"#.into(),
        consensus_json: r#"{"api_endpoints":{"0":"wss://peer"}}"#.into(),
    }
}

#[tokio::test]
async fn the_guardian_archive_is_carried_and_restored_byte_for_byte() {
    let dir = tempfile::TempDir::new().unwrap();
    let seat_dir = dir.path().join("seats/0/data");
    tokio::fs::create_dir_all(&seat_dir).await.unwrap();

    let consensus = r#"{"api_endpoints":{"0":"wss://peer"}}"#;
    for (name, contents) in [
        ("private.encrypt", "deadbeef"),
        ("private.salt", "c2FsdA"),
        ("local.json", r#"{"api_bind":"127.0.0.1:8174"}"#),
        ("consensus.json", consensus),
    ] {
        tokio::fs::write(seat_dir.join(name), contents)
            .await
            .unwrap();
    }

    let archive = read_guardian_archive(&seat_dir).await.unwrap().unwrap();
    // Restore into a *new* directory: recovery never writes over a live seat.
    let restored_dir = dir.path().join("seats/1/data");
    let digest = archive.digest();
    write_restored_seat_dir(&restored_dir, &archive, &digest, "derived-api-auth")
        .await
        .unwrap();
    for name in [
        "private.encrypt",
        "private.salt",
        "local.json",
        "consensus.json",
    ] {
        assert_eq!(
            tokio::fs::read_to_string(restored_dir.join(name))
                .await
                .unwrap(),
            tokio::fs::read_to_string(seat_dir.join(name))
                .await
                .unwrap(),
            "{name} did not round-trip"
        );
    }
    // The password is not part of the round trip: it is re-derived, and
    // without it fedimintd would treat the restored config as absent.
    assert_eq!(
        tokio::fs::read_to_string(restored_dir.join("password.private"))
            .await
            .unwrap(),
        "derived-api-auth"
    );
}

#[tokio::test]
async fn a_seat_without_a_finished_ceremony_has_nothing_to_back_up() {
    let dir = tempfile::TempDir::new().unwrap();
    let seat_dir = dir.path().join("data");
    tokio::fs::create_dir_all(&seat_dir).await.unwrap();

    // No config at all.
    assert!(read_guardian_archive(&seat_dir).await.unwrap().is_none());

    // A partially written set is "not ready", never a truncated backup.
    tokio::fs::write(seat_dir.join("private.encrypt"), "deadbeef")
        .await
        .unwrap();
    assert!(read_guardian_archive(&seat_dir).await.unwrap().is_none());
}

#[tokio::test]
async fn an_archive_that_is_not_the_backups_own_is_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = guardian_archive(64);

    // The digest is the only thing standing between relay-supplied bytes and a
    // guardian's data directory, so the refusal has to come from the write
    // itself — it cannot be a check a restore path might forget to run.
    let err = write_restored_seat_dir(&dir.path().join("data"), &archive, &"11".repeat(32), "auth")
        .await
        .unwrap_err();
    assert!(
        matches!(err, RestoreConfigError::ArchiveMismatch { .. }),
        "expected ArchiveMismatch, got {err:?}"
    );
    assert!(
        !dir.path().join("data").exists(),
        "refusal left a directory"
    );
}

/// A publication is a whole value: the seat's document and, until the relay
/// has confirmed it, the guardian archive the document names. No caller
/// assembles either or decides what goes with what.
#[tokio::test]
async fn a_publication_carries_the_archive_its_document_names() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = crate::db::Db::open(temp.path()).await.unwrap();
    let process = crate::seat_process::SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: temp.path().join("fedimintd"),
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(
            crate::seat_process::BitcoindConfig {
                url: "http://127.0.0.1:18443".to_owned(),
                username: "user".to_owned(),
                password: "pass".to_owned(),
            },
        ),
    };
    let facts = seat_facts();

    crate::test_support::insert_test_seat(
        &db,
        crate::db::NewSeat {
            seat_id: facts.seat_id.clone(),
            fi_id: facts.fi_id,
            plan: facts.plan.clone(),
            federation_size: facts.federation_size,
            payment: None,
        },
    )
    .await;

    // Before DKG there is nothing but the seat itself.
    let plan = seat_publication_plan(&db, &process, &facts, None)
        .await
        .unwrap();
    assert!(plan.archive.is_none());
    assert!(plan.document.guardian.is_none());

    // A seat the database knows has run consensus refuses to be published
    // without its key shares.
    db.record_formed(&facts.seat_id, &InviteCode("test-invite".to_owned()))
        .await
        .unwrap();
    assert!(
        seat_publication_plan(&db, &process, &facts, None)
            .await
            .is_err(),
        "a seat holding key shares must not publish a document without them"
    );

    let dir = crate::seat_process::seat_data_dir(&process, facts.seat_no);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    for (name, contents) in [
        ("private.encrypt", "deadbeef"),
        ("private.salt", "c2FsdA"),
        ("local.json", r#"{"api_bind":"127.0.0.1:1"}"#),
        ("consensus.json", r#"{"api_endpoints":{}}"#),
    ] {
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }

    let plan = seat_publication_plan(&db, &process, &facts, None)
        .await
        .unwrap();
    let archive = plan
        .archive
        .as_ref()
        .expect("the archive travels unconfirmed");
    assert_eq!(archive.private_encrypt, "deadbeef");
    assert_eq!(
        plan.document.guardian.as_ref().unwrap().archive_sha256,
        archive.digest(),
        "the document names the digest of the archive published with it"
    );
    assert_eq!(plan.archive_digest(), Some(archive.digest().as_str()));

    // A confirmed digest short-circuits the file read: the document still
    // names it, but no archive travels again.
    let confirmed = archive.digest();
    let plan = seat_publication_plan(&db, &process, &facts, Some(&confirmed))
        .await
        .unwrap();
    assert!(plan.archive.is_none());
    assert_eq!(
        plan.document.guardian.as_ref().unwrap().archive_sha256,
        confirmed
    );
}
