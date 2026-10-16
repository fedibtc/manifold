use super::*;
use crate::backup::read_guardian_archive;
use crate::test_support::write_legacy_layout;

async fn file_names(dir: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        names.push(entry.file_name().into_string().unwrap());
    }
    names.sort();
    names
}

/// A seat in the legacy encrypted layout is nothing to back up until it is
/// migrated; the migration decrypts the private config under the seat's
/// `api_auth` into `private.json` and sets the legacy files aside as `*.old`.
/// Running it again, or on a seat already migrated, changes nothing.
#[tokio::test]
async fn a_legacy_layout_migrates_to_the_plaintext_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let seat_dir = dir.path().join("data");
    tokio::fs::create_dir_all(&seat_dir).await.unwrap();
    let private_json = r#"{"shares":"deadbeef"}"#;
    write_legacy_layout(&seat_dir, "derived-api-auth", private_json).await;
    assert!(read_guardian_archive(&seat_dir).await.unwrap().is_none());

    assert_eq!(
        migrate_seat_data_dir(&seat_dir, "derived-api-auth")
            .await
            .unwrap(),
        LayoutMigration::Converted
    );
    let migrated = [
        "consensus.json",
        "local.json",
        "password.private.old",
        "private.encrypt.old",
        "private.json",
        "private.salt.old",
    ];
    assert_eq!(file_names(&seat_dir).await, migrated);
    let archive = read_guardian_archive(&seat_dir).await.unwrap().unwrap();
    assert_eq!(archive.private_json, private_json);

    assert_eq!(
        migrate_seat_data_dir(&seat_dir, "derived-api-auth")
            .await
            .unwrap(),
        LayoutMigration::AlreadyPlaintext
    );
    assert_eq!(
        read_guardian_archive(&seat_dir).await.unwrap().unwrap(),
        archive
    );

    // Interrupted after `private.json` landed: the sweep finishes the job
    // without touching the config fedimintd already prefers.
    tokio::fs::write(seat_dir.join("password.private"), "stale")
        .await
        .unwrap();
    assert_eq!(
        migrate_seat_data_dir(&seat_dir, "another-password")
            .await
            .unwrap(),
        LayoutMigration::AlreadyPlaintext
    );
    assert_eq!(file_names(&seat_dir).await, migrated);
    assert_eq!(
        read_guardian_archive(&seat_dir).await.unwrap().unwrap(),
        archive
    );
}

/// The wrong credential migrates nothing: the legacy files stay as they were,
/// so the child keeps running from them and nothing is published.
#[tokio::test]
async fn a_legacy_layout_under_another_password_is_left_alone() {
    let dir = tempfile::TempDir::new().unwrap();
    let seat_dir = dir.path().join("data");
    tokio::fs::create_dir_all(&seat_dir).await.unwrap();
    write_legacy_layout(&seat_dir, "derived-api-auth", r#"{"shares":"deadbeef"}"#).await;
    let before = file_names(&seat_dir).await;

    let refused = migrate_seat_data_dir(&seat_dir, "not-this-seats-password")
        .await
        .unwrap_err();
    assert!(
        matches!(refused, LayoutMigrationError::Decrypt(_)),
        "{refused}"
    );
    assert_eq!(file_names(&seat_dir).await, before);
    assert!(read_guardian_archive(&seat_dir).await.unwrap().is_none());

    // An unformed seat has nothing to migrate either.
    let empty = dir.path().join("empty");
    tokio::fs::create_dir_all(&empty).await.unwrap();
    assert_eq!(
        migrate_seat_data_dir(&empty, "derived-api-auth")
            .await
            .unwrap(),
        LayoutMigration::NothingToDo
    );
    assert!(file_names(&empty).await.is_empty());
}
