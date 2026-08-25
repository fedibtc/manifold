use fedi_decentralized_service_fleet_manager::{FederationSize, FiId, InviteCode, Plan, QuoteId};
use fman_core::backup::GuardianArchiveRef;
use fman_core::facts::{SeatFacts, SeatNo};
use fman_core::identity::RootMnemonic;

use super::*;

const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn test_mnemonic() -> RootMnemonic {
    RootMnemonic::parse(TEST_MNEMONIC).unwrap()
}

fn test_identity() -> BackupIdentity {
    BackupIdentity::derive(&test_mnemonic())
}

fn stranger_identity() -> BackupIdentity {
    BackupIdentity::derive(
        &RootMnemonic::parse("zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong").unwrap(),
    )
}

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

fn seat_facts_no(seat: u8, seat_no: u32) -> SeatFacts {
    let fi_id = test_fi_id();
    SeatFacts {
        seat_id: SeatId::from(QuoteId([seat; 32])),
        seat_no: SeatNo(seat_no),
        fi_id,
        plan: Plan::InfiniteBestEffort { price_msats: 0 },
        federation_size: FederationSize(7),
        created_at_ms: 1_700_000_000_000,
    }
}

fn seat_facts() -> SeatFacts {
    seat_facts_no(3, 4)
}

fn guardian_archive(private_len: usize) -> GuardianArchive {
    GuardianArchive {
        private_encrypt: "ab".repeat(private_len),
        private_salt: "c2FsdHNhbHRzYWx0c2E".into(),
        local_json: r#"{"api_bind":"127.0.0.1:8174"}"#.into(),
        consensus_json: r#"{"api_endpoints":{"0":"wss://peer"}}"#.into(),
    }
}

