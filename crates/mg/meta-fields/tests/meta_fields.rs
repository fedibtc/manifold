//! Compiled metadata-policy dispatch tests.

use fedi_decentralized_service_manifold_guardian::{
    FEDERATION_ICON_URL_META_FIELD_KEY, FEDERATION_METADATA_NAME_MAX_BYTES,
    FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES, FEDERATION_NAME_META_FIELD_KEY,
    GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY, GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY, MetaFieldKey,
    MetaFieldValue, TERMS_OF_SERVICE_URL_META_FIELD_KEY, WELCOME_MESSAGE_META_FIELD_KEY,
};

use super::*;

#[test]
fn compiled_dispatch_accepts_every_served_key_through_wire_wrappers() {
    for (key, value) in [
        (GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY, "5000"),
        (FEDERATION_NAME_META_FIELD_KEY, "Federation"),
        (
            FEDERATION_ICON_URL_META_FIELD_KEY,
            "https://example.com/icon.png",
        ),
        (WELCOME_MESSAGE_META_FIELD_KEY, "Welcome!"),
        (
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            "https://example.com/custom-terms",
        ),
    ] {
        assert!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue(value.to_owned()),
            )
            .is_ok(),
            "compiled dispatch rejected {key}"
        );
    }
}

#[test]
fn compiled_dispatch_refuses_unknown_and_semantically_invalid_values() {
    assert_eq!(
        validate_meta_field(
            &MetaFieldKey("fedi:unknown".to_owned()),
            &MetaFieldValue("value".to_owned()),
        ),
        Err(MetaFieldError::UnknownKey)
    );
    for (key, value) in [
        (FEDERATION_NAME_META_FIELD_KEY, "ab"),
        (FEDERATION_ICON_URL_META_FIELD_KEY, "ftp://example.com/icon"),
        (WELCOME_MESSAGE_META_FIELD_KEY, "   "),
        (TERMS_OF_SERVICE_URL_META_FIELD_KEY, "file:///tmp/tos"),
    ] {
        assert!(matches!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue(value.to_owned()),
            ),
            Err(MetaFieldError::InvalidValue(_))
        ));
    }
}

#[test]
fn formation_owned_keys_are_not_in_generic_dispatch() {
    for key in [
        fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY,
        GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY,
    ] {
        assert_eq!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue("not inspected".to_owned()),
            ),
            Err(MetaFieldError::UnknownKey),
        );
    }
}

#[test]
fn absolute_raw_caps_preserve_guardianito_padding_but_bound_resources() {
    for (key, value) in [
        (
            FEDERATION_NAME_META_FIELD_KEY,
            format!(" {} ", "a".repeat(FEDERATION_METADATA_NAME_MAX_BYTES)),
        ),
        (
            WELCOME_MESSAGE_META_FIELD_KEY,
            format!(
                " {} ",
                "a".repeat(FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES)
            ),
        ),
        (
            FEDERATION_ICON_URL_META_FIELD_KEY,
            format!(" {} ", "https://example.com/"),
        ),
        (
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            "  http://example.com/terms  ".to_owned(),
        ),
    ] {
        assert!(
            validate_meta_field(&MetaFieldKey(key.to_owned()), &MetaFieldValue(value),).is_ok(),
            "Guardianito-compatible padded value was narrowed for {key}",
        );
    }

    for (key, value) in [
        (
            FEDERATION_NAME_META_FIELD_KEY,
            " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1),
        ),
        (
            WELCOME_MESSAGE_META_FIELD_KEY,
            " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1),
        ),
        (
            FEDERATION_ICON_URL_META_FIELD_KEY,
            " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1),
        ),
        (
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1),
        ),
    ] {
        assert!(matches!(
            validate_meta_field(&MetaFieldKey(key.to_owned()), &MetaFieldValue(value)),
            Err(MetaFieldError::InvalidValue(_))
        ));
    }

    assert_eq!(
        validate_meta_field(
            &MetaFieldKey("x".repeat(META_FIELD_KEY_MAX_BYTES + 1)),
            &MetaFieldValue("value".to_owned()),
        ),
        Err(MetaFieldError::UnknownKey),
    );
}

// Guardianito semantic boundaries are pinned with the shared newtypes in
// `fedi-decentralized-service-manifold-guardian`'s `maintenance` tests; the tests
// below prove the wire boundary still refuses hardened values through the
// delegating validators, byte-identical error text included.

#[test]
fn invisible_and_direction_control_characters_are_refused() {
    // An RLO-spoofed name renders its tail reversed; the bytes and the glyphs
    // disagree about what the federation is called.
    let name = FederationNameValidator;
    assert_eq!(
        name.validate("Fed\u{202E}gnp.eration"),
        Err(MetaFieldError::InvalidValue(
            "federation name contains a bidirectional control character".to_owned()
        ))
    );
    for spoofed in ["abc\u{2066}def", "abc\u{200F}def", "abc\u{061C}def"] {
        assert!(name.validate(spoofed).is_err());
    }

    let welcome = WelcomeMessageValidator;
    assert_eq!(
        welcome.validate("warm\u{200B}greetings"),
        Err(MetaFieldError::InvalidValue(
            "welcome message contains a zero-width character".to_owned()
        ))
    );
    for spoofed in ["a\u{200D}b", "a\u{FEFF}b", "a\u{202A}b"] {
        assert!(welcome.validate(spoofed).is_err());
    }

    // Ordinary non-ASCII text is untouched: the refusal is about invisible
    // and reordering characters, not about scripts.
    assert!(name.validate("Café 联盟").is_ok());
    assert!(welcome.validate("¡Bienvenidos! ようこそ 🎉").is_ok());
}

