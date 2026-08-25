//! Tests for setup-payment federation publication content.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

use fedimint_core::PeerId;
use fedimint_core::config::FederationId as FedimintFederationId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;

use super::*;

fn invite_with_secret(url: &str, api_secret: Option<String>) -> String {
    FedimintInviteCode::new(
        SafeUrl::parse(url).expect("test URL is valid"),
        PeerId::from(0),
        FedimintFederationId::dummy(),
        api_secret,
    )
    .to_string()
}

fn invite(url: &str) -> String {
    invite_with_secret(url, None)
}

fn invite_for_id(index: usize) -> String {
    FedimintInviteCode::new(
        SafeUrl::parse(&format!("https://{index}.example/")).expect("test URL is valid"),
        PeerId::from(0),
        format!("{index:064x}")
            .parse()
            .expect("test federation ID is valid"),
        None,
    )
    .to_string()
}

fn content(invites: Vec<String>) -> Vec<u8> {
    content_with_min_fee_ppm(invites, DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM)
}

fn content_with_min_fee_ppm(invites: Vec<String>, min_fee_ppm: u64) -> Vec<u8> {
    serde_json::to_vec(&SetupPaymentFederationsContent {
        version: ProtocolV1,
        fman_version: "0.1.0".parse().expect("valid FMan version"),
        federations: invites.into_iter().map(InviteCode).collect(),
        telemetry_registration_url: Url(
            "https://push.fedi.example/v1/telemetry/registrations".to_owned()
        ),
        min_fee_ppm,
    })
    .expect("test content serializes")
}

#[test]
fn wire_shape_includes_fman_version_and_invite_array() {
    let invite = invite("https://one.example/");
    let encoded = content(vec![invite.clone()]);

    assert_eq!(
        String::from_utf8(encoded).expect("JSON is UTF-8"),
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":["{invite}"],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","min_fee_ppm":1500}}"#
        )
    );
}

#[test]
fn omitted_min_fee_ppm_defaults_to_the_published_floor() {
    let invite = invite("https://one.example/");
    // Exactly what Fedi publishes today, with no `min_fee_ppm` at all: an
    // un-upgraded publication must still carry the 0.15% floor, not zero.
    let raw = format!(
        r#"{{"version":1,"fman_version":"0.1.0","federations":["{invite}"],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}}"#
    );

    let admitted = AdmittedSetupPaymentFederations::parse(raw.as_bytes())
        .expect("content without the field is admitted");

    assert_eq!(admitted.min_fee_ppm(), 1_500);
    assert_eq!(admitted.min_fee_ppm(), DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM);
}

#[test]
fn admits_a_published_min_fee_ppm_up_to_the_payer_cap() {
    for min_fee_ppm in [
        0,
        1,
        DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM,
        SETUP_PAYMENT_MAX_MIN_FEE_PPM,
    ] {
        let admitted =
            AdmittedSetupPaymentFederations::parse(&content_with_min_fee_ppm(vec![], min_fee_ppm))
                .expect("an in-range minimum is admitted");
        assert_eq!(admitted.min_fee_ppm(), min_fee_ppm);
    }
}

#[test]
fn rejects_a_published_min_fee_ppm_above_the_payer_cap() {
    // A floor above the payer's 210,000-ppm ceiling would leave no proposable
    // rate, so a publisher mistake is refused rather than silently disabling
    // every fee proposal.
    let error = AdmittedSetupPaymentFederations::parse(&content_with_min_fee_ppm(
        vec![],
        SETUP_PAYMENT_MAX_MIN_FEE_PPM + 1,
    ))
    .expect_err("a minimum above the payer cap is rejected");

    assert_eq!(error, SetupPaymentFederationsContentError::MinFeePpmTooHigh);
}

#[test]
fn admits_only_credential_free_https_telemetry_registration_urls() {
    for invalid in [
        "http://push.fedi.example/v1/telemetry/registrations",
        "https://user:pass@push.fedi.example/v1/telemetry/registrations",
        "https://push.fedi.example/v1/telemetry/registrations?secret=value",
        "https://push.fedi.example/v1/telemetry/registrations#fragment",
        "not-a-url",
    ] {
        let raw = serde_json::to_vec(&SetupPaymentFederationsContent {
            version: ProtocolV1,
            fman_version: "0.1.0".parse().unwrap(),
            federations: Vec::new(),
            telemetry_registration_url: Url(invalid.to_owned()),
            min_fee_ppm: DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM,
        })
        .unwrap();
        assert_eq!(
            AdmittedSetupPaymentFederations::parse(&raw).unwrap_err(),
            SetupPaymentFederationsContentError::InvalidTelemetryRegistrationUrl
        );
    }
}

#[test]
fn protocol_bounds_are_pinned() {
    assert_eq!(SETUP_PAYMENT_FEDERATION_INVITE_MAX_BYTES, 16 * 1024);
    assert_eq!(SETUP_PAYMENT_FEDERATIONS_MAX_COUNT, 16);
    assert_eq!(SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES, 128 * 1024);
    assert_eq!(DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM, 1_500);
    assert_eq!(SETUP_PAYMENT_MAX_MIN_FEE_PPM, 210_000);
}