/// An archive big enough to need more than one event, which is the only
/// interesting size: the real `consensus.json` runs to a hundred kilobytes.
fn big_guardian_archive() -> GuardianArchive {
    let mut archive = guardian_archive(64);
    archive.consensus_json = format!(r#"{{"pad":"{}"}}"#, "a".repeat(40_000));
    archive
}

fn guardian_ref() -> GuardianArchiveRef {
    GuardianArchiveRef {
        archive_sha256: guardian_archive(64).digest(),
        federation_invite: Some(InviteCode("fed11-backup-test".into())),
    }
}

/// Publish exactly as the daemon would, so decoding is tested against the
/// real event shape rather than a hand-built one.
fn published_events(identity: &BackupIdentity, publication: &SeatPublication) -> Vec<Event> {
    identity
        .publication_events(publication)
        .unwrap()
        .into_iter()
        .map(|builder| builder.sign_with_keys(identity.keys()).unwrap())
        .collect()
}

fn formed_publication(facts: &SeatFacts, archive: &GuardianArchive) -> SeatPublication {
    SeatPublication {
        document: SeatBackupDocument::new(
            facts,
            None,
            Some(GuardianArchiveRef {
                archive_sha256: archive.digest(),
                federation_invite: None,
            }),
            None,
        ),
        archive: Some(archive.clone()),
    }
}

#[test]
fn seat_document_roundtrips_through_seal_and_restores_the_original_facts() {
    let identity = test_identity();
    let facts = seat_facts();
    let document = SeatBackupDocument::new(&facts, None, Some(guardian_ref()), None);

    let sealed = identity.seal_seat_document(&document).unwrap();
    let recovered = identity.open_seat_document(&sealed).unwrap();

    assert_eq!(recovered, document);
    // The whole point of the document: the durable facts come back intact.
    assert_eq!(recovered.to_seat_facts(), facts);
    // The guardian half is named, not carried: this document is republished
    // whenever the seat changes, and the archive it points at never changes.
    assert_eq!(recovered.guardian.unwrap(), guardian_ref());
}

#[test]
fn sealed_contents_are_one_fixed_size_whatever_they_carry() {
    let identity = test_identity();
    let facts = seat_facts();

    let formed = identity
        .seal_seat_document(&SeatBackupDocument::new(
            &facts,
            None,
            Some(guardian_ref()),
            None,
        ))
        .unwrap();
    let unformed = identity
        .seal_seat_document(&SeatBackupDocument::new(&facts, None, None, None))
        .unwrap();
    let slices = identity.seal_archive(&big_guardian_archive());

    // A seat that has formed, a seat that has not, and every slice of a
    // guardian archive are all one length on the relay — an observer cannot
    // tell them apart, or count either family
    // (SPEC-nostr-backup-restore, *Documents*).
    assert_eq!(formed.len(), unformed.len());
    assert!(slices.len() > 1, "the test needs a multi-event archive");
    for slice in &slices {
        assert_eq!(formed.len(), slice.len());
    }
}

#[test]
fn an_oversized_document_fails_loudly_instead_of_being_truncated() {
    let identity = test_identity();
    let document = SeatBackupDocument::new(
        &seat_facts(),
        None,
        Some(GuardianArchiveRef {
            archive_sha256: "11".repeat(32),
            federation_invite: Some(InviteCode("a".repeat(PADDED_PLAINTEXT_LEN))),
        }),
        None,
    );

    let err = identity.seal_seat_document(&document).unwrap_err();
    assert!(
        matches!(err, BackupError::TooLarge { .. }),
        "expected TooLarge, got {err:?}"
    );
}

#[test]
fn another_mnemonic_cannot_read_a_backup() {
    let document = SeatBackupDocument::new(&seat_facts(), None, Some(guardian_ref()), None);
    let sealed = test_identity().seal_seat_document(&document).unwrap();

    let err = stranger_identity().open_seat_document(&sealed).unwrap_err();
    assert!(
        matches!(err, BackupError::Decrypt),
        "expected Decrypt, got {err:?}"
    );
}

#[test]
fn backup_events_carry_only_their_blinded_coordinate() {
    let identity = test_identity();
    let document = SeatBackupDocument::new(&seat_facts(), None, Some(guardian_ref()), None);
    let events = published_events(
        &identity,
        &SeatPublication {
            document: document.clone(),
            archive: None,
        },
    );
    let [event] = events.as_slice() else {
        panic!("a documents-only publication is one event");
    };

    assert_eq!(
        event.kind,
        Kind::Custom(fedi_decentralized_nostr::fman::FMAN_BACKUP_EVENT_KIND)
    );
    // Exactly one tag: anything further would describe in the clear what the
    // content seals.
    let tags: Vec<_> = event.tags.iter().map(nostr_sdk::Tag::as_slice).collect();
    let coordinate = identity.seat_coordinate(&document.seat_id);
    assert_eq!(tags, vec![vec!["d".to_owned(), coordinate.clone()]]);

    // The coordinate is public, and the seat id inside it is known to the FI
    // that bought the seat: it must not appear, in any spelling, anywhere on
    // the event.
    let seat_id = document.seat_id.to_string();
    assert!(
        !coordinate.contains(&seat_id),
        "coordinate leaks the seat id"
    );
    assert!(
        !event.content.contains(&seat_id),
        "encrypted content leaks the seat id"
    );

    // Blinded per seat and per install: a second seat lands elsewhere, and
    // another mnemonic addressing the same seat lands elsewhere again.
    let other_seat = SeatId::from(QuoteId([4; 32]));
    assert_ne!(coordinate, identity.seat_coordinate(&other_seat));
    assert_ne!(
        coordinate,
        stranger_identity().seat_coordinate(&document.seat_id)
    );
}

#[test]
fn same_second_live_event_can_sort_before_its_tombstone() {
    let identity = test_identity();
    let event = |content| {
        nostr_sdk::EventBuilder::new(
            Kind::Custom(fedi_decentralized_nostr::fman::FMAN_BACKUP_EVENT_KIND),
            content,
        )
        .tag(nostr_sdk::Tag::identifier("same-blinded-coordinate"))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(1))
        .sign_with_keys(identity.keys())
        .unwrap()
    };
    let live = event("live");
    let tombstone = event("tombstone");

    assert_eq!(live.created_at, tombstone.created_at);
    assert!(live.id < tombstone.id);
}

