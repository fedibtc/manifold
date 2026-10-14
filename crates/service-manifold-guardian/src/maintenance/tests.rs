//! Shared federation-metadata newtype tests.
//!
//! These types are the single semantic authority for FA-side pre-validation
//! and the MG's compiled meta-field policy, so the Guardianito-compatible
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

fn app(setting: FederationAppSetting, value: &str) -> Result<(), InvalidFederationMetadataValue> {
    FederationMetadataUpdate::app_setting(setting, value).map(|_| ())
}

fn community_invite(hrp: &str, payload: &str) -> String {
    bech32::encode::<bech32::Bech32m>(bech32::Hrp::parse(hrp).unwrap(), payload.as_bytes()).unwrap()
}

#[test]
fn app_setting_keys_are_distinct_prefixed_and_round_trip() {
    let keys = FederationAppSetting::ALL
        .into_iter()
        .map(FederationAppSetting::key)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), FederationAppSetting::ALL.len());
    for setting in FederationAppSetting::ALL {
        assert!(setting.key().starts_with("fedi:"), "{setting:?}");
        assert_eq!(FederationAppSetting::from_key(setting.key()), Some(setting));
    }
    // Unprefixed aliases and Fedi-operated keys are not settings.
    for key in [
        "stability_pool_disabled",
        "vetted_gateways",
        "fedi:fixed_exchange_rate",
    ] {
        assert_eq!(FederationAppSetting::from_key(key), None);
    }
}

#[test]
fn app_setting_update_submits_the_exact_value_under_its_key() {
    let (key, value) =
        FederationMetadataUpdate::app_setting(FederationAppSetting::StabilityPoolDisabled, "false")
            .unwrap()
            .into_field();
    assert_eq!(key.0, "fedi:stability_pool_disabled");
    assert_eq!(value.0, "false");
}

#[test]
fn flags_accept_only_the_exact_strings_the_app_compares() {
    for setting in [
        FederationAppSetting::StabilityPoolDisabled,
        FederationAppSetting::NewMembersDisabled,
    ] {
        assert!(app(setting, "true").is_ok());
        assert!(app(setting, "false").is_ok());
        for value in ["True", "FALSE", "1", "0", "", " false", "false "] {
            assert!(
                app(setting, value).is_err(),
                "{setting:?} accepted {value:?}"
            );
        }
    }
}

#[test]
fn msat_limits_are_canonical_and_bounded_by_the_bitcoin_supply() {
    let setting = FederationAppSetting::MaxInvoiceMsats;
    // Zero is meaningful to the app: it blocks every invoice.
    for value in ["0", "1", "1000", "2100000000000000000"] {
        assert!(app(setting, value).is_ok(), "{value}");
    }
    for value in [
        "",
        "00",
        "01",
        "+1",
        "-1",
        "1.5",
        "1e3",
        " 1",
        "2100000000000000001",
        "18446744073709551616",
    ] {
        assert!(app(setting, value).is_err(), "{value:?}");
    }
}

#[test]
fn currency_accepts_iso_codes_and_app_aliases_only() {
    let setting = FederationAppSetting::DefaultCurrency;
    for value in ["USD", "XAF", "cameroon", "cote_d_ivoire"] {
        assert!(app(setting, value).is_ok(), "{value}");
    }
    for value in ["", "US", "USDT", "Usd", "_niger", "côte", "US D", "usd1"] {
        assert!(app(setting, value).is_err(), "{value:?}");
    }
    assert!(app(setting, &"a".repeat(33)).is_err());
}

#[test]
fn pinned_message_clears_with_empty_and_otherwise_matches_welcome_rules() {
    let setting = FederationAppSetting::PinnedMessage;
    assert!(app(setting, "").is_ok());
    assert!(app(setting, " Maintenance on Friday ").is_ok());
    assert!(
        app(
            setting,
            &"a".repeat(FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES)
        )
        .is_ok()
    );
    // Whitespace alone would render an empty pinned card.
    assert!(app(setting, "   ").is_err());
    assert!(
        app(
            setting,
            &"a".repeat(FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES + 1)
        )
        .is_err()
    );
    assert!(app(setting, "safe\u{202E}txt").is_err());
    assert!(app(setting, "line\nbreak").is_err());
}