#[test]
fn admits_invite_and_derives_federation_id() {
    let invite = invite("https://one.example/");
    let admitted = AdmittedSetupPaymentFederations::parse(&content(vec![invite.clone()]))
        .expect("valid content is admitted");

    let federation_id = FederationId(FedimintFederationId::dummy().to_string());
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted.invite(&federation_id), Some(&InviteCode(invite)));
}

#[test]
fn admits_empty_set() {
    let admitted =
        AdmittedSetupPaymentFederations::parse(&content(vec![])).expect("empty set is valid");
    assert!(admitted.is_empty());
    assert_eq!(admitted.fman_version().to_string(), "0.1.0");
}

#[test]
fn rejects_duplicate_derived_federation_id() {
    let error = AdmittedSetupPaymentFederations::parse(&content(vec![
        invite("https://one.example/"),
        invite("https://two.example/"),
    ]))
    .expect_err("different invites for one federation are duplicates");

    assert_eq!(
        error,
        SetupPaymentFederationsContentError::DuplicateFederation
    );
}

#[test]
fn rejects_invalid_and_oversized_invites() {
    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&content(vec!["invalid".to_owned()]))
            .expect_err("invalid invite is rejected"),
        SetupPaymentFederationsContentError::InvalidInvite
    );
    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&content(vec![
            "x".repeat(SETUP_PAYMENT_FEDERATION_INVITE_MAX_BYTES + 1)
        ]))
        .expect_err("oversized invite is rejected before parsing"),
        SetupPaymentFederationsContentError::InviteTooLarge
    );
}

#[test]
fn rejects_secret_bearing_invite() {
    let secret_invite = invite_with_secret(
        "https://one.example/",
        Some("private-api-password".to_owned()),
    );
    assert!(
        FedimintInviteCode::from_str(&secret_invite)
            .expect("test invite parses")
            .api_secret()
            .is_some()
    );

    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&content(vec![secret_invite]))
            .expect_err("bearer secret cannot be published"),
        SetupPaymentFederationsContentError::SecretInvite
    );
}

#[test]
fn admitted_set_equality_ignores_publication_order() {
    let first = invite("https://one.example/");
    let second = FedimintInviteCode::new(
        SafeUrl::parse("https://two.example/").expect("test URL is valid"),
        PeerId::from(0),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .expect("test federation ID is valid"),
        None,
    )
    .to_string();

    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&content(vec![first.clone(), second.clone()]))
            .expect("first ordering is valid"),
        AdmittedSetupPaymentFederations::parse(&content(vec![second, first]))
            .expect("second ordering is valid")
    );
}
#[test]
fn rejects_excessive_entries_before_invite_parsing() {
    let error = AdmittedSetupPaymentFederations::parse(&content(vec![
        "invalid".to_owned();
        SETUP_PAYMENT_FEDERATIONS_MAX_COUNT
            + 1
    ]))
    .expect_err("entry bound is checked first");

    assert_eq!(
        error,
        SetupPaymentFederationsContentError::TooManyFederations
    );
}

#[test]
fn accepts_exact_content_and_entry_bounds() {
    let invites = (0..SETUP_PAYMENT_FEDERATIONS_MAX_COUNT)
        .map(invite_for_id)
        .collect();
    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&content(invites))
            .expect("exact entry limit is valid")
            .len(),
        SETUP_PAYMENT_FEDERATIONS_MAX_COUNT
    );

    let mut padded = content(vec![]);
    padded.resize(SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES, b' ');
    assert!(
        AdmittedSetupPaymentFederations::parse(&padded)
            .expect("exact content byte limit is valid")
            .is_empty()
    );
}

#[test]
fn rejects_oversized_malformed_unknown_and_duplicate_fields() {
    assert_eq!(
        AdmittedSetupPaymentFederations::parse(&vec![
            b' ';
            SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES
                + 1
        ])
        .expect_err("oversized content is rejected before parsing"),
        SetupPaymentFederationsContentError::ContentTooLarge
    );
    for malformed in [
        br#"{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","extra":true}"#.as_slice(),
        br#"{"version":1,"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#.as_slice(),
        br#"{"version":2,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#.as_slice(),
        br#"{"version":1,"fman_version":"0.1.0","federations":"not-an-array","telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#.as_slice(),
        br#"{"version":1,"fman_version":"0.1.0","federations":[]}"#.as_slice(),
        br#"{"version":1,"federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#.as_slice(),
        br#"{"version":1,"fman_version":"latest","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#.as_slice(),
    ] {
        assert_eq!(
            AdmittedSetupPaymentFederations::parse(malformed)
                .expect_err("malformed content is rejected"),
            SetupPaymentFederationsContentError::MalformedContent
        );
    }
}