/// A publication is encoded whole: the seat's archive slices and then the
/// seat's own document, so a relay never holds a seat document naming a digest
/// whose archive is not there yet. No caller assembles or orders any of this.
#[test]
fn a_publication_carries_its_archive_slices_before_the_seat_document() {
    let identity = test_identity();
    let archive = big_guardian_archive();
    let publication = formed_publication(&seat_facts(), &archive);
    let events = published_events(&identity, &publication);

    let (seat, slices) = events.split_last().unwrap();
    assert!(slices.len() > 1, "the test needs a multi-event archive");
    // The last event is the seat's document, at the seat's coordinate.
    assert_eq!(
        identity.open_seat_document(&seat.content).unwrap(),
        publication.document
    );
    assert_eq!(
        seat.tags.identifier().unwrap(),
        identity.seat_coordinate(&publication.document.seat_id)
    );
    // The slices before it sit at consecutive archive coordinates, in order,
    // and none of them opens on its own: order and membership live in the
    // coordinate and the cipher, not in any plaintext marker.
    for (index, slice) in slices.iter().enumerate() {
        assert_eq!(
            slice.tags.identifier().unwrap(),
            identity
                .archive_coordinate(&publication.document.seat_id, u32::try_from(index).unwrap())
        );
        assert!(matches!(
            identity.open_seat_document(&slice.content),
            Err(BackupError::Decrypt)
        ));
    }

    // A confirmed archive is not re-encoded: the publication carries only the
    // document.
    let events = published_events(
        &identity,
        &SeatPublication {
            document: publication.document.clone(),
            archive: None,
        },
    );
    assert_eq!(events.len(), 1);
}

#[test]
fn a_document_from_a_future_version_is_refused() {
    let identity = test_identity();
    // A future version's body is one this build cannot parse — here, not
    // even a map. The refusal must come from the version check alone, before
    // any typed parsing: a Parse error instead would mean the reader tried
    // the body first.
    let mut payload = Vec::new();
    ciborium::into_writer(
        &Envelope {
            version: BACKUP_DOCUMENT_VERSION + 1,
            document: 0xF00D_u32,
        },
        &mut payload,
    )
    .unwrap();
    let sealed = identity.seal_padded(payload, AAD_DOCUMENT).remove(0);

    let err = identity.open_seat_document(&sealed).unwrap_err();
    assert!(
        matches!(err, BackupError::UnsupportedVersion { .. }),
        "expected UnsupportedVersion, got {err:?}"
    );
}

/// The frame inside a seal has one canonical spelling: a length prefix, the
/// payload, zero padding short of a whole extra event, and nothing hiding
/// after the CBOR item inside the declared payload. Each deviation is our own
/// writing gone wrong, and each is refused rather than read around.
#[test]
fn a_non_canonical_frame_is_refused() {
    let identity = test_identity();
    let document = SeatBackupDocument::new(&seat_facts(), None, None, None);
    let mut payload = Vec::new();
    ciborium::into_writer(
        &Envelope {
            version: BACKUP_DOCUMENT_VERSION,
            document: &document,
        },
        &mut payload,
    )
    .unwrap();

    let seal_frame = |frame: Vec<u8>| BASE64.encode(identity.seal_blob(&frame, AAD_DOCUMENT));
    let framed = |payload: &[u8], padded_len: usize| {
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        frame.resize(padded_len, 0);
        frame
    };

    // The declared length runs past the sealed bytes.
    let overrun = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
    // A padding byte is not zero.
    let mut dirty_padding = framed(&payload, PADDED_PLAINTEXT_LEN);
    *dirty_padding.last_mut().unwrap() = 1;
    // A whole event of padding beyond what the payload needs.
    let excess_padding = framed(&payload, PADDED_PLAINTEXT_LEN + SEALED_LEN);
    // Bytes after the CBOR item, inside the declared payload length.
    let mut trailing = payload.clone();
    trailing.push(0);
    let trailing_junk = framed(&trailing, PADDED_PLAINTEXT_LEN);

    for (case, frame) in [
        ("length overrun", overrun),
        ("nonzero padding", dirty_padding),
        ("excess padding", excess_padding),
        ("trailing bytes in payload", trailing_junk),
    ] {
        let err = identity.open_seat_document(&seal_frame(frame)).unwrap_err();
        assert!(
            matches!(err, BackupError::Frame),
            "{case}: expected Frame, got {err:?}"
        );
    }

    // The canonical spelling of the same document still opens.
    let canonical = seal_frame(framed(&payload, PADDED_PLAINTEXT_LEN));
    identity.open_seat_document(&canonical).unwrap();
}

