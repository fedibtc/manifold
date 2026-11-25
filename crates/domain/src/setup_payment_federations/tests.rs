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
        federations: invites.into_iter().map(InviteCode).collect(),
        telemetry_registration_url: Url(
            "https://push.fedi.example/v1/telemetry/registrations".to_owned()
        ),
        min_fee_ppm,
        legacy_features: crate::FeaturePolicy {
            mg_version: "0.1.0".parse().expect("valid MG version"),
            min_uptime_percent: None,
            max_guardian_offline_secs: None,
            support_nostr_pubkey: None,
            seat_memory_budget_mib: None,
            seat_cpu_iterations_per_sec: None,
            seat_monthly_data_gb: None,
            seat_disk_gb: None,
            seat_disk_gb_btrfs: None,
            max_seats_per_mg: None,
            seat_memory_budget_mib_v2: None,
            seat_memory_budget_mib_v3: None,
            max_seats_per_mg_v2: None,
            max_pressure_percent: None,
            guardian_transfer_v2: None,
            guardian_link: None,
            aleph_quiescence: None,
            aleph_quiescence_park_secs: None,
            iroh_legacy_timeouts: None,
            walletv2_legacy_vote_delay: None,
            min_mg_version: None,
            recommended_min_mg_version: None,
        },
    })
    .expect("test content serializes")
}

