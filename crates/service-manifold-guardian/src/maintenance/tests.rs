//! Shared federation-metadata newtype tests.
//!
//! These types are the single semantic authority for FA-side pre-validation
//! and the MG's compiled meta-field policy, so validation boundaries and security
//! checks are tested here, at the type layer.

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
fn name_constructor_checks_validation_boundaries_and_errors() {
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
    // Unprefixed aliases, community settings, and Fedi-operated keys are refused.
    for key in [
        "stability_pool_disabled",
        "vetted_gateways",
        "fedi:fedimods",
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
fn currency_accepts_only_codes_and_aliases_the_app_supports() {
    let setting = FederationAppSetting::DefaultCurrency;
    for value in ["USD", "XAF", "ZWL", "cameroon", "cote_d_ivoire", "togo"] {
        assert!(app(setting, value).is_ok(), "{value}");
    }
    // Well-shaped but unknown to the app: an unknown alias makes
    // `Intl.NumberFormat` throw, an unknown code shows a zero rate.
    for value in [
        "",
        "usd",
        "Usd",
        "XYZ",
        "AED",
        "banana",
        "us_dollars",
        "togo ",
    ] {
        assert!(app(setting, value).is_err(), "{value:?}");
    }
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
    // Unlike the welcome message, control padding is refused too.
    assert!(app(setting, "\u{b}Maintenance\u{b}").is_err());
}

fn v2_payload(decryption_key: &str) -> String {
    serde_json::json!({
        "author_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        "community_uuid_hex": "00112233445566778899aabbccddeeff",
        "decryption_key": decryption_key,
    })
    .to_string()
}

#[test]
fn linked_communities_accept_only_invites_fedi_can_join() {
    use base64::Engine as _;

    let setting = FederationAppSetting::AutojoinCommunities;
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let v1 = community_invite(
        "fedi:community",
        r#"{"community_meta_url":"https://example.com/c.json"}"#,
    );
    let v2 = community_invite("fedi:community2", &v2_payload(&key));
    assert!(app(setting, "[]").is_ok());
    assert!(app(setting, &serde_json::json!([v1, v2]).to_string()).is_ok());
    // Fedi lowercases before decoding, so an uppercase code still joins.
    assert!(app(setting, &serde_json::json!([v2.to_uppercase()]).to_string()).is_ok());
    // serde_with's base64 decoder ignores padding.
    let unpadded_key = base64::engine::general_purpose::STANDARD_NO_PAD.encode([7u8; 32]);
    let unpadded = community_invite("fedi:community2", &v2_payload(&unpadded_key));
    assert!(app(setting, &serde_json::json!([unpadded]).to_string()).is_ok());

    let short_key = base64::engine::general_purpose::STANDARD.encode([7u8; 31]);
    let url_safe_key = base64::engine::general_purpose::URL_SAFE.encode([0xfbu8; 32]);
    let mut bad_checksum = v2.clone();
    let last = bad_checksum.pop().unwrap();
    bad_checksum.push(if last == 'q' { 'p' } else { 'q' });
    for invite in [
        community_invite("fedi:group", r#"{"community_meta_url":"https://x.com"}"#),
        community_invite("fedi:community2", "not json"),
        community_invite("fedi:community", "{}"),
        community_invite("fedi:community2", "{}"),
        community_invite("fedi:community", r#"{"community_meta_url":7}"#),
        // Each HRP carries its own payload version.
        community_invite("fedi:community", &v2_payload(&key)),
        community_invite(
            "fedi:community2",
            r#"{"community_meta_url":"https://example.com/c.json"}"#,
        ),
        community_invite("fedi:community2", &v2_payload(&short_key)),
        community_invite("fedi:community2", &v2_payload(&url_safe_key)),
        community_invite("fedi:community2", &v2_payload(&key).replace("79be", "zzbe")),
        bad_checksum,
    ] {
        let value = serde_json::json!([invite]).to_string();
        assert!(app(setting, &value).is_err(), "{value}");
    }
    for value in [
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
fn linked_community_urls_follow_the_icon_url_rules() {
    for (url, accepted) in [
        ("https://example.com/community.json", true),
        ("http://example.com/community.json", true),
        ("http://127.0.0.1:8080/action", false),
        ("http://10.0.0.1/action", false),
        ("http://172.16.0.1/action", false),
        ("http://192.168.1.1/action", false),
        ("http://169.254.169.254/", false),
        ("http://[::1]/", false),
        ("http://[::ffff:127.0.0.1]/", false),
        ("http://[fe80::1]/", false),
        ("http://localhost/action", false),
        ("http://app.localhost/action", false),
        ("http://printer/action", false),
        ("file:///etc/passwd", false),
        ("not a URL", false),
        ("", false),
    ] {
        let payload = serde_json::json!({ "community_meta_url": url }).to_string();
        let invite = community_invite("fedi:community", &payload);
        let value = serde_json::json!([invite]).to_string();
        assert_eq!(
            app(FederationAppSetting::AutojoinCommunities, &value).is_ok(),
            accepted,
            "{url}"
        );
    }
}

#[test]
fn group_chats_accept_legacy_and_v12_matrix_room_ids() {
    let setting = FederationAppSetting::DefaultGroupChats;
    assert!(app(setting, "[]").is_ok());
    assert!(app(setting, r#"["!abc:matrix.org","!def:matrix.org"]"#).is_ok());
    // Room version 12 ids are the bare create-event hash, without a server.
    assert!(
        app(
            setting,
            r#"["!31hneApxJ_1o-63DmFrpeqnkFfWppnzWso1JvH3ogLM"]"#
        )
        .is_ok()
    );
    for value in [
        r#"["abc:matrix.org"]"#,
        r#"["!"]"#,
        r##"["#alias:matrix.org"]"##,
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

#[test]
fn app_settings_at_their_caps_stay_a_small_share_of_the_object_cap() {
    // A stored value is a JSON string. App-setting values hold no raw control
    // characters except JSON whitespace, so escaping at most doubles them. A
    // newline-padded empty list is valid and is that worst case.
    for setting in [
        FederationAppSetting::AutojoinCommunities,
        FederationAppSetting::DefaultGroupChats,
    ] {
        let max_bytes = setting.raw_max_bytes();
        let padded = format!("[{}]", "\n".repeat(max_bytes - 2));
        assert_eq!(app(setting, &padded), Ok(()), "{setting:?}");
        let stored = serde_json::to_string(&padded).unwrap().len();
        assert_eq!(stored, 2 * max_bytes, "{setting:?}");
    }
    let stored = FederationAppSetting::ALL
        .into_iter()
        .map(|setting| 2 * setting.raw_max_bytes() + 2 + setting.key().len() + 4)
        .sum::<usize>();
    // Padding in name, icon, welcome, and terms fields can exceed the object cap
    // on its own, and MG refuses such a target before voting. The app
    // settings add only a bounded share on top.
    assert!(
        stored < FEDERATION_METADATA_OBJECT_MAX_BYTES / 4,
        "app settings can store {stored} bytes"
    );
}
