use super::*;

const GIB: u64 = 1024 * MIB;

#[test]
fn runtime_limit_counts_memory_the_seats_already_hold() {
    let budget = GIB;
    // 2.5 GiB available and nothing running: two seats fit.
    assert_eq!(memory_max_seats(5 * GIB / 2, 0, budget), 2);
    // The same host after two seats took 1 GiB privately: still two, not
    // two running plus two more.
    assert_eq!(memory_max_seats(3 * GIB / 2, GIB, budget), 2);
    // Running seats that outgrow the budget leave a limit below them.
    assert_eq!(memory_max_seats(GIB / 2, GIB / 4, budget), 0);
    assert_eq!(memory_max_seats(u64::MAX, u64::MAX, 1), u32::MAX);
}

#[test]
fn smoothing_lowers_at_once_and_raises_after_three_agreeing_samples() {
    let mut smoothed = Smoothed::starting_at(6);
    assert_eq!(smoothed.push(4), 4);
    assert_eq!(smoothed.push(8), 4);
    assert_eq!(smoothed.push(8), 4);
    // The 4 has now left the window.
    assert_eq!(smoothed.push(8), 8);
    assert_eq!(smoothed.push(7), 7);
}

#[test]
fn a_restart_needs_three_samples_to_raise_the_stored_limit() {
    let mut smoothed = Smoothed::starting_at(1);
    assert_eq!(smoothed.push(12), 1);
    assert_eq!(smoothed.push(12), 1);
    assert_eq!(smoothed.push(12), 12);
    // A lower first sample still applies at once.
    assert_eq!(Smoothed::starting_at(5).push(2), 2);
}

#[test]
fn cgroup_limits_apply_from_the_own_group_and_every_ancestor() {
    let dirs = cgroup_dirs("0::/system.slice/mg.service\n");
    assert_eq!(
        dirs,
        [
            "/sys/fs/cgroup/system.slice/mg.service",
            "/sys/fs/cgroup/system.slice",
            "/sys/fs/cgroup",
        ]
        .map(std::path::PathBuf::from)
    );
    // A private cgroup namespace sees itself at the root.
    assert_eq!(
        cgroup_dirs("0::/\n"),
        [std::path::PathBuf::from("/sys/fs/cgroup")]
    );
    // Without a unified-hierarchy entry (cgroup v1 only), no limit applies.
    assert!(cgroup_dirs("4:memory:/docker/abc\n").is_empty());
}

#[test]
fn parses_mem_available() {
    let meminfo =
        "MemTotal:       16118776 kB\nMemFree:         391384 kB\nMemAvailable:    5527500 kB\n";
    assert_eq!(
        parse_mem_available_bytes(meminfo).unwrap(),
        5_527_500 * 1024
    );
    assert!(parse_mem_available_bytes("MemTotal: 1 kB\n").is_err());
}

#[test]
fn cgroup_headroom_applies_only_under_a_numeric_limit() {
    assert_eq!(
        cgroup_headroom_bytes(Some("4294967296\n"), Some("1073741824\n")),
        Some(3 * GIB)
    );
    assert_eq!(
        cgroup_headroom_bytes(Some("max\n"), Some("1073741824\n")),
        None
    );
    assert_eq!(cgroup_headroom_bytes(None, Some("1\n")), None);
    assert_eq!(cgroup_headroom_bytes(Some("1\n"), Some("5\n")), Some(0));
}
