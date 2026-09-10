//! Shared federation-metadata newtype tests.
//!
//! These types are the single semantic authority for FI-side pre-validation
//! and the FMan's compiled meta-field policy, so the Guardianito-compatible
//! boundaries and the security hardenings are pinned here, at the type layer.

use super::*;

fn name(value: &str) -> Result<FederationMetadataName, InvalidFederationMetadataValue> {
    FederationMetadataName::try_from(value.to_owned())
}

fn welcome(
    value: &str,
) -> Result<FederationMetadataWelcomeMessage, InvalidFederationMetadataValue> {
    FederationMetadataWelcomeMessage::try_from(value.to_owned())
}

#[test]
fn name_constructor_pins_every_guardianito_boundary_and_error_class() {
    for length in [3, FEDERATION_METADATA_NAME_MAX_BYTES] {
        assert!(FederationMetadataUpdate::name("a".repeat(length)).is_ok());
    }
    for length in [0, 1, 2, FEDERATION_METADATA_NAME_MAX_BYTES + 1] {
        assert!(matches!(
            FederationMetadataUpdate::name("a".repeat(length)),
            Err(InvalidFederationMetadataValue::InvalidTrimmedLength {
                field: "federation name",
                min_bytes: 3,
                max_bytes: FEDERATION_METADATA_NAME_MAX_BYTES,
            })
        ));
    }
    for control in ['\0', '\n', '\u{7f}'] {
        assert!(matches!(
            FederationMetadataUpdate::name(format!("abc{control}def")),
            Err(InvalidFederationMetadataValue::ControlCharacter {
                field: "federation name"
            })
        ));
    }
    for phrase in [
        "payment request rejected",
        "PAYMENT REQUEST REJECTED",
        "x PaYmEnT ReQuEsT ReJeCtEd y",
    ] {
        assert!(matches!(
            FederationMetadataUpdate::name(phrase),
            Err(InvalidFederationMetadataValue::RefusedNamePhrase)
        ));
    }

    let exact_raw_limit = format!("{}abc", " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES - 3));
    let (_, preserved) = FederationMetadataUpdate::name(exact_raw_limit.clone())
        .expect("the inclusive raw limit is accepted")
        .into_field();
    assert_eq!(preserved.0, exact_raw_limit);
    assert!(matches!(
        FederationMetadataUpdate::name("a".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1)),
        Err(InvalidFederationMetadataValue::RawTooLarge {
            field: "federation name",
            max_bytes: FEDERATION_METADATA_RAW_MAX_BYTES,
        })
    ));
}

#[test]
fn welcome_constructor_pins_500_byte_boundary_controls_and_raw_cap() {
    for length in [1, FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES] {
        assert!(FederationMetadataUpdate::welcome_message("a".repeat(length)).is_ok());
    }
    for length in [0, FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES + 1] {
        assert!(matches!(
            FederationMetadataUpdate::welcome_message("a".repeat(length)),
            Err(InvalidFederationMetadataValue::InvalidTrimmedLength {
                field: "welcome message",
                min_bytes: 1,
                max_bytes: FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES,
            })
        ));
    }
    for control in ['\0', '\r', '\u{7f}'] {
        assert!(matches!(
            FederationMetadataUpdate::welcome_message(format!("hello{control}world")),
            Err(InvalidFederationMetadataValue::ControlCharacter {
                field: "welcome message"
            })
        ));
    }
    assert!(matches!(
        FederationMetadataUpdate::welcome_message(
            "a".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1)
        ),
        Err(InvalidFederationMetadataValue::RawTooLarge {
            field: "welcome message",
            max_bytes: FEDERATION_METADATA_RAW_MAX_BYTES,
        })
    ));
}

#[test]
fn url_constructors_share_validation() {
    let icon = FederationMetadataUpdate::icon_url as fn(String) -> _;
    let terms = FederationMetadataUpdate::terms_of_service_url as fn(String) -> _;
    check_url_boundaries("federation icon URL", icon);
    check_url_boundaries("terms URL", terms);
    check_url_hosts(icon);
    check_url_hosts(terms);
}

