use super::*;

fn directory(temp: &tempfile::TempDir) -> AnchoredDirectory {
    AnchoredDirectory::open(temp.path()).unwrap()
}

#[test]
fn malformed_final_fails_closed_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"bad").unwrap();

    assert!(open_next_segment(&directory(&temp), &BTreeMap::new()).is_err());
    assert_eq!(
        std::fs::read(temp.path().join(NEXT_SEGMENT_FILE)).unwrap(),
        b"bad"
    );
}

#[test]
fn valid_and_malformed_pending_recover_without_regression() {
    let valid = tempfile::tempdir().unwrap();
    std::fs::write(valid.path().join(NEXT_SEGMENT_PENDING_FILE), b"7\n").unwrap();
    assert_eq!(
        open_next_segment(&directory(&valid), &BTreeMap::new()).unwrap(),
        7
    );
    assert!(!valid.path().join(NEXT_SEGMENT_PENDING_FILE).exists());

    let malformed = tempfile::tempdir().unwrap();
    std::fs::write(malformed.path().join(NEXT_SEGMENT_PENDING_FILE), b"partial").unwrap();
    assert_eq!(
        open_next_segment(&directory(&malformed), &BTreeMap::new()).unwrap(),
        0
    );
}

#[test]
fn reservation_must_be_ahead_of_every_segment() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"2\n").unwrap();
    let segments = BTreeMap::from([(2, "events-2.jsonl".to_owned())]);
    assert!(open_next_segment(&directory(&temp), &segments).is_err());
}

#[test]
fn publication_evidence_is_completed_before_pending_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let final_path = temp.path().join(NEXT_SEGMENT_FILE);
    let pending_path = temp.path().join(NEXT_SEGMENT_PENDING_FILE);
    std::fs::write(&final_path, b"5\n").unwrap();
    std::fs::hard_link(&final_path, &pending_path).unwrap();

    assert_eq!(
        open_next_segment(&directory(&temp), &BTreeMap::new()).unwrap(),
        5
    );
    assert!(!pending_path.exists());
    assert_eq!(std::fs::read(final_path).unwrap(), b"5\n");
}

#[test]
fn newer_pending_reservation_replaces_old_final() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"5\n").unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_PENDING_FILE), b"6\n").unwrap();

    assert_eq!(
        open_next_segment(&directory(&temp), &BTreeMap::new()).unwrap(),
        6
    );
    assert_eq!(
        std::fs::read(temp.path().join(NEXT_SEGMENT_FILE)).unwrap(),
        b"6\n"
    );
}

#[test]
fn established_reservation_never_falls_back_to_legacy_after_both_names_disappear() {
    let temp = tempfile::tempdir().unwrap();
    let directory = directory(&temp);
    assert_eq!(open_next_segment(&directory, &BTreeMap::new()).unwrap(), 0);
    std::fs::remove_file(temp.path().join(NEXT_SEGMENT_FILE)).unwrap();

    assert!(open_next_segment(&directory, &BTreeMap::new()).is_err());
    assert!(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE).exists());
}

#[test]
fn external_hardlinks_reject_final_and_newer_pending() {
    let final_link = tempfile::tempdir().unwrap();
    std::fs::write(final_link.path().join(NEXT_SEGMENT_FILE), b"5\n").unwrap();
    std::fs::hard_link(
        final_link.path().join(NEXT_SEGMENT_FILE),
        final_link.path().join("external"),
    )
    .unwrap();
    assert!(open_next_segment(&directory(&final_link), &BTreeMap::new()).is_err());

    let pending_link = tempfile::tempdir().unwrap();
    std::fs::write(pending_link.path().join(NEXT_SEGMENT_FILE), b"5\n").unwrap();
    std::fs::write(pending_link.path().join(NEXT_SEGMENT_PENDING_FILE), b"6\n").unwrap();
    std::fs::hard_link(
        pending_link.path().join(NEXT_SEGMENT_PENDING_FILE),
        pending_link.path().join("external"),
    )
    .unwrap();
    assert!(open_next_segment(&directory(&pending_link), &BTreeMap::new()).is_err());
    assert_eq!(
        std::fs::read(pending_link.path().join(NEXT_SEGMENT_FILE)).unwrap(),
        b"5\n"
    );

    let linked_final_with_pending = tempfile::tempdir().unwrap();
    let final_path = linked_final_with_pending.path().join(NEXT_SEGMENT_FILE);
    std::fs::write(&final_path, b"5\n").unwrap();
    std::fs::hard_link(
        &final_path,
        linked_final_with_pending.path().join("external"),
    )
    .unwrap();
    std::fs::write(
        linked_final_with_pending
            .path()
            .join(NEXT_SEGMENT_PENDING_FILE),
        b"6\n",
    )
    .unwrap();
    assert!(open_next_segment(&directory(&linked_final_with_pending), &BTreeMap::new()).is_err());
    assert_eq!(std::fs::read(final_path).unwrap(), b"5\n");
    assert_eq!(
        std::fs::read(
            linked_final_with_pending
                .path()
                .join(NEXT_SEGMENT_PENDING_FILE)
        )
        .unwrap(),
        b"6\n"
    );
}

#[test]
fn markerless_final_is_upgraded_before_both_name_loss_can_regress() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"100\n").unwrap();
    let directory = directory(&temp);

    assert_eq!(
        open_next_segment(&directory, &BTreeMap::new()).unwrap(),
        100
    );
    assert!(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE).exists());
    std::fs::remove_file(temp.path().join(NEXT_SEGMENT_FILE)).unwrap();
    assert!(open_next_segment(&directory, &BTreeMap::new()).is_err());
}

#[test]
fn marker_creation_interruption_retries_from_published_final() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"12\n").unwrap();

    assert_eq!(
        open_next_segment(&directory(&temp), &BTreeMap::new()).unwrap(),
        12
    );
    assert_eq!(
        std::fs::read(temp.path().join(NEXT_SEGMENT_FILE)).unwrap(),
        b"12\n"
    );
    assert!(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE).exists());
}

#[test]
fn partial_marker_is_repaired_after_final_publication() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"12\n").unwrap();
    std::fs::write(
        temp.path().join(SEGMENT_RESERVATION_MARKER_FILE),
        b"partial",
    )
    .unwrap();

    assert_eq!(
        open_next_segment(&directory(&temp), &BTreeMap::new()).unwrap(),
        12
    );
    assert_eq!(
        std::fs::read(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE)).unwrap(),
        b"1\n"
    );
}

#[test]
fn valid_marker_is_resynced_and_reused_after_prior_sync_ambiguity() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(NEXT_SEGMENT_FILE), b"12\n").unwrap();
    std::fs::write(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE), b"1\n").unwrap();

    assert_eq!(
        open_next_segment(&directory(&temp), &BTreeMap::new()).unwrap(),
        12
    );
    assert_eq!(
        std::fs::read(temp.path().join(SEGMENT_RESERVATION_MARKER_FILE)).unwrap(),
        b"1\n"
    );
}