#[test]
fn linked_communities_accept_v1_and_v2_invites_and_reject_lookalikes() {
    let setting = FederationAppSetting::AutojoinCommunities;
    let v1 = community_invite(
        "fedi:community",
        r#"{"community_meta_url":"https://example.com/c.json"}"#,
    );
    let v2 = community_invite("fedi:community2", r#"{"community_uuid_hex":"ab"}"#);
    assert!(app(setting, "[]").is_ok());
    assert!(app(setting, &serde_json::json!([v1, v2]).to_string()).is_ok());
    // Fedi lowercases before decoding, so an uppercase code still joins.
    assert!(app(setting, &serde_json::json!([v2.to_uppercase()]).to_string()).is_ok());

    let wrong_hrp = community_invite("fedi:group", r#"{"id":"x"}"#);
    let not_json = community_invite("fedi:community2", "not json");
    let mut bad_checksum = v2.clone();
    let last = bad_checksum.pop().unwrap();
    bad_checksum.push(if last == 'q' { 'p' } else { 'q' });
    for value in [
        serde_json::json!([wrong_hrp]).to_string(),
        serde_json::json!([not_json]).to_string(),
        serde_json::json!([bad_checksum]).to_string(),
        // The same community twice, differing only in case.
        serde_json::json!([v2, v2.to_uppercase()]).to_string(),
        serde_json::json!(v2).to_string(),
        format!("[{}]", serde_json::json!({ "code": v2 })),
        String::new(),
    ] {
        assert!(app(setting, &value).is_err(), "{value}");
    }

    let at_limit = (0..FEDERATION_APP_SETTING_MAX_COMMUNITIES)
        .map(|index| {
            community_invite(
                "fedi:community",
                &format!(r#"{{"community_meta_url":"https://example.com/{index}"}}"#),
            )
        })
        .collect::<Vec<_>>();
    assert!(app(setting, &serde_json::json!(at_limit).to_string()).is_ok());
    let mut over_limit = at_limit;
    over_limit.push(v2);
    assert!(app(setting, &serde_json::json!(over_limit).to_string()).is_err());
}

#[test]
fn group_chats_require_unique_matrix_room_ids() {
    let setting = FederationAppSetting::DefaultGroupChats;
    assert!(app(setting, "[]").is_ok());
    assert!(app(setting, r#"["!abc:matrix.org","!def:matrix.org"]"#).is_ok());
    for value in [
        r#"["abc:matrix.org"]"#,
        r#"["!abc"]"#,
        r#"["!:matrix.org"]"#,
        r#"["!abc:"]"#,
        r#"["!a b:matrix.org"]"#,
        r#"["!abc:matrix.org","!abc:matrix.org"]"#,
        r#"[1]"#,
        r#"{"room":"!abc:matrix.org"}"#,
    ] {
        assert!(app(setting, value).is_err(), "{value}");
    }
    // Matrix caps room ids at 255 bytes.
    let longest = format!("!{}:s", "a".repeat(252));
    assert!(app(setting, &serde_json::json!([longest]).to_string()).is_ok());
    let too_long = format!("!{}:s", "a".repeat(253));
    assert!(app(setting, &serde_json::json!([too_long]).to_string()).is_err());
    let over_limit = (0..=FEDERATION_APP_SETTING_MAX_GROUP_CHATS)
        .map(|index| format!("!room{index}:matrix.org"))
        .collect::<Vec<_>>();
    assert!(app(setting, &serde_json::json!(over_limit).to_string()).is_err());
}

#[test]
fn fedimods_accept_the_app_shape_with_public_urls_only() {
    let setting = FederationAppSetting::FediMods;
    let full = serde_json::json!({
        "id": "btcmap",
        "title": "BTC Map",
        "url": "https://btcmap.org/map",
        "imageUrl": "https://example.com/btcmap.png",
        "description": "Find merchants",
        "color": "#ff9900",
    });
    let minimal = serde_json::json!({
        "id": "faucet",
        "title": "Faucet",
        "url": "https://faucet.example.com/",
    });
    assert!(app(setting, "[]").is_ok());
    assert!(app(setting, &serde_json::json!([full, minimal]).to_string()).is_ok());

    let with = |key: &str, value: serde_json::Value| {
        let mut entry = minimal.clone();
        entry[key] = value;
        serde_json::json!([entry]).to_string()
    };
    let without_url = {
        let mut entry = minimal.clone();
        entry.as_object_mut().unwrap().remove("url");
        serde_json::json!([entry]).to_string()
    };
    for value in [
        with("url", "javascript:alert(1)".into()),
        with("url", "http://localhost:8080/".into()),
        with("imageUrl", "http://192.168.1.1/x.png".into()),
        with("title", "".into()),
        with("title", "a\u{200B}b".into()),
        with("id", " ".into()),
        with("unexpected", "field".into()),
        // The app's field is camelCase; a snake_case key would be ignored.
        with("image_url", "https://example.com/x.png".into()),
        without_url,
        serde_json::json!([minimal, minimal]).to_string(),
    ] {
        assert!(app(setting, &value).is_err(), "{value}");
    }
    let over_limit = (0..=FEDERATION_APP_SETTING_MAX_FEDIMODS)
        .map(|index| {
            serde_json::json!({
                "id": format!("mod{index}"),
                "title": "Mod",
                "url": "https://example.com/",
            })
        })
        .collect::<Vec<_>>();
    assert!(app(setting, &serde_json::json!(over_limit).to_string()).is_err());
}

#[test]
fn app_settings_check_the_raw_cap_before_parsing() {
    for setting in FederationAppSetting::ALL {
        let max_bytes = setting.raw_max_bytes();
        assert_eq!(
            app(setting, &" ".repeat(max_bytes + 1)),
            Err(InvalidFederationMetadataValue::RawTooLarge {
                field: setting.key(),
                max_bytes,
            }),
            "{setting:?}",
        );
    }
}
