use super::*;

#[test]
fn wrong_type_fifo_is_rejected_without_blocking() {
    let temp = tempfile::tempdir().unwrap();
    let directory = AnchoredDirectory::open(temp.path()).unwrap();
    rustix::fs::mkfifoat(
        &directory.0,
        "incarnation",
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();

    assert!(
        directory
            .open_file(
                "incarnation",
                rustix::fs::OFlags::RDONLY,
                rustix::fs::Mode::empty()
            )
            .is_err()
    );
}
