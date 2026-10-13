use std::os::unix::fs::PermissionsExt as _;

use super::*;

fn secure_file(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn no_replace_move_preserves_competing_journal() {
    let temp = tempfile::tempdir().unwrap();
    let directory = open_directory(temp.path()).unwrap();
    secure_file(&temp.path().join("source"), "source");
    secure_file(&temp.path().join("journal"), "competing");

    assert!(rename_entry(&directory, OsStr::new("source"), OsStr::new("journal")).is_err());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("journal")).unwrap(),
        "competing"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("source")).unwrap(),
        "source"
    );
}

#[test]
fn publication_rejects_journal_substitution() {
    let temp = tempfile::tempdir().unwrap();
    let directory = open_directory(temp.path()).unwrap();
    let source = temp.path().join("source");
    let journal = temp.path().join("journal");
    secure_file(&source, "validated");
    let retained = open_entry(&directory, OsStr::new("source"))
        .unwrap()
        .unwrap();

    let error = publish_source_with_hook(
        &directory,
        OsStr::new("source"),
        OsStr::new("journal"),
        &retained,
        &source,
        &journal,
        || {
            std::fs::rename(&journal, temp.path().join("displaced"))?;
            secure_file(&journal, "substitute");
            Ok(())
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("was replaced"));
    assert_eq!(std::fs::read_to_string(journal).unwrap(), "substitute");
}
