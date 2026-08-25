//! Tests for complete Nostr event admission and rollback protection.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

use fedi_decentralized_domain::{
    DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM, InviteCode, ProtocolV1, SetupPaymentFederationsContent, Url,
};
use nostr::secp256k1::Message;
use nostr::{EventBuilder, EventId, Keys, SECP256K1, Tag, Tags};

use super::*;

const VALID_INVITE: &str = "fed11qgqpu8rhwden5te0vejkg6tdd9h8gepwd4cxcumxv4jzuen0duhsqqfqh6nl7sgk72caxfx8khtfnn8y436q3nhyrkev3qp8ugdhdllnh86qmp42pm";
const SIGNED_EVENT_FIXTURE: &str = r#"{"id":"ee7d6ff74b9a89b0fe488a3aa124a24b00a799002fee31d85c71597c95eb5de4","pubkey":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","created_at":1700000000,"kind":37707,"tags":[["d","setup-payment-federations"]],"content":"{\"version\":1,\"fman_version\":\"0.1.0\",\"federations\":[],\"telemetry_registration_url\":\"https://push.fedi.example/v1/telemetry/registrations\"}","sig":"975a7d6cf9a4fda41c460eb01d6365effd076290b68f3dd197a19c7dc89383d2d37cf607e64e0b58688f054f9008896e0550a9b84137e7a81581c1ac0a9de5a0"}"#;

fn content(invites: &[&str]) -> String {
    serde_json::to_string(&SetupPaymentFederationsContent {
        version: ProtocolV1,
        fman_version: "0.1.0".parse().expect("valid FMan version"),
        federations: invites
            .iter()
            .map(|invite| InviteCode((*invite).to_owned()))
            .collect(),
        telemetry_registration_url: Url(
            "https://push.fedi.example/v1/telemetry/registrations".to_owned()
        ),
        min_fee_ppm: DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM,
    })
    .expect("test content serializes")
}

fn event(keys: &Keys, created_at: u64, content: String) -> Event {
    let content: SetupPaymentFederationsContent =
        serde_json::from_str(&content).expect("test content parses as the shared wire type");
    setup_payment_federations_event_builder(&content)
        .expect("test content passes producer admission")
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(keys)
        .expect("test event signs")
}

fn deterministic_event() -> Event {
    let keys = Keys::parse("0000000000000000000000000000000000000000000000000000000000000001")
        .expect("fixed test key is valid");
    let created_at = Timestamp::from_secs(1_700_000_000);
    let kind = Kind::from(37_707);
    let tags = vec![Tag::identifier("setup-payment-federations")];
    let content = r#"{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#;
    let id = EventId::new(
        &keys.public_key(),
        &created_at,
        &kind,
        &Tags::from_list(tags.clone()),
        content,
    );
    let signature = SECP256K1.sign_schnorr_no_aux_rand(
        &Message::from_digest(id.to_bytes()),
        keys.key_pair(SECP256K1),
    );
    Event::new(
        id,
        keys.public_key(),
        created_at,
        kind,
        tags,
        content,
        signature,
    )
}

#[test]
fn complete_signed_event_wire_fixture_is_stable() {
    let event = deterministic_event();
    assert_eq!(
        serde_json::to_string(&event).expect("fixed event serializes"),
        SIGNED_EVENT_FIXTURE
    );
    let event: Event =
        serde_json::from_str(SIGNED_EVENT_FIXTURE).expect("fixed event fixture parses");
    admit_setup_payment_federations_event(
        &event,
        event.pubkey,
        Timestamp::from_secs(1_700_000_000),
        None,
    )
    .expect("fixed event is admitted");
}

#[test]
fn protocol_constants_are_pinned() {
    assert_eq!(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND, 37_707);
    assert_eq!(SETUP_PAYMENT_FEDERATIONS_D_TAG, "setup-payment-federations");
    assert_eq!(SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS, 86_400);
}

