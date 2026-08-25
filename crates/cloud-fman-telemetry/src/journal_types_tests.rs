//! Safe-journal value validation tests.

use fedi_decentralized_service_fleet_manager::{SafeEventCursor, SafeEventJournalIncarnation};

use super::*;

fn incarnation() -> SafeEventJournalIncarnation {
    "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap()
}

fn validate(jsonl: Vec<u8>) -> Result<ValidatedJournalBatch, JournalValueError> {
    let incarnation = incarnation();
    ValidatedJournalBatch::new(
        &incarnation,
        None,
        incarnation.clone(),
        jsonl,
        Some(SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: 0,
            offset: 1,
        }),
        false,
    )
}

#[test]
fn accepts_exact_bounded_safe_jsonl() {
    let bytes = b"{\"fields\":{\"safe_to_share\":true},\"message\":\"ok\"}\n".to_vec();
    assert_eq!(validate(bytes.clone()).unwrap().jsonl(), bytes);
}

#[test]
fn cursor_coordinates_must_fit_the_durable_sqlite_domain() {
    let incarnation = incarnation();
    let batch = |segment, offset| {
        ValidatedJournalBatch::new(
            &incarnation,
            None,
            incarnation.clone(),
            b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
            Some(SafeEventCursor {
                incarnation: incarnation.clone(),
                segment,
                offset,
            }),
            false,
        )
    };

    assert!(batch(i64::MAX as u64, i64::MAX as u64).is_ok());
    assert!(batch(i64::MAX as u64 + 1, 1).is_err());
    assert!(batch(1, i64::MAX as u64 + 1).is_err());
}

#[test]
fn rejects_unsafe_spanned_malformed_and_oversized_records() {
    assert!(validate(b"{\"fields\":{\"safe_to_share\":false}}\n".to_vec()).is_err());
    assert!(validate(b"{\"fields\":{\"safe_to_share\":true},\"span\":{}}\n".to_vec()).is_err());
    assert!(validate(b"{not-json}\n".to_vec()).is_err());
    assert!(validate(b"{\"fields\":{\"safe_to_share\":true}}".to_vec()).is_err());
    assert!(
        validate(
            format!(
                "{{\"fields\":{{\"safe_to_share\":true}},\"x\":\"{}\"}}\n",
                "x".repeat(MAX_RECORD_BYTES)
            )
            .into_bytes()
        )
        .is_err()
    );
}

#[test]
fn civil_date_conversion_handles_rollover_and_range() {
    assert_eq!(
        ReceptionDay::from_unix_seconds(1_704_067_199)
            .unwrap()
            .as_str(),
        "2023-12-31"
    );
    assert_eq!(
        ReceptionDay::from_unix_seconds(1_704_067_200)
            .unwrap()
            .as_str(),
        "2024-01-01"
    );
    assert!(ReceptionDay::from_unix_seconds(i64::MAX).is_err());
}
