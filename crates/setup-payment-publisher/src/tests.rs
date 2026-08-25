//! Tests for production publisher safety and orchestration.

use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use fedimint_core::PeerId;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;
use tempfile::TempDir;

use super::*;

const POLICY_FIXTURE: &str = include_str!("../testdata/valid-policy.json");
const OPERATOR_TEMPLATE: &str = include_str!("../example-policy.json");
const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const RUN_REAL_RELAY_TESTS_ENV: &str = "DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS";

fn public_invite(index: u16) -> String {
    FedimintInviteCode::new(
        SafeUrl::parse(&format!("https://guardian-{index}.example/"))
            .expect("fixture URL is valid"),
        PeerId::from(0),
        FederationId::from_str(&format!("{index:064x}")).expect("fixture federation ID is valid"),
        None,
    )
    .to_string()
}

fn write_policy(temp: &TempDir, content: &str) -> PathBuf {
    let path = temp.path().join("policy.json");
    std::fs::write(&path, content).unwrap();
    path
}

fn valid_content() -> SetupPaymentFederationsContent {
    serde_json::from_str(POLICY_FIXTURE).expect("golden policy parses")
}

fn content_with_invites(invites: Vec<String>) -> SetupPaymentFederationsContent {
    let mut value: serde_json::Value = serde_json::from_str(POLICY_FIXTURE).unwrap();
    value["federations"] = serde_json::json!(invites);
    serde_json::from_value(value).unwrap()
}

fn test_keys() -> Keys {
    Keys::parse(TEST_SECRET).unwrap()
}

fn signed_event(content: &SetupPaymentFederationsContent, created_at: u64) -> Event {
    setup_payment_federations_event_builder(content)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(&test_keys())
        .unwrap()
}

fn signed_opaque_event(content: &str, created_at: u64) -> Event {
    nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(
            fedi_decentralized_nostr::setup_payment_federations::
                SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
        ),
        content,
    )
    .tag(nostr_sdk::Tag::identifier(
        fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
    ))
    .custom_created_at(Timestamp::from_secs(created_at))
    .sign_with_keys(&test_keys())
    .unwrap()
}

fn test_relays() -> Vec<RelayUrl> {
    ["wss://one.example", "wss://two.example"]
        .into_iter()
        .map(|url| RelayUrl::parse(url).unwrap())
        .collect()
}

#[test]
fn complete_policy_fixture_pins_the_publisher_contract() {
    let temp = TempDir::new().unwrap();
    let path = write_policy(&temp, POLICY_FIXTURE);
    let content = read_content(&path).unwrap();
    assert_eq!(
        format!("{}\n", serde_json::to_string(&content).unwrap()),
        POLICY_FIXTURE
    );
}

#[test]
fn operator_template_cannot_reach_signing() {
    let temp = TempDir::new().unwrap();
    let path = write_policy(&temp, OPERATOR_TEMPLATE);
    assert!(read_content(&path).is_err());
}

#[test]
fn manifest_accepts_multiple_federations_through_the_shared_wire_type() {
    let content = content_with_invites(vec![public_invite(1), public_invite(2)]);
    setup_payment_federations_event_builder(&content)
        .expect("two distinct public federation invites are admitted");
}

#[test]
fn manifest_rejects_only_the_injected_unknown_field() {
    let temp = TempDir::new().unwrap();
    let mut content: serde_json::Value = serde_json::from_str(POLICY_FIXTURE).unwrap();
    assert!(
        serde_json::from_value::<SetupPaymentFederationsContent>(content.clone()).is_ok(),
        "base policy must be otherwise valid"
    );
    content["future_field"] = serde_json::json!(true);
    let path = write_policy(&temp, &serde_json::to_string(&content).unwrap());
    assert!(read_content(&path).is_err());
}

#[test]
fn empty_stop_set_requires_explicit_acknowledgement() {
    let content = valid_content();
    assert!(validate_content_for_publication(&content, false).is_err());
    validate_content_for_publication(&content, true).unwrap();
    validate_content_for_publication(&content_with_invites(vec![public_invite(1)]), false).unwrap();
}

#[test]
fn signer_rejects_a_secret_for_another_publisher() {
    let result = sign_event(
        &valid_content(),
        Keys::generate().public_key(),
        Zeroizing::new(TEST_SECRET.to_owned()),
        Timestamp::from_secs(1_700_000_000),
    );
    assert!(result.is_err());
}

#[test]
fn signer_builds_an_event_that_self_admits() {
    let keys = test_keys();
    let event = sign_event(
        &valid_content(),
        keys.public_key(),
        Zeroizing::new(TEST_SECRET.to_owned()),
        Timestamp::from_secs(1_700_000_000),
    )
    .unwrap();
    admit_setup_payment_federations_event(
        &event,
        keys.public_key(),
        Timestamp::from_secs(1_700_000_000),
        None,
    )
    .unwrap();
}

