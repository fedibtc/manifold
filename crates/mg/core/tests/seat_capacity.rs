use super::*;

const GIB: u64 = 1024 * MIB;

#[test]
fn one_seat_per_one_point_two_gib_capped_at_eight() {
    let budget = 1_229 * MIB;
    // Multiples of the 1.2 GiB budget recommend exactly their multiplier...
    for seats in 1..=8u64 {
        assert_eq!(
            recommended_max_seats(seats * budget),
            seats as u32,
            "at exactly {seats} x 1.2 GiB"
        );
        // ...and one byte less truncates down a seat.
        assert_eq!(
            recommended_max_seats(seats * budget - 1),
            seats as u32 - 1,
            "one byte short of {seats} x 1.2 GiB"
        );
    }
    // Below the first budget the host is too small to sell seats at all.
    assert_eq!(recommended_max_seats(0), 0);
    assert_eq!(recommended_max_seats(GIB), 0);
    // The cap holds arbitrarily high.
    assert_eq!(recommended_max_seats(9 * budget), 8);
    assert_eq!(recommended_max_seats(u64::MAX), 8);
}

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
    let mut smoothed = Smoothed::default();
    assert_eq!(smoothed.push(6), 6);
    assert_eq!(smoothed.push(4), 4);
    assert_eq!(smoothed.push(8), 4);
    assert_eq!(smoothed.push(8), 4);
    // The 4 has now left the window.
    assert_eq!(smoothed.push(8), 8);
    assert_eq!(smoothed.push(7), 7);
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