#[test]
fn url_hosts_must_be_public() {
    check_url_hosts(&FederationIconUrlValidator);
    check_url_hosts(&TermsOfServiceUrlValidator);
}

fn check_url_hosts(validator: &dyn MetaFieldValidator) {
    for internal in [
        // Loopback.
        "http://localhost/icon.png",
        "https://sub.localhost/icon.png",
        "https://127.0.0.1/icon.png",
        "https://127.8.8.8/icon.png",
        "https://[::1]/icon.png",
        "https://[::ffff:127.0.0.1]/icon.png",
        // Link-local.
        "https://169.254.7.7/icon.png",
        "https://[fe80::1]/icon.png",
        // RFC-1918 private ranges.
        "https://10.0.0.1/icon.png",
        "https://172.16.0.1/icon.png",
        "https://172.31.255.1/icon.png",
        "https://192.168.1.1/icon.png",
        // A bare hostname only resolves inside some local network.
        "https://intranet/icon.png",
    ] {
        assert!(
            matches!(
                validator.validate(internal),
                Err(MetaFieldError::InvalidValue(_))
            ),
            "accepted an internal icon host: {internal}"
        );
    }

    for public in [
        "https://example.com/icon.png",
        // Not private: 172.32/12 is outside 172.16/12.
        "https://172.32.0.1/icon.png",
    ] {
        assert!(
            validator.validate(public).is_ok(),
            "rejected a public icon host: {public}"
        );
    }
}

#[test]
fn registry_serves_every_app_setting_once_and_no_duplicate_keys() {
    let keys = VALIDATORS
        .iter()
        .map(|validator| validator.key())
        .collect::<Vec<_>>();
    let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), keys.len(), "duplicate key in VALIDATORS");
    for setting in FederationAppSetting::ALL {
        assert!(keys.contains(&setting.key()), "{setting:?} is not served");
    }
}

#[test]
fn compiled_dispatch_relays_app_settings_with_their_shared_rules() {
    for (key, value) in [
        ("fedi:stability_pool_disabled", "false"),
        ("fedi:max_balance_msats", "0"),
        ("fedi:default_currency", "EUR"),
        ("fedi:pinned_message", ""),
        ("fedi:default_group_chats", r#"["!abc:matrix.org"]"#),
        ("fedi:autojoin_communities", "[]"),
    ] {
        assert_eq!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue(value.to_owned())
            ),
            Ok(()),
            "{key}"
        );
    }
    for (key, value) in [
        ("fedi:stability_pool_disabled", "no"),
        ("fedi:max_balance_msats", "-1"),
        ("fedi:default_group_chats", r#"["abc"]"#),
    ] {
        assert!(matches!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue(value.to_owned())
            ),
            Err(MetaFieldError::InvalidValue(_))
        ));
    }
}

#[test]
fn fedi_operated_and_inert_keys_are_not_served() {
    for key in [
        "vetted_gateways",
        "recurringd_api",
        "fedi:fixed_exchange_rate",
        "fedi:popup_end_timestamp",
        "fedi:popup_countdown_message",
        "fedi:popup_ended_message",
        "federation_expiry_timestamp",
        "fedi:social_recovery_disabled",
        "meta_override_url",
        // Unprefixed spellings would lose to any `fedi:` value the app reads first.
        "stability_pool_disabled",
    ] {
        assert_eq!(
            validate_meta_field(
                &MetaFieldKey(key.to_owned()),
                &MetaFieldValue("false".to_owned()),
            ),
            Err(MetaFieldError::UnknownKey),
            "{key}"
        );
    }
}

#[test]
fn every_served_field_at_its_cap_fits_the_consensus_object_cap() {
    // Filling every served field must leave later maintenance possible, so the
    // sum of raw caps plus formation-owned fields stays below half the object
    // cap: JSON string escaping at most doubles a value's printable bytes.
    let served = VALIDATORS
        .iter()
        .map(|validator| validator.raw_value_max_bytes() + validator.key().len() + 8)
        .sum::<usize>();
    let formation = fedi_decentralized_domain::MG_SEAT_BINDINGS_MAX_VALUE_BYTES
        + fedi_decentralized_domain::MG_API_URLS_MAX_VALUE_BYTES;
    assert!(
        served + formation
            < fedi_decentralized_service_manifold_guardian::FEDERATION_METADATA_OBJECT_MAX_BYTES
                / 2,
        "served {served} + formation {formation} bytes"
    );
}