#[test]
fn receipt_round_trip_preserves_the_exact_signed_event() {
    let temp = TempDir::new().unwrap();
    let event = signed_event(&valid_content(), 1_700_000_000);
    let receipt = temp.path().join("event.json");
    write_receipt(&receipt, &event).unwrap();
    assert_eq!(read_receipt(&receipt).unwrap(), event);
    assert!(write_receipt(&receipt, &event).is_err());
}

#[test]
fn receipt_reads_enforce_the_complete_event_bound() {
    let temp = TempDir::new().unwrap();
    let exact = temp.path().join("exact");
    let over = temp.path().join("over");
    std::fs::write(&exact, vec![b'x'; ROLE_FETCHED_EVENT_MAX_BYTES]).unwrap();
    std::fs::write(&over, vec![b'x'; ROLE_FETCHED_EVENT_MAX_BYTES + 1]).unwrap();
    assert_eq!(
        read_file_bounded(&exact, ROLE_FETCHED_EVENT_MAX_BYTES, "test")
            .unwrap()
            .len(),
        ROLE_FETCHED_EVENT_MAX_BYTES
    );
    assert!(read_file_bounded(&over, ROLE_FETCHED_EVENT_MAX_BYTES, "test").is_err());
}

#[test]
fn update_timestamp_always_outranks_the_previous_receipt() {
    let previous = signed_event(&valid_content(), 1_700_000_010);
    let admitted = restore_durably_admitted_setup_payment_federations_event(
        &previous,
        test_keys().public_key(),
    )
    .unwrap();
    assert_eq!(
        PublicationBasis::Previous(admitted)
            .next_timestamp(Timestamp::from_secs(1_700_000_000))
            .unwrap()
            .as_secs(),
        1_700_000_011
    );
}

#[test]
fn replacement_selection_is_order_independent_at_equal_timestamps() {
    let first = signed_event(&valid_content(), 1_700_000_000);
    let second = signed_event(&content_with_invites(vec![public_invite(1)]), 1_700_000_000);
    let expected = if first.id < second.id {
        first.id
    } else {
        second.id
    };
    for events in [
        vec![first.clone(), second.clone()],
        vec![second.clone(), first.clone()],
    ] {
        let latest = latest_address_state(&events, test_keys().public_key(), None).unwrap();
        assert_eq!(latest.event().id, expected);
    }
}

#[test]
fn previous_receipt_rejects_a_newer_relay_event() {
    let previous = signed_event(&valid_content(), 1_700_000_000);
    let newer = signed_event(&valid_content(), 1_700_000_001);
    let basis = PublicationBasis::Previous(
        restore_durably_admitted_setup_payment_federations_event(
            &previous,
            test_keys().public_key(),
        )
        .unwrap(),
    );
    assert!(
        basis
            .validate_relay_high_water(&[newer], test_keys().public_key())
            .is_err()
    );
}

#[test]
fn canonical_readback_rejects_a_newer_selected_event() {
    let published = signed_event(&valid_content(), 1_700_000_000);
    let newer = signed_event(&valid_content(), 1_700_000_001);
    assert!(verify_canonical_selection(&published, &[published.clone(), newer]).is_err());
}

#[test]
fn first_publication_rejects_a_latent_far_future_event() {
    let future = signed_event(&valid_content(), Timestamp::now().as_secs() + 86_401);
    assert!(
        PublicationBasis::First
            .validate_relay_high_water(&[future], test_keys().public_key())
            .is_err()
    );
}

#[test]
fn previous_receipt_rejects_a_latent_far_future_event() {
    let previous = signed_event(&valid_content(), 1_700_000_000);
    let future = signed_event(&valid_content(), Timestamp::now().as_secs() + 86_401);
    let basis = PublicationBasis::Previous(
        restore_durably_admitted_setup_payment_federations_event(
            &previous,
            test_keys().public_key(),
        )
        .unwrap(),
    );
    assert!(
        basis
            .validate_relay_high_water(&[future], test_keys().public_key())
            .is_err()
    );
}

#[test]
fn canonical_readback_rejects_a_latent_far_future_event() {
    let published = signed_event(&valid_content(), Timestamp::now().as_secs());
    let future = signed_event(&valid_content(), Timestamp::now().as_secs() + 86_401);
    assert!(verify_canonical_selection(&published, &[published.clone(), future]).is_err());
}