/// The decode half of a restore: the published events come back as the seats
/// and per-seat archives the install consumes, under the version that read
/// them.
#[test]
fn published_events_decode_into_the_fleet_they_describe() {
    let identity = test_identity();
    let archive = big_guardian_archive();
    let publication = formed_publication(&seat_facts_no(1, 0), &archive);
    let unformed = SeatBackupDocument::new(&seat_facts_no(2, 1), None, None, None);

    let mut events = published_events(&identity, &publication);
    assert!(events.len() > 2, "the interesting case is a sliced archive");
    events.extend(published_events(
        &identity,
        &SeatPublication {
            document: unformed.clone(),
            archive: None,
        },
    ));

    let recovered = recover_from_events(&identity, events).unwrap();
    assert_eq!(recovered.seats.len(), 2);
    assert_eq!(recovered.formed(), 1);
    assert_eq!(recovered.format_version, BACKUP_DOCUMENT_VERSION);
    assert_eq!(recovered.archives[&publication.document.seat_id], archive);
}

/// The AEAD tag authenticates the archive as one whole: slices of two
/// different seals — even of byte-identical archives — refuse to open as one,
/// so the relay cannot splice a plausible archive out of publications it has
/// seen.
#[test]
fn slices_of_two_different_seals_never_reassemble_into_one_archive() {
    let identity = test_identity();
    let archive = big_guardian_archive();
    let publication = formed_publication(&seat_facts(), &archive);
    // The same publication sealed twice: same seat, same archive, but a fresh
    // nonce, so the slices belong to two different wholes.
    let first = published_events(&identity, &publication);
    let second = published_events(&identity, &publication);
    assert!(first.len() > 2, "the test needs a multi-event archive");

    let mut events = first;
    let replaced = events.len() - 2;
    events[replaced] = second[replaced].clone();

    let Err(err) = recover_from_events(&identity, events) else {
        panic!("spliced slices must not reassemble");
    };
    assert!(
        err.to_string().contains("guardian archive for seat"),
        "expected an archive refusal, got {err:?}"
    );
}

/// Withholding any slice — the relay serving a strict subset — refuses the
/// restore rather than yielding a shorter archive.
#[test]
fn a_withheld_archive_slice_refuses_the_restore() {
    let identity = test_identity();
    let archive = big_guardian_archive();
    let mut events = published_events(&identity, &formed_publication(&seat_facts(), &archive));
    assert!(events.len() > 2, "the test needs a multi-event archive");
    // Drop the last slice; the document itself stays published.
    events.remove(events.len() - 2);

    assert!(recover_from_events(&identity, events).is_err());
}

/// Slices belong to the seat that published them, not to a digest. Two seats
/// holding byte-identical configs — the same digest — do not cover for each
/// other: the restore of a seat reads that seat's own coordinates, which is
/// what makes there be nothing to resolve between documents.
#[test]
fn one_seats_slices_do_not_stand_in_for_another_seats() {
    let identity = test_identity();
    let archive = big_guardian_archive();
    let backed_up = formed_publication(&seat_facts_no(6, 0), &archive);
    let bare = SeatBackupDocument::new(
        &seat_facts_no(7, 1),
        None,
        Some(GuardianArchiveRef {
            archive_sha256: archive.digest(),
            federation_invite: None,
        }),
        None,
    );

    let mut events = published_events(&identity, &backed_up);
    events.extend(published_events(
        &identity,
        &SeatPublication {
            document: bare.clone(),
            archive: None,
        },
    ));

    let recovered = recover_from_events(&identity, events).unwrap();
    assert!(recovered.archives.contains_key(&backed_up.document.seat_id));
    assert!(
        !recovered.archives.contains_key(&bare.seat_id),
        "an archive must not be credited to a seat that did not publish it"
    );
}

/// A document under this identity that will not read is this fleet's own, and
/// a restore happens once: rebuilding what is left would be a fleet missing
/// whatever that document held, permanently and without anyone being told.
#[test]
fn a_document_that_cannot_be_read_refuses_the_restore() {
    let seat = SeatBackupDocument::new(&seat_facts(), None, None, None);

    // Relay answers are signature-verified and filtered by author, so this
    // stands in for the case that can actually reach a restore: a payload
    // this mnemonic published that this build cannot open — it classifies as
    // an archive slice, no seat claims it, and the leftover is fatal.
    assert!(matches!(
        recover_from_events(
            &test_identity(),
            published_events(
                &stranger_identity(),
                &SeatPublication {
                    document: seat,
                    archive: None,
                },
            ),
        ),
        Err(RecoverError::UnreadableDocument(_))
    ));
}