#[test]
fn admits_complete_event_and_empty_stop_set() {
    let keys = Keys::generate();
    let populated = event(&keys, 1_000, content(&[VALID_INVITE]));
    let admitted = admit_setup_payment_federations_event(
        &populated,
        keys.public_key(),
        Timestamp::from_secs(1_000),
        None,
    )
    .expect("valid event is admitted");
    assert_eq!(admitted.event(), &populated);
    assert_eq!(admitted.set().len(), 1);

    let empty = event(&keys, 1_001, content(&[]));
    assert!(
        admit_setup_payment_federations_event(
            &empty,
            keys.public_key(),
            Timestamp::from_secs(1_001),
            Some(&admitted),
        )
        .expect("empty stop set is valid")
        .set()
        .is_empty()
    );
}

#[test]
fn rejects_invalid_signature_publisher_kind_and_d_tag() {
    let keys = Keys::generate();
    let valid = event(&keys, 1_000, content(&[]));

    let mut invalid = valid.clone();
    invalid.content.push(' ');
    assert_eq!(
        admit_setup_payment_federations_event(
            &invalid,
            keys.public_key(),
            Timestamp::from_secs(1_000),
            None,
        )
        .expect_err("modified event is invalid"),
        SetupPaymentFederationsEventError::InvalidEvent
    );
    assert_eq!(
        admit_setup_payment_federations_event(
            &valid,
            Keys::generate().public_key(),
            Timestamp::from_secs(1_000),
            None,
        )
        .expect_err("wrong publisher is rejected"),
        SetupPaymentFederationsEventError::WrongPublisher
    );

    let wrong_kind = EventBuilder::new(Kind::from(37708), content(&[]))
        .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG))
        .custom_created_at(Timestamp::from_secs(1_000))
        .sign_with_keys(&keys)
        .expect("test event signs");
    assert_eq!(
        admit_setup_payment_federations_event(
            &wrong_kind,
            keys.public_key(),
            Timestamp::from_secs(1_000),
            None,
        )
        .expect_err("wrong kind is rejected"),
        SetupPaymentFederationsEventError::WrongKind
    );

    for tags in [
        vec![],
        vec![Tag::identifier("wrong")],
        vec![
            Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG),
            Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG),
        ],
        vec![Tag::parse(["d"]).expect("raw short d tag parses")],
        vec![
            Tag::parse(["d", SETUP_PAYMENT_FEDERATIONS_D_TAG, "extra"])
                .expect("raw long d tag parses"),
        ],
    ] {
        let wrong_d_tag = EventBuilder::new(
            Kind::from(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
            content(&[]),
        )
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(1_000))
        .sign_with_keys(&keys)
        .expect("test event signs");
        assert_eq!(
            admit_setup_payment_federations_event(
                &wrong_d_tag,
                keys.public_key(),
                Timestamp::from_secs(1_000),
                None,
            )
            .expect_err("wrong d tag is rejected"),
            SetupPaymentFederationsEventError::WrongDTag
        );
    }
}

#[test]
fn accepts_exact_future_bound_and_rejects_one_second_beyond() {
    let keys = Keys::generate();
    let now = Timestamp::from_secs(1_000);
    let boundary = event(
        &keys,
        now.as_secs() + SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS,
        content(&[]),
    );
    admit_setup_payment_federations_event(&boundary, keys.public_key(), now, None)
        .expect("exact 24-hour lead is accepted");

    let too_far = event(
        &keys,
        now.as_secs() + SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS + 1,
        content(&[]),
    );
    assert_eq!(
        admit_setup_payment_federations_event(&too_far, keys.public_key(), now, None)
            .expect_err("more than 24 hours is rejected"),
        SetupPaymentFederationsEventError::CreatedTooFarInFuture
    );
}

#[test]
fn address_authentication_treats_future_schema_and_time_as_opaque() {
    let keys = Keys::generate();
    let current = event(&keys, 1_000, content(&[]));
    let future_schema = EventBuilder::new(
        Kind::from(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
        r#"{"version":2,"new_required_field":true}"#,
    )
    .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG))
    .custom_created_at(Timestamp::from_secs(
        1_000 + SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS + 1,
    ))
    .sign_with_keys(&keys)
    .expect("future-schema test event signs");

    let current = authenticate_setup_payment_federations_address_event(&current, keys.public_key())
        .expect("current address authenticates");
    let future =
        authenticate_setup_payment_federations_address_event(&future_schema, keys.public_key())
            .expect("future schema and timestamp do not affect address authentication");
    assert!(future.is_newer_than(&current));
    assert!(
        admit_setup_payment_federations_event(
            &future_schema,
            keys.public_key(),
            Timestamp::from_secs(1_000),
            None,
        )
        .is_err(),
        "full semantic admission remains separate"
    );
}