#[test]
fn newer_future_schema_event_is_an_opaque_high_water_mark() {
    let previous = signed_event(&valid_content(), 1_700_000_000);
    let future_schema =
        signed_opaque_event(r#"{"version":2,"new_required_field":true}"#, 1_700_000_001);
    let basis = PublicationBasis::Previous(
        restore_durably_admitted_setup_payment_federations_event(
            &previous,
            test_keys().public_key(),
        )
        .unwrap(),
    );
    assert!(
        PublicationBasis::First
            .validate_relay_high_water(&[future_schema.clone()], test_keys().public_key())
            .is_err()
    );
    assert!(
        basis
            .validate_relay_high_water(&[future_schema.clone()], test_keys().public_key())
            .is_err()
    );
    assert!(verify_canonical_selection(&previous, &[previous.clone(), future_schema]).is_err());
}

#[tokio::test]
async fn preflight_attempts_every_relay_after_incomplete_queries() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&attempts);
    let result = preflight_relays_with(
        &test_relays(),
        test_keys().public_key(),
        &PublicationBasis::First,
        move |relay, _| {
            let recorded = Arc::clone(&recorded);
            async move {
                recorded.lock().unwrap().push(relay);
                anyhow::bail!("incomplete relay answer")
            }
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn fanout_attempts_every_relay_and_reuses_the_exact_event() {
    let relays = test_relays();
    let event = signed_event(&valid_content(), 1_700_000_000);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&attempts);
    let first_relay = relays[0].clone();
    let result = publish_to_relays_with(&relays, &event, move |relay, candidate| {
        let recorded = Arc::clone(&recorded);
        let first_relay = first_relay.clone();
        async move {
            recorded.lock().unwrap().push(candidate.id);
            ensure!(relay != first_relay, "simulated relay failure");
            Ok(())
        }
    })
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.lock().unwrap().as_slice(), &[event.id, event.id]);
}

#[tokio::test]
async fn receipt_is_written_before_the_first_network_attempt() {
    let temp = TempDir::new().unwrap();
    let receipt = temp.path().join("event.json");
    let event = signed_event(&valid_content(), 1_700_000_000);
    let checked = Arc::new(Mutex::new(false));
    let recorded = Arc::clone(&checked);
    let receipt_for_publish = receipt.clone();
    save_receipt_and_publish_with(
        &receipt,
        &test_relays()[..1],
        &event,
        move |_, candidate| {
            let recorded = Arc::clone(&recorded);
            let receipt = receipt_for_publish.clone();
            async move {
                assert_eq!(read_receipt(&receipt).unwrap(), candidate);
                *recorded.lock().unwrap() = true;
                Ok(())
            }
        },
    )
    .await
    .unwrap();
    assert!(*checked.lock().unwrap());
}

#[tokio::test]
async fn saved_event_can_be_republished_without_key_material() {
    let temp = TempDir::new().unwrap();
    let receipt = temp.path().join("event.json");
    let event = signed_event(&valid_content(), 1_700_000_000);
    write_receipt(&receipt, &event).unwrap();
    let restored = read_receipt(&receipt).unwrap();
    publish_to_relays_with(&test_relays()[..1], &restored, |_, _| async { Ok(()) })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "opt-in leased local defe relay test; never contacts production"]
async fn publishes_and_verifies_on_a_leased_defe_relay() {
    if std::env::var(RUN_REAL_RELAY_TESTS_ENV).as_deref() != Ok("1") {
        return;
    }
    let relay = std::env::var("DEV_DEFE_NOSTR_RELAY_URL")
        .expect("run through defe-cli --request-relay=exclusive");
    let relay = RelayUrl::parse(&relay).unwrap();
    ensure_local_defe_relay(&relay).unwrap();
    let event = signed_event(&valid_content(), Timestamp::now().as_secs());
    publish_and_read_back(&relay, &event).await.unwrap();
}

#[test]
fn defe_relay_guard_requires_insecure_loopback() {
    ensure_local_defe_relay(&RelayUrl::parse("ws://127.0.0.1:7777").unwrap()).unwrap();
    ensure_local_defe_relay(&RelayUrl::parse("ws://[::1]:7777").unwrap()).unwrap();
    assert!(ensure_local_defe_relay(&RelayUrl::parse("wss://127.0.0.1:7777").unwrap()).is_err());
    assert!(ensure_local_defe_relay(&RelayUrl::parse("ws://10.0.0.1:7777").unwrap()).is_err());
    assert!(ensure_local_defe_relay(&RelayUrl::parse("wss://relay.example").unwrap()).is_err());
}

fn ensure_local_defe_relay(relay: &RelayUrl) -> anyhow::Result<()> {
    use nostr_sdk::nostr::types::url::Host;

    let url: &nostr_sdk::Url = relay.into();
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    ensure!(
        url.scheme() == "ws" && loopback,
        "defe integration test requires an insecure loopback relay"
    );
    Ok(())
}