#[test]
fn wire_shape_includes_mg_version_and_invite_array() {
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

fn content_with_min_uptime(field: &str) -> String {
    format!(
        r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
    )
}

#[test]
fn omitted_min_uptime_defaults_to_70_90_93_percent() {
    let admitted = AdmittedSetupPaymentFederations::parse(content_with_min_uptime("").as_bytes())
        .expect("content without the field is admitted");
    assert_eq!(
        admitted.features().min_uptime_percent(),
        MinUptimePercent {
            day: 70,
            week: 90,
            month: 93
        }
    );
}

/// A publication may set one window; the others keep their defaults, and a
/// window this consumer does not know is ignored.
#[test]
fn a_published_window_overrides_only_itself() {
    let admitted = AdmittedSetupPaymentFederations::parse(
        content_with_min_uptime(r#","min_uptime_percent":{"30d":0,"365d":99}"#).as_bytes(),
    )
    .expect("a partial minimum is admitted");
    assert_eq!(
        admitted.features().min_uptime_percent(),
        MinUptimePercent {
            day: 70,
            week: 90,
            month: 0
        }
    );
    let content: SetupPaymentFederationsContent = serde_json::from_str(&content_with_min_uptime(
        r#","min_uptime_percent":{"1d":75,"7d":85,"30d":100}"#,
    ))
    .unwrap();
    assert_eq!(
        content.legacy_features.min_uptime_percent,
        Some(MinUptimePercent {
            day: 75,
            week: 85,
            month: 100
        })
    );
}

#[test]
fn rejects_a_min_uptime_above_one_hundred_percent_in_any_window() {
    for window in ["1d", "7d", "30d"] {
        let error = AdmittedSetupPaymentFederations::parse(
            content_with_min_uptime(&format!(r#","min_uptime_percent":{{"{window}":101}}"#))
                .as_bytes(),
        )
        .expect_err("a minimum above 100% is rejected");
        assert_eq!(error, SetupPaymentFederationsContentError::MinUptimeTooHigh);
    }
}

fn content_with_support(support_nostr_pubkey: &str) -> String {
    format!(
        r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","support_nostr_pubkey":"{support_nostr_pubkey}"}}"#
    )
}

#[test]
fn support_nostr_pubkey_is_optional_and_admitted_when_valid() {
    // Secret key 7: a fixed, valid x-only key.
    let hex = "5cbdf0646e5db4eaa398f365f2ea7a0e3d419b7e0330e39ce92bddedcac4f9bc";
    let admitted = AdmittedSetupPaymentFederations::parse(content_with_support(hex).as_bytes())
        .expect("a valid support key is admitted");
    assert_eq!(
        admitted
            .features()
            .support_nostr_pubkey()
            .map(nostr::PublicKey::to_hex),
        Some(hex.to_owned())
    );

    let without = AdmittedSetupPaymentFederations::parse(&content(vec![])).unwrap();
    assert_eq!(without.features().support_nostr_pubkey(), None);
    // Absent stays absent on the wire, so today's publications re-serialize
    // byte for byte.
    assert!(
        !String::from_utf8(content(vec![]))
            .unwrap()
            .contains("support")
    );
}

#[test]
fn rejects_a_support_nostr_pubkey_that_is_not_a_key() {
    assert!(
        AdmittedSetupPaymentFederations::parse(content_with_support("not-a-key").as_bytes())
            .is_err()
    );
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
            federations: Vec::new(),
            telemetry_registration_url: Url(invalid.to_owned()),
            min_fee_ppm: DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM,
            legacy_features: crate::FeaturePolicy {
                mg_version: "0.1.0".parse().unwrap(),
                min_uptime_percent: None,
                max_guardian_offline_secs: None,
                support_nostr_pubkey: None,
                seat_memory_budget_mib: None,
                seat_cpu_iterations_per_sec: None,
                seat_monthly_data_gb: None,
                seat_disk_gb: None,
                seat_disk_gb_btrfs: None,
                max_seats_per_mg: None,
                seat_memory_budget_mib_v2: None,
                seat_memory_budget_mib_v3: None,
                max_seats_per_mg_v2: None,
                max_pressure_percent: None,
                guardian_transfer_v2: None,
                guardian_link: None,
                aleph_quiescence: None,
                aleph_quiescence_park_secs: None,
                iroh_legacy_timeouts: None,
                walletv2_legacy_vote_delay: None,
                min_mg_version: None,
                recommended_min_mg_version: None,
            },
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
    assert_eq!(admitted.features().mg_version().to_string(), "0.1.0");
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
fn rejects_oversized_malformed_versions_and_duplicate_fields() {
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

fn parse_with_fields(
    fields: &str,
) -> Result<AdmittedSetupPaymentFederations, SetupPaymentFederationsContentError> {
    AdmittedSetupPaymentFederations::parse(
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{fields}}}"#
        )
        .as_bytes(),
    )
}

#[test]
fn seat_memory_budget_comes_from_v3_only_and_every_field_is_bounded() {
    let budget = |fields: &str| {
        parse_with_fields(fields)
            .unwrap()
            .features()
            .seat_memory_budget_mib()
    };
    // Written out rather than taken from the constant, so a changed default
    // fails here.
    assert_eq!(budget(""), 1_024);
    // The fields for older MGs do not reach this release, not even as a
    // fallback when v3 is absent.
    assert_eq!(budget(r#","seat_memory_budget_mib_v2":750"#), 1_024);
    assert_eq!(
        budget(r#","seat_memory_budget_mib":2048,"seat_memory_budget_mib_v2":750"#),
        1_024
    );
    assert_eq!(
        budget(
            r#","seat_memory_budget_mib":2048,"seat_memory_budget_mib_v2":1229,"seat_memory_budget_mib_v3":750"#
        ),
        750
    );
    for mib in [256, 16_384] {
        assert_eq!(
            budget(&format!(r#","seat_memory_budget_mib_v3":{mib}"#)),
            mib
        );
    }

    for field in [
        "seat_memory_budget_mib",
        "seat_memory_budget_mib_v2",
        "seat_memory_budget_mib_v3",
    ] {
        for mib in ["0", "255", "16385"] {
            assert_eq!(
                parse_with_fields(&format!(r#","{field}":{mib}"#)),
                Err(SetupPaymentFederationsContentError::SeatMemoryBudgetOutOfRange),
                "{field}={mib}"
            );
        }
    }
    // An out-of-range v3 is refused even beside in-range older fields.
    assert_eq!(
        parse_with_fields(
            r#","seat_memory_budget_mib":1024,"seat_memory_budget_mib_v2":1024,"seat_memory_budget_mib_v3":255"#
        ),
        Err(SetupPaymentFederationsContentError::SeatMemoryBudgetOutOfRange)
    );
}

#[test]
fn seat_monthly_data_defaults_to_65_and_is_bounded() {
    let raw = |gb: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","seat_monthly_data_gb":{gb}}}"#
        )
    };
    let absent = AdmittedSetupPaymentFederations::parse(&content(vec![])).unwrap();
    assert_eq!(absent.features().seat_monthly_data_gb().get(), 65);
    assert_eq!(DEFAULT_SEAT_MONTHLY_DATA_GB, 65);
    assert!(SETUP_PAYMENT_SEAT_MONTHLY_DATA_GB_RANGE.contains(&DEFAULT_SEAT_MONTHLY_DATA_GB));
    let published = AdmittedSetupPaymentFederations::parse(raw("60").as_bytes()).unwrap();
    assert_eq!(published.features().seat_monthly_data_gb().get(), 60);
    for gb in [1, 60, 10_000] {
        let admitted = AdmittedSetupPaymentFederations::parse(raw(&gb.to_string()).as_bytes())
            .expect("an in-range figure is admitted");
        assert_eq!(admitted.features().seat_monthly_data_gb().get(), gb);
    }
    for gb in ["0", "10001"] {
        assert_eq!(
            AdmittedSetupPaymentFederations::parse(raw(gb).as_bytes()),
            Err(SetupPaymentFederationsContentError::SeatMonthlyDataOutOfRange),
            "{gb}"
        );
    }
}

/// Like the data figure, an absent disk figure takes the built-in default.
#[test]
fn seat_disk_defaults_to_80_and_is_bounded() {
    let raw = |gb: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","seat_disk_gb":{gb}}}"#
        )
    };
    let absent = AdmittedSetupPaymentFederations::parse(&content(vec![])).unwrap();
    assert_eq!(absent.features().seat_disk_gb().get(), 80);
    assert_eq!(DEFAULT_SEAT_DISK_GB, 80);
    for gb in [1, 75, 100_000] {
        let admitted = AdmittedSetupPaymentFederations::parse(raw(&gb.to_string()).as_bytes())
            .expect("an in-range figure is admitted");
        assert_eq!(admitted.features().seat_disk_gb().get(), gb);
    }
    for gb in ["0", "100001"] {
        assert_eq!(
            AdmittedSetupPaymentFederations::parse(raw(gb).as_bytes()),
            Err(SetupPaymentFederationsContentError::SeatDiskOutOfRange),
            "{gb}"
        );
    }
}

/// The btrfs figure has its own default, which the general figure does not
/// move, and the same bounds.
#[test]
fn seat_disk_on_btrfs_defaults_to_160_independently_and_is_bounded() {
    let raw = |fields: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{fields}}}"#
        )
    };
    let parse = |fields: &str| AdmittedSetupPaymentFederations::parse(raw(fields).as_bytes());
    assert_eq!(DEFAULT_SEAT_DISK_GB_BTRFS, 160);
    let general_only = parse(r#","seat_disk_gb":100"#).unwrap();
    assert_eq!(general_only.features().seat_disk_gb().get(), 100);
    assert_eq!(general_only.features().seat_disk_gb_btrfs().get(), 160);
    let btrfs_only = parse(r#","seat_disk_gb_btrfs":240"#).unwrap();
    assert_eq!(btrfs_only.features().seat_disk_gb().get(), 80);
    assert_eq!(btrfs_only.features().seat_disk_gb_btrfs().get(), 240);
    for gb in ["0", "100001"] {
        assert_eq!(
            parse(&format!(r#","seat_disk_gb_btrfs":{gb}"#)),
            Err(SetupPaymentFederationsContentError::SeatDiskOutOfRange),
            "{gb}"
        );
    }
}

#[test]
fn seat_cpu_budget_is_optional_with_no_default_and_bounded() {
    let raw = |budget: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","seat_cpu_iterations_per_sec":{budget}}}"#
        )
    };
    // Absent means no CPU limit, not a default budget.
    let absent = AdmittedSetupPaymentFederations::parse(&content(vec![]))
        .expect("content without a CPU budget is admitted");
    assert_eq!(absent.features().seat_cpu_iterations_per_sec(), None);

    for budget in [1, 40, 100_000] {
        let admitted = AdmittedSetupPaymentFederations::parse(raw(&budget.to_string()).as_bytes())
            .expect("an in-range CPU budget is admitted");
        assert_eq!(
            admitted
                .features()
                .seat_cpu_iterations_per_sec()
                .map(NonZeroU32::get),
            Some(budget)
        );
    }
    for budget in ["0", "100001", "-1", "0.5", "null"] {
        let parsed = AdmittedSetupPaymentFederations::parse(raw(budget).as_bytes());
        match budget {
            // `null` is the same as absent.
            "null" => assert_eq!(
                parsed.unwrap().features().seat_cpu_iterations_per_sec(),
                None
            ),
            "-1" | "0.5" => assert_eq!(
                parsed,
                Err(SetupPaymentFederationsContentError::MalformedContent),
                "{budget}"
            ),
            _ => assert_eq!(
                parsed,
                Err(SetupPaymentFederationsContentError::SeatCpuBudgetOutOfRange),
                "{budget}"
            ),
        }
    }
}

#[test]
fn max_seats_per_mg_comes_from_v2_only_and_both_fields_are_bounded() {
    let seats = |fields: &str| {
        parse_with_fields(fields)
            .unwrap()
            .features()
            .max_seats_per_mg()
    };
    assert_eq!(seats(""), 12);
    assert_eq!(seats(r#","max_seats_per_mg":4"#), 12);
    assert_eq!(
        seats(r#","max_seats_per_mg":4,"max_seats_per_mg_v2":20"#),
        20
    );
    for limit in [0, SETUP_PAYMENT_MAX_SEATS_PER_MG] {
        assert_eq!(seats(&format!(r#","max_seats_per_mg_v2":{limit}"#)), limit);
    }

    let too_high = SETUP_PAYMENT_MAX_SEATS_PER_MG + 1;
    for field in ["max_seats_per_mg", "max_seats_per_mg_v2"] {
        assert_eq!(
            parse_with_fields(&format!(r#","{field}":{too_high}"#)),
            Err(SetupPaymentFederationsContentError::MaxSeatsPerMgTooHigh),
            "{field}"
        );
    }
}

#[test]
fn omitted_max_pressure_defaults_per_window_and_must_be_bounded() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let parse = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    let windows = |six_hours, three_and_a_half_days| MaxPressureWindows {
        six_hours,
        three_and_a_half_days,
    };
    let defaults = MaxPressurePercent {
        cpu: windows(15, 10),
        io: windows(25, 15),
        memory: windows(70, 70),
    };
    assert_eq!(MaxPressurePercent::default(), defaults);
    assert_eq!(
        parse("").unwrap().features().max_pressure_percent(),
        defaults
    );
    assert!(
        !String::from_utf8(content(vec![]))
            .unwrap()
            .contains("max_pressure_percent")
    );
    assert_eq!(
        parse(
            r#","max_pressure_percent":{"cpu":{"6h":30,"3.5d":20},"io":{"6h":40,"3.5d":25},"memory":{"6h":50,"3.5d":45}}"#
        )
        .unwrap()
        .features().max_pressure_percent(),
        MaxPressurePercent {
            cpu: windows(30, 20),
            io: windows(40, 25),
            memory: windows(50, 45),
        }
    );
    // An omitted window or resource takes its own default, which differs by
    // resource, and an unknown window is ignored, as for min_uptime_percent.
    assert_eq!(
        parse(r#","max_pressure_percent":{"io":{"3.5d":8,"1h":1}}"#)
            .unwrap()
            .features()
            .max_pressure_percent(),
        MaxPressurePercent {
            cpu: windows(15, 10),
            io: windows(25, 8),
            memory: windows(70, 70),
        }
    );
    assert_eq!(
        parse(r#","max_pressure_percent":{"memory":{"6h":40}}"#)
            .unwrap()
            .features()
            .max_pressure_percent(),
        MaxPressurePercent {
            memory: windows(40, 70),
            ..defaults
        }
    );
    assert_eq!(
        parse(r#","max_pressure_percent":{}"#)
            .unwrap()
            .features()
            .max_pressure_percent(),
        defaults
    );
    // Windows are independent, and both ends of the range are admitted.
    assert!(
        parse(r#","max_pressure_percent":{"cpu":{"6h":1,"3.5d":100},"io":{"6h":100,"3.5d":1}}"#)
            .is_ok()
    );
    // Zero would close every MG; a share of time cannot exceed 100%. Any one
    // window being wrong refuses the publication.
    for field in [
        r#","max_pressure_percent":{"cpu":{"6h":0}}"#,
        r#","max_pressure_percent":{"cpu":{"3.5d":0}}"#,
        r#","max_pressure_percent":{"io":{"6h":101}}"#,
        r#","max_pressure_percent":{"io":{"3.5d":101}}"#,
        r#","max_pressure_percent":{"memory":{"6h":0}}"#,
        r#","max_pressure_percent":{"memory":{"3.5d":101}}"#,
    ] {
        assert_eq!(
            parse(field),
            Err(SetupPaymentFederationsContentError::InvalidMaxPressure),
            "{field}"
        );
    }
}

#[test]
fn unset_seat_limits_stay_off_the_wire() {
    // Consumers built before unknown fields were admitted refuse a
    // publication that carries one, so unset limits must not serialize.
    let encoded = String::from_utf8(content(vec![])).unwrap();
    assert!(!encoded.contains("seat_memory_budget_mib"));
    assert!(!encoded.contains("seat_cpu_iterations_per_sec"));
    assert!(!encoded.contains("seat_monthly_data_gb"));
    assert!(!encoded.contains("max_seats_per_mg"));
    assert!(!encoded.contains("guardian_transfer_v2"));
    assert!(!encoded.contains("min_mg_version"));
    assert!(!encoded.contains("recommended_min_mg_version"));
}

/// A minimum is optional (absent or null), compares by SemVer precedence, may not exceed the latest
/// release the same event names, and a value that is not a version rejects
/// the whole event, as any other malformed field does.
#[test]
fn min_mg_version_is_optional_semver_and_at_most_the_latest() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.10.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert_eq!(admitted("").unwrap().features().min_mg_version(), None);
    // 0.9.0 sorts above 0.10.0 as text but below it as a version.
    assert_eq!(
        admitted(r#","min_mg_version":"0.9.0""#)
            .unwrap()
            .features()
            .min_mg_version()
            .map(ToString::to_string),
        Some("0.9.0".to_owned())
    );
    assert!(admitted(r#","min_mg_version":"0.10.0""#).is_ok());
    // Build metadata has no SemVer precedence: 0.10.0+build is not above 0.10.0.
    // Equality stays exact, so a re-published build still reads as a change.
    let with_build = admitted(r#","min_mg_version":"0.10.0+build.7""#).unwrap();
    assert_ne!(
        with_build.features().min_mg_version(),
        admitted(r#","min_mg_version":"0.10.0""#)
            .unwrap()
            .features()
            .min_mg_version()
    );
    // An explicit null is the same as an absent field: no minimum.
    assert_eq!(
        admitted(r#","min_mg_version":null"#)
            .unwrap()
            .features()
            .min_mg_version(),
        None
    );
    assert_eq!(
        admitted(r#","min_mg_version":"0.10.1""#),
        Err(SetupPaymentFederationsContentError::MinMgVersionAboveLatest)
    );
    for malformed in [r#""0.9""#, r#""v0.9.0""#, r#""""#, "9"] {
        assert_eq!(
            admitted(&format!(r#","min_mg_version":{malformed}"#)),
            Err(SetupPaymentFederationsContentError::MalformedContent),
            "{malformed}"
        );
    }
}

/// A recommendation is optional like the minimum, independent of it, and
/// may not exceed the latest release either.
#[test]
fn recommended_min_mg_version_is_optional_and_at_most_the_latest() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.10.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert_eq!(
        admitted("")
            .unwrap()
            .features()
            .recommended_min_mg_version(),
        None
    );
    let both =
        admitted(r#","min_mg_version":"0.2.0","recommended_min_mg_version":"0.9.0""#).unwrap();
    assert_eq!(
        both.features()
            .recommended_min_mg_version()
            .map(ToString::to_string),
        Some("0.9.0".to_owned())
    );
    assert_eq!(
        both.features().min_mg_version().map(ToString::to_string),
        Some("0.2.0".to_owned())
    );
    assert_eq!(
        admitted(r#","recommended_min_mg_version":"0.10.1""#),
        Err(SetupPaymentFederationsContentError::RecommendedMinMgVersionAboveLatest)
    );
    assert_eq!(
        admitted(r#","recommended_min_mg_version":"0.9""#),
        Err(SetupPaymentFederationsContentError::MalformedContent)
    );
}

#[test]
fn guardian_transfer_v2_is_absent_unless_the_policy_sets_it() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert_eq!(
        admitted("").unwrap().features().guardian_transfer_v2(),
        None
    );
    assert_eq!(
        admitted(r#","guardian_transfer_v2":false"#)
            .unwrap()
            .features()
            .guardian_transfer_v2(),
        Some(false)
    );
    assert_eq!(
        admitted(r#","guardian_transfer_v2":true"#)
            .unwrap()
            .features()
            .guardian_transfer_v2(),
        Some(true)
    );
    assert_eq!(
        admitted(r#","guardian_transfer_v2":"yes""#),
        Err(SetupPaymentFederationsContentError::MalformedContent)
    );
}

#[test]
fn aleph_quiescence_is_off_unless_the_policy_turns_it_on() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert!(!admitted("").unwrap().features().aleph_quiescence());
    assert!(
        !admitted(r#","aleph_quiescence":false"#)
            .unwrap()
            .features()
            .aleph_quiescence()
    );
    assert!(
        admitted(r#","aleph_quiescence":true"#)
            .unwrap()
            .features()
            .aleph_quiescence()
    );
    assert_eq!(
        admitted(r#","aleph_quiescence":"yes""#),
        Err(SetupPaymentFederationsContentError::MalformedContent)
    );
}

#[test]
fn aleph_quiescence_park_secs_is_optional_and_bounded() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert_eq!(
        admitted("")
            .unwrap()
            .features()
            .aleph_quiescence_park_secs(),
        None
    );
    assert_eq!(
        admitted(r#","aleph_quiescence_park_secs":14400"#)
            .unwrap()
            .features()
            .aleph_quiescence_park_secs(),
        Some(14_400)
    );
    assert_eq!(
        admitted(r#","aleph_quiescence_park_secs":59"#),
        Err(SetupPaymentFederationsContentError::AlephQuiescenceParkSecsOutOfRange)
    );
    assert_eq!(
        admitted(r#","aleph_quiescence_park_secs":86401"#),
        Err(SetupPaymentFederationsContentError::AlephQuiescenceParkSecsOutOfRange)
    );
}

#[test]
fn fedimintd_kill_switches_are_off_unless_the_policy_turns_each_on() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let switches = |field: &str| {
        AdmittedSetupPaymentFederations::parse(raw(field).as_bytes()).map(|admitted| {
            let features = admitted.features();
            (
                features.iroh_legacy_timeouts(),
                features.walletv2_legacy_vote_delay(),
            )
        })
    };
    assert_eq!(switches(""), Ok((false, false)));
    assert_eq!(
        switches(r#","iroh_legacy_timeouts":true"#),
        Ok((true, false))
    );
    assert_eq!(
        switches(r#","walletv2_legacy_vote_delay":true"#),
        Ok((false, true))
    );
    assert_eq!(
        switches(r#","iroh_legacy_timeouts":false,"walletv2_legacy_vote_delay":false"#),
        Ok((false, false))
    );
    assert_eq!(
        switches(r#","iroh_legacy_timeouts":1"#),
        Err(SetupPaymentFederationsContentError::MalformedContent)
    );
}

#[test]
fn guardian_link_is_absent_unless_the_policy_sets_it() {
    let raw = |field: &str| {
        format!(
            r#"{{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"{field}}}"#
        )
    };
    let admitted = |field: &str| AdmittedSetupPaymentFederations::parse(raw(field).as_bytes());
    assert_eq!(admitted("").unwrap().features().guardian_link(), None);
    assert_eq!(
        admitted(r#","guardian_link":false"#)
            .unwrap()
            .features()
            .guardian_link(),
        Some(false)
    );
    assert_eq!(
        admitted(r#","guardian_link":true"#)
            .unwrap()
            .features()
            .guardian_link(),
        Some(true)
    );
    assert_eq!(
        admitted(r#","guardian_link":"yes""#),
        Err(SetupPaymentFederationsContentError::MalformedContent)
    );
}

#[test]
fn guardian_absence_policy_defaults_and_round_trips() {
    let old = content_with_min_uptime("");
    let admitted = AdmittedSetupPaymentFederations::parse(old.as_bytes()).unwrap();
    assert_eq!(admitted.features().max_guardian_offline_secs(), 86_400);
    for seconds in [1, 3_600, 172_800] {
        let raw = content_with_min_uptime(&format!(",\"max_guardian_offline_secs\":{seconds}"));
        let parsed: SetupPaymentFederationsContent = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            parsed.legacy_features.max_guardian_offline_secs,
            Some(seconds)
        );
        let encoded = serde_json::to_vec(&parsed).unwrap();
        assert_eq!(
            AdmittedSetupPaymentFederations::parse(&encoded)
                .unwrap()
                .features()
                .max_guardian_offline_secs(),
            seconds
        );
    }
}

#[test]
fn guardian_absence_policy_rejects_invalid_durations() {
    for value in ["0", "-1", "1.5", "\"86400\"", "18446744073709551616"] {
        let raw = content_with_min_uptime(&format!(",\"max_guardian_offline_secs\":{value}"));
        assert!(AdmittedSetupPaymentFederations::parse(raw.as_bytes()).is_err());
    }
}
