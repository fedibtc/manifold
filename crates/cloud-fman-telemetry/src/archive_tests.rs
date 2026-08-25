//! Archive durability and bound tests.

use std::{io::Read as _, sync::Arc};

use fedi_decentralized_service_fleet_manager::{SafeEventCursor, SafeEventJournalIncarnation};

use super::*;

fn stream(value: &str) -> JournalStreamId {
    JournalStreamId::parse(value.repeat(32 / value.len())).unwrap()
}

fn day(value: &str) -> ReceptionDay {
    ReceptionDay::parse(value.to_owned()).unwrap()
}

fn batch(jsonl: &[u8], offset: u64) -> ValidatedJournalBatch {
    let incarnation: SafeEventJournalIncarnation =
        "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap();
    ValidatedJournalBatch::new(
        &incarnation,
        None,
        incarnation.clone(),
        jsonl.to_vec(),
        Some(SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: 1,
            offset,
        }),
        false,
    )
    .unwrap()
}

fn archive(directory: &tempfile::TempDir, quota: u64) -> JournalArchive {
    JournalArchive::open(directory.path(), quota).unwrap()
}

#[test]
fn concatenated_frames_decode_to_exact_source_jsonl() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(&directory, 1024 * 1024);
    let stream = stream("a");
    let day = day("2024-01-01");
    let first = batch(b"{\"fields\":{\"safe_to_share\":true},\"n\":1}\n", 1);
    let second = batch(b"{\"fields\":{\"safe_to_share\":true},\"n\":2}\n", 2);
    archive.append(&stream, &day, &first).unwrap();
    archive.append(&stream, &day, &second).unwrap();

    let mut decoded = Vec::new();
    zstd::stream::read::Decoder::new(std::fs::File::open(archive.path(&stream, &day)).unwrap())
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(
        decoded,
        [first.jsonl(), second.jsonl()].concat(),
        "concatenated independent frames preserve exact bytes"
    );
}

#[test]
fn recovery_truncates_orphan_tail_to_committed_hash_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(&directory, 1024 * 1024);
    let stream = stream("z");
    let day = day("2024-02-29");
    let committed = archive
        .append(
            &stream,
            &day,
            &batch(b"{\"fields\":{\"safe_to_share\":true},\"n\":1}\n", 1),
        )
        .unwrap();
    archive
        .append(
            &stream,
            &day,
            &batch(b"{\"fields\":{\"safe_to_share\":true},\"n\":2}\n", 2),
        )
        .unwrap();

    archive
        .recover(vec![FrameBoundary {
            stream_id: stream.clone(),
            day: day.clone(),
            start: committed.start,
            end: committed.end,
            hash: committed.hash,
        }])
        .unwrap();
    assert_eq!(
        std::fs::metadata(archive.path(&stream, &day))
            .unwrap()
            .len(),
        committed.end
    );
}

#[test]
fn serialized_quota_allows_only_one_concurrent_append() {
    let probe_directory = tempfile::tempdir().unwrap();
    let probe = archive(&probe_directory, 1024 * 1024);
    let payload = batch(
        format!(
            "{{\"fields\":{{\"safe_to_share\":true}},\"payload\":\"{}\"}}\n",
            "abcdef0123456789".repeat(3000)
        )
        .as_bytes(),
        1,
    );
    let frame_len = probe
        .append(&stream("c"), &day("2024-01-01"), &payload)
        .unwrap()
        .end;

    let directory = tempfile::tempdir().unwrap();
    let archive = Arc::new(archive(&directory, frame_len));
    let first = {
        let archive = archive.clone();
        std::thread::spawn(move || archive.append(&stream("d"), &day("2024-01-01"), &payload))
    };
    let second_payload = batch(
        format!(
            "{{\"fields\":{{\"safe_to_share\":true}},\"payload\":\"{}\"}}\n",
            "abcdef0123456789".repeat(3000)
        )
        .as_bytes(),
        1,
    );
    let second = {
        let archive = archive.clone();
        std::thread::spawn(move || {
            archive.append(&stream("e"), &day("2024-01-01"), &second_payload)
        })
    };
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .any(|result| matches!(result, Err(ArchiveError::Capacity)))
    );
    assert_eq!(archive.used_bytes().unwrap(), frame_len);
}

#[test]
fn recovery_removes_uncommitted_day_and_empty_stream_directory() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(&directory, 1024 * 1024);
    let stream = stream("f");
    let day = day("2024-12-31");
    archive
        .append(
            &stream,
            &day,
            &batch(b"{\"fields\":{\"safe_to_share\":true}}\n", 1),
        )
        .unwrap();
    archive.recover(Vec::new()).unwrap();
    assert!(!archive.root.join(stream.as_str()).exists());
}

#[test]
fn path_and_day_types_reject_hostile_components() {
    assert!(JournalStreamId::parse("../escape".into()).is_err());
    assert!(JournalStreamId::parse("a/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()).is_err());
    assert!(ReceptionDay::parse("../../etc".into()).is_err());
    assert!(ReceptionDay::parse("2024-1-001".into()).is_err());
}

#[test]
fn traversal_bound_covers_admitted_target_stream_retention_scale() {
    let supported = std::hint::black_box(4096usize) * (1 + 32 * 31);
    assert!(MAX_ARCHIVE_FILES >= supported);
}

#[test]
fn periodic_prune_removes_only_days_before_cutoff_and_releases_quota() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(&directory, 1024 * 1024);
    let stream = stream("g");
    let old = day("2024-01-01");
    let current = day("2024-01-02");
    archive
        .append(
            &stream,
            &old,
            &batch(b"{\"fields\":{\"safe_to_share\":true},\"old\":1}\n", 1),
        )
        .unwrap();
    let current_frame = archive
        .append(
            &stream,
            &current,
            &batch(b"{\"fields\":{\"safe_to_share\":true},\"new\":1}\n", 2),
        )
        .unwrap();
    archive.prune_before(&current).unwrap();
    assert!(!archive.path(&stream, &old).exists());
    assert!(archive.path(&stream, &current).exists());
    assert_eq!(
        archive.used_bytes().unwrap(),
        current_frame.end - current_frame.start
    );
}

#[test]
fn indeterminate_sync_failure_poison_blocks_sibling_admission_until_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let hook = Arc::new(TestAppendHook {
        entered: std::sync::Barrier::new(1),
        release: std::sync::Barrier::new(1),
        fail_after_write: std::sync::atomic::AtomicBool::new(true),
    });
    let archive = archive(&directory, 1024 * 1024).with_append_hook(hook);
    let payload = batch(b"{\"fields\":{\"safe_to_share\":true}}\n", 1);
    assert!(matches!(
        archive.append(&stream("h"), &day("2024-01-01"), &payload),
        Err(ArchiveError::Injected)
    ));
    assert!(matches!(
        archive.append(&stream("i"), &day("2024-01-01"), &payload),
        Err(ArchiveError::Poisoned)
    ));
    archive.recover(Vec::new()).unwrap();
    archive
        .append(&stream("i"), &day("2024-01-01"), &payload)
        .unwrap();
}