#[test]
fn enforces_nip01_replacement_order_and_allows_replay() {
    let keys = Keys::generate();
    let current = event(&keys, 2_000, content(&[]));
    let current = admit_setup_payment_federations_event(
        &current,
        keys.public_key(),
        Timestamp::from_secs(2_000),
        None,
    )
    .expect("current event is admitted");
    let older = event(&keys, 1_999, content(&[VALID_INVITE]));
    assert_eq!(
        admit_setup_payment_federations_event(
            &older,
            keys.public_key(),
            Timestamp::from_secs(2_000),
            Some(&current),
        )
        .expect_err("older event is a rollback"),
        SetupPaymentFederationsEventError::Rollback
    );

    admit_setup_payment_federations_event(
        current.event(),
        keys.public_key(),
        Timestamp::from_secs(2_000),
        Some(&current),
    )
    .expect("same event is idempotent");

    let later = event(&keys, 2_001, content(&[VALID_INVITE]));
    admit_setup_payment_federations_event(
        &later,
        keys.public_key(),
        Timestamp::from_secs(2_001),
        Some(&current),
    )
    .expect("later timestamp replaces current event");
}

#[test]
fn lower_event_id_wins_timestamp_tie() {
    let keys = Keys::generate();
    let first = event(&keys, 2_000, content(&[]));
    let second = event(&keys, 2_000, content(&[VALID_INVITE]));
    let (preferred, displaced) = if first.id < second.id {
        (&first, &second)
    } else {
        (&second, &first)
    };
    let displaced = admit_setup_payment_federations_event(
        displaced,
        keys.public_key(),
        Timestamp::from_secs(2_000),
        None,
    )
    .expect("displaced event is initially admitted");

    let preferred = admit_setup_payment_federations_event(
        preferred,
        keys.public_key(),
        Timestamp::from_secs(2_000),
        Some(&displaced),
    )
    .expect("lower event ID wins equal timestamp");
    assert_eq!(
        admit_setup_payment_federations_event(
            displaced.event(),
            keys.public_key(),
            Timestamp::from_secs(2_000),
            Some(&preferred),
        )
        .expect_err("higher event ID loses equal timestamp"),
        SetupPaymentFederationsEventError::Rollback
    );
}

#[test]
fn current_event_survives_clock_rollback_and_static_restore() {
    let keys = Keys::generate();
    let now = Timestamp::from_secs(100_000);
    let boundary = event(
        &keys,
        now.as_secs() + SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS,
        content(&[]),
    );
    let boundary = admit_setup_payment_federations_event(&boundary, keys.public_key(), now, None)
        .expect("boundary event is admitted");
    admit_setup_payment_federations_event(
        boundary.event(),
        keys.public_key(),
        Timestamp::from_secs(now.as_secs() - 1),
        Some(&boundary),
    )
    .expect("same event remains valid after one-second rollback");

    let ordinary = event(&keys, now.as_secs(), content(&[]));
    let ordinary = admit_setup_payment_federations_event(&ordinary, keys.public_key(), now, None)
        .expect("ordinary event is admitted");
    admit_setup_payment_federations_event(
        ordinary.event(),
        keys.public_key(),
        Timestamp::zero(),
        Some(&ordinary),
    )
    .expect("same event remains valid after large clock rollback");
    restore_durably_admitted_setup_payment_federations_event(ordinary.event(), keys.public_key())
        .expect("durably retained event can be statically restored");
}

#[test]
fn content_bound_precedes_event_crypto() {
    let keys = Keys::generate();
    let mut invalid = event(&keys, 1_000, content(&[]));
    invalid.content =
        "x".repeat(fedi_decentralized_domain::SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES + 1);

    assert_eq!(
        admit_setup_payment_federations_event(
            &invalid,
            keys.public_key(),
            Timestamp::from_secs(1_000),
            None,
        )
        .expect_err("oversized content is rejected before invalid event ID"),
        SetupPaymentFederationsEventError::Content(
            fedi_decentralized_domain::SetupPaymentFederationsContentError::ContentTooLarge
        )
    );
}