fn check_url_boundaries(
    field: &'static str,
    constructor: fn(String) -> Result<FederationMetadataUpdate, InvalidFederationMetadataValue>,
) {
    let url = |length: usize| {
        const PREFIX: &str = "https://example.com/";
        format!("{PREFIX}{}", "a".repeat(length - PREFIX.len()))
    };
    assert!(constructor("http://example.com/icon.png".to_owned()).is_ok());
    assert!(constructor("https://example.com/icon.png".to_owned()).is_ok());
    assert!(constructor(url(FEDERATION_METADATA_ICON_URL_MAX_BYTES)).is_ok());
    assert!(constructor(" ".to_owned()).is_err());
    let padded = format!(
        "{}https://example.com",
        " ".repeat(FEDERATION_METADATA_RAW_MAX_BYTES - 19)
    );
    assert_eq!(
        constructor(padded.clone()).unwrap().into_field().1.0,
        padded
    );
    assert!(matches!(
        constructor(url(FEDERATION_METADATA_ICON_URL_MAX_BYTES + 1)),
        Err(InvalidFederationMetadataValue::InvalidTrimmedLength {
            field: actual,
            min_bytes: 1,
            max_bytes: FEDERATION_METADATA_ICON_URL_MAX_BYTES,
        }) if actual == field
    ));
    for invalid in [
        "ftp://example.com/icon.png",
        "data:image/png;base64,AA==",
        "file:///tmp/icon.png",
        "https://[",
        "not a URL",
    ] {
        assert!(matches!(
            constructor(invalid.to_owned()),
            Err(InvalidFederationMetadataValue::InvalidUrl { .. })
        ));
    }
    assert!(matches!(
        constructor("https://example.com/a\nb".to_owned()),
        Err(InvalidFederationMetadataValue::ControlCharacter {
            field: actual
        }) if actual == field
    ));
    assert!(matches!(
        constructor("a".repeat(FEDERATION_METADATA_RAW_MAX_BYTES + 1)),
        Err(InvalidFederationMetadataValue::RawTooLarge {
            field: actual,
            max_bytes: FEDERATION_METADATA_RAW_MAX_BYTES,
        }) if actual == field
    ));
}

#[test]
fn typed_updates_preserve_raw_values_and_pin_exact_keys_and_terms() {
    for (update, key, value) in [
        (
            FederationMetadataUpdate::name("  Federation Name  ").unwrap(),
            FEDERATION_NAME_META_FIELD_KEY,
            "  Federation Name  ",
        ),
        (
            FederationMetadataUpdate::icon_url("  https://example.com/icon.png  ").unwrap(),
            FEDERATION_ICON_URL_META_FIELD_KEY,
            "  https://example.com/icon.png  ",
        ),
        (
            FederationMetadataUpdate::welcome_message("  Welcome  ").unwrap(),
            WELCOME_MESSAGE_META_FIELD_KEY,
            "  Welcome  ",
        ),
        (
            FederationMetadataUpdate::terms_of_service_url("  https://example.com/custom-terms  ")
                .unwrap(),
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            "  https://example.com/custom-terms  ",
        ),
    ] {
        let (actual_key, actual_value) = update.into_field();
        assert_eq!(actual_key.0, key);
        assert_eq!(actual_value.0, value);
    }
}

#[test]
fn ordinary_unicode_is_accepted() {
    assert!(name("🌟🌟🌟").is_ok());
    assert!(name("Café 联盟").is_ok());
    assert!(welcome("Unicode: 你好 🎉 café").is_ok());
    assert!(welcome("¡Bienvenidos! ようこそ 🎉").is_ok());
}

#[test]
fn invisible_and_direction_control_characters_are_refused() {
    // An RLO-spoofed name renders its tail reversed; the bytes and the glyphs
    // disagree about what the federation is called.
    assert_eq!(
        name("Fed\u{202E}gnp.eration")
            .expect_err("RLO name accepted")
            .to_string(),
        "federation name contains a bidirectional control character"
    );
    for spoofed in ["abc\u{2066}def", "abc\u{200F}def", "abc\u{061C}def"] {
        assert!(matches!(
            name(spoofed),
            Err(InvalidFederationMetadataValue::InvisibleCharacter { .. })
        ));
    }

    assert_eq!(
        welcome("warm\u{200B}greetings")
            .expect_err("zero-width welcome accepted")
            .to_string(),
        "welcome message contains a zero-width character"
    );
    for spoofed in ["a\u{200D}b", "a\u{FEFF}b", "a\u{202A}b"] {
        assert!(matches!(
            welcome(spoofed),
            Err(InvalidFederationMetadataValue::InvisibleCharacter { .. })
        ));
    }
}

fn check_url_hosts(
    constructor: fn(String) -> Result<FederationMetadataUpdate, InvalidFederationMetadataValue>,
) {
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
                constructor(internal.to_owned()),
                Err(InvalidFederationMetadataValue::NonPublicUrlHost { .. })
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
            constructor(public.to_owned()).is_ok(),
            "rejected a public icon host: {public}"
        );
    }
}
