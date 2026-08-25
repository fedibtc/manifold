//! Compiled metadata-policy dispatch tests.

use fedi_decentralized_service_fleet_manager::{
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
            GUARDIANITO_TERMS_OF_SERVICE_URL,
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
        (
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            "https://example.com/tos",
        ),
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
        fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
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
// `fedi-decentralized-service-fleet-manager`'s `maintenance` tests; the tests
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
fn icon_url_hosts_must_be_public() {
    let icon = FederationIconUrlValidator;

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
                icon.validate(internal),
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
            icon.validate(public).is_ok(),
            "rejected a public icon host: {public}"
        );
    }
}
