use super::*;

#[test]
fn dkg_display_names_are_bounded_human_readable_text() {
    assert!(validate_dkg_display_name("guardian one").is_ok());
    assert!(
        validate_dkg_display_name(&"é".repeat(64)).is_ok(),
        "the limit is 128 UTF-8 bytes, not scalar values"
    );

    for invalid in ["", "   ", "guardian\nname", &"x".repeat(129)] {
        assert!(validate_dkg_display_name(invalid).is_err());
    }
    assert!(validate_dkg_display_name(&"é".repeat(65)).is_err());
}

fn callback_with_lengths(url_len: usize, key_len: usize) -> serde_json::Value {
    serde_json::json!({
        "callback_url": "x".repeat(url_len),
        "idempotency_key": "k".repeat(key_len),
    })
}

#[test]
fn completion_callback_validates_boundaries_and_redacts_debug() {
    for value in [
        callback_with_lengths(DKG_COMPLETION_CALLBACK_URL_MAX_BYTES, 1),
        callback_with_lengths(1, DKG_COMPLETION_IDEMPOTENCY_KEY_MAX_BYTES),
    ] {
        assert!(serde_json::from_value::<DkgCompletionCallback>(value).is_ok());
    }
    for value in [
        callback_with_lengths(DKG_COMPLETION_CALLBACK_URL_MAX_BYTES + 1, 1),
        callback_with_lengths(1, DKG_COMPLETION_IDEMPOTENCY_KEY_MAX_BYTES + 1),
        callback_with_lengths(0, 1),
        callback_with_lengths(1, 0),
    ] {
        assert!(serde_json::from_value::<DkgCompletionCallback>(value).is_err());
    }

    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();
    let encoded = serde_json::to_vec(&callback).unwrap();
    let decoded: DkgCompletionCallback = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, callback);
    let formatted = format!("{callback:?}");
    assert!(!formatted.contains("bearer-secret"));
    assert!(!formatted.contains("formation-dkg-complete"));
}

#[test]
fn callback_start_and_restart_reject_unknown_fields() {
    let callback = serde_json::json!({
        "callback_url": "https://push.example/hooks/a/b",
        "idempotency_key": "key",
        "unexpected": true,
    });
    assert!(serde_json::from_value::<DkgCompletionCallback>(callback).is_err());

    let request = serde_json::json!({
        "ts": 1,
        "fi_id": "00".repeat(32),
        "seat_id": "00".repeat(32),
        "guardian_codes": [],
        "completion_callback": {
            "callback_url": "https://push.example/hooks/a/b",
            "idempotency_key": "key"
        },
        "future_field": true,
    });
    assert!(serde_json::from_value::<StartDkgRequest>(request).is_err());

    let restart = serde_json::json!({
        "ts": 1,
        "fi_id": "00".repeat(32),
        "seat_id": "00".repeat(32),
        "guardian_codes": [],
        "completion_callback": null,
    });
    assert!(serde_json::from_value::<RestartDkgRequest>(restart).is_err());
}
