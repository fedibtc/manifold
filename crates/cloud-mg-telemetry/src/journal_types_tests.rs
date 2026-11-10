//! Safe-journal value validation tests.

use fedi_decentralized_service_manifold_guardian::{SafeEventCursor, SafeEventJournalIncarnation};

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

/// A torn line, which an MG writer leaves when it dies mid-append, is never
/// served: the batch the MG's own reader serves validates, while the raw lines
/// it was read from would make this collector reject the batch.
#[test]
fn a_torn_journal_line_is_not_served_to_the_collector() {
    use fedi_decentralized_service_manifold_guardian::{
        FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, SafeEventJournal,
        TelemetryCapability,
    };

    let directory = tempfile::tempdir().unwrap();
    let incarnation: SafeEventJournalIncarnation =
        bounded_rolling_file::open_incarnation(directory.path())
            .unwrap()
            .as_str()
            .parse()
            .unwrap();
    let record = b"{\"fields\":{\"safe_to_share\":true},\"message\":\"ok\"}\n";
    let segment = directory.path().join("events-0.jsonl");
    std::fs::write(&segment, [&record[..], b"{\"fields\":{\"safe"].concat()).unwrap();
    let mut writer = bounded_rolling_file::RollingFileAppender::open(
        directory.path(),
        bounded_rolling_file::Config {
            max_file_bytes: 4096,
            max_files: 2,
        },
    )
    .unwrap();
    writer.append_record(record).unwrap();
    drop(writer);

    let served = mg_telemetry::read_safe_event_journal(
        &SafeEventJournal::Mg,
        directory.path().to_owned(),
        FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([3; 32]),
            journal: SafeEventJournal::Mg,
            incarnation: incarnation.clone(),
            cursor: None,
        },
    )
    .unwrap();
    let FetchSafeEventJournalResponse::Current {
        incarnation: served_incarnation,
        jsonl,
        next_cursor,
        continuity_gap,
    } = served
    else {
        panic!("journal incarnation must be current");
    };
    let batch = ValidatedJournalBatch::new(
        &incarnation,
        None,
        served_incarnation,
        jsonl,
        next_cursor,
        continuity_gap,
    )
    .unwrap();
    assert_eq!(batch.jsonl(), [&record[..], &record[..]].concat());

    assert!(validate(std::fs::read(&segment).unwrap()).is_err());
}

/// Torn lines that fill a whole storage batch do not stall the collector: the
/// MG's reader serves the record after them in one batch this collector accepts.
#[test]
fn torn_lines_filling_a_storage_batch_do_not_hold_back_the_collector() {
    use fedi_decentralized_service_manifold_guardian::{
        FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, MAX_SAFE_EVENT_BATCH_BYTES,
        SafeEventJournal, TelemetryCapability,
    };

    let directory = tempfile::tempdir().unwrap();
    let incarnation: SafeEventJournalIncarnation =
        bounded_rolling_file::open_incarnation(directory.path())
            .unwrap()
            .as_str()
            .parse()
            .unwrap();
    let torn_line = [vec![b'{'; 60 * 1024 - 1], vec![b'\n']].concat();
    let record = b"{\"fields\":{\"safe_to_share\":true},\"message\":\"ok\"}\n";
    let mut segment = Vec::new();
    for _ in 0..MAX_SAFE_EVENT_BATCH_BYTES / torn_line.len() + 2 {
        segment.extend_from_slice(&torn_line);
    }
    segment.extend_from_slice(record);
    std::fs::write(directory.path().join("events-0.jsonl"), &segment).unwrap();

    let served = mg_telemetry::read_safe_event_journal(
        &SafeEventJournal::Mg,
        directory.path().to_owned(),
        FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([3; 32]),
            journal: SafeEventJournal::Mg,
            incarnation: incarnation.clone(),
            cursor: None,
        },
    )
    .unwrap();
    let FetchSafeEventJournalResponse::Current {
        incarnation: served_incarnation,
        jsonl,
        next_cursor,
        continuity_gap,
    } = served
    else {
        panic!("journal incarnation must be current");
    };
    let batch = ValidatedJournalBatch::new(
        &incarnation,
        None,
        served_incarnation,
        jsonl,
        next_cursor,
        continuity_gap,
    )
    .unwrap();
    assert_eq!(batch.jsonl(), record);
    assert_eq!(
        batch.next_cursor().map(|cursor| cursor.offset),
        Some(segment.len() as u64)
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