/// The blocker a review caught: an archive small enough to seal into a
/// *single* slice is a complete AEAD whole, and without domain separation it
/// would open as a document candidate and abort the restore as unreadable.
/// The AAD keeps it in the archive domain whatever its size.
#[test]
fn a_single_slice_archive_restores() {
    let identity = test_identity();
    let archive = guardian_archive(64);
    let publication = formed_publication(&seat_facts(), &archive);
    let events = published_events(&identity, &publication);
    assert_eq!(events.len(), 2, "the test needs a one-slice archive");

    let recovered = recover_from_events(&identity, events).unwrap();
    assert_eq!(recovered.archives[&publication.document.seat_id], archive);
}

/// Addressable events replace; two live events at one coordinate mean the
/// enumeration cannot be the latest of anything — including two copies of a
/// seat document, which would otherwise both parse.
#[test]
fn a_duplicate_document_event_refuses_the_restore() {
    let identity = test_identity();
    let publication = SeatPublication {
        document: SeatBackupDocument::new(&seat_facts(), None, None, None),
        archive: None,
    };
    let mut events = published_events(&identity, &publication);
    events.extend(published_events(&identity, &publication));

    let Err(err) = recover_from_events(&identity, events) else {
        panic!("two documents at one coordinate must refuse");
    };
    assert!(
        err.to_string()
            .contains("two backup events at one coordinate"),
        "got {err:?}"
    );
}

/// A document names its own coordinate (derived from the seat id inside it);
/// the relay serving it anywhere else is a rearrangement this restore will
/// not act on.
#[test]
fn a_document_at_a_foreign_coordinate_refuses_the_restore() {
    let identity = test_identity();
    let publication = SeatPublication {
        document: SeatBackupDocument::new(&seat_facts(), None, None, None),
        archive: None,
    };
    let content = published_events(&identity, &publication).remove(0).content;
    let misplaced = addressable_event(content, "d0".repeat(32))
        .sign_with_keys(identity.keys())
        .unwrap();

    assert!(matches!(
        recover_from_events(&identity, vec![misplaced]),
        Err(RecoverError::UnreadableDocument(_))
    ));
}

/// Withholding the *first* slice leaves the rest unclaimed (the probe starts
/// at index 0), and unclaimed events are fatal.
#[test]
fn a_missing_first_slice_refuses_the_restore() {
    let identity = test_identity();
    let mut events = published_events(
        &identity,
        &formed_publication(&seat_facts(), &big_guardian_archive()),
    );
    assert!(events.len() > 2, "the test needs a multi-event archive");
    events.remove(0);

    assert!(matches!(
        recover_from_events(&identity, events),
        Err(RecoverError::UnreadableDocument(_))
    ));
}

/// An event that is not even a seal — garbage content at an unknown
/// coordinate — is still this author's on the wire, and still refuses.
#[test]
fn a_garbage_event_refuses_the_restore() {
    let identity = test_identity();
    let garbage = addressable_event("not base64 !!".to_owned(), "ab".repeat(32))
        .sign_with_keys(identity.keys())
        .unwrap();

    assert!(matches!(
        recover_from_events(&identity, vec![garbage]),
        Err(RecoverError::UnreadableDocument(_))
    ));
}

/// The padding boundary: a payload exactly at one event's capacity stays one
/// event, one byte more spills to two, and every slice is the uniform length
/// either way.
#[test]
fn padding_boundaries_produce_whole_uniform_events() {
    let identity = test_identity();
    let capacity = DOCUMENT_CAPACITY;

    let exact = identity.seal_padded(vec![7; capacity], AAD_ARCHIVE);
    let spilled = identity.seal_padded(vec![7; capacity + 1], AAD_ARCHIVE);
    let empty = identity.seal_padded(Vec::new(), AAD_ARCHIVE);

    assert_eq!(exact.len(), 1);
    assert_eq!(spilled.len(), 2);
    assert_eq!(empty.len(), 1);
    let uniform = exact[0].len();
    for slice in exact.iter().chain(&spilled).chain(&empty) {
        assert_eq!(slice.len(), uniform);
    }
}
