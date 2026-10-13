use super::*;

#[test]
fn one_seat_per_one_and_a_half_gib_capped_at_eight() {
    // Multiples of the 1.5 GiB budget recommend exactly their multiplier...
    for seats in 1..=8u64 {
        assert_eq!(
            recommended_max_seats(seats * PER_SEAT_BYTES),
            seats as u32,
            "at exactly {seats} x 1.5 GiB"
        );
        // ...and one byte less truncates down a seat.
        assert_eq!(
            recommended_max_seats(seats * PER_SEAT_BYTES - 1),
            seats as u32 - 1,
            "one byte short of {seats} x 1.5 GiB"
        );
    }
    // Below the first budget the host is too small to sell seats at all.
    assert_eq!(recommended_max_seats(0), 0);
    assert_eq!(recommended_max_seats(GIB), 0);
    // The cap holds arbitrarily high: 9 x 1.5 GiB and beyond stay at 8.
    assert_eq!(recommended_max_seats(9 * PER_SEAT_BYTES), 8);
    assert_eq!(recommended_max_seats(u64::MAX), 8);
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
