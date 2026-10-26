//! Seat capacity and seat readiness decoding tests.

use super::*;

fn record(timestamp: &str, message: &str, fields: &str) -> serde_json::Value {
    serde_json::from_str(&format!(
        "{{\"timestamp\":\"{timestamp}\",\"level\":\"INFO\",\"fields\":{{\"message\":\"{message}\",\"safe_to_share\":true,{fields}}},\"target\":\"mg\"}}"
    ))
    .unwrap()
}

const CAPACITY: &str = "\"mg_version\":\"0.1.6\",\"memory_max_seats\":9,\"policy_max_seats\":12,\"available_seats\":6,\"accepting_seats\":true";

const READINESS: &str = "\"ready\":false,\"relay\":\"pass\",\"discovery\":\"pass\",\"bitcoin\":\"pass\",\"uptime\":\"uptime_7d_low\",\"uptime_1d_pct\":99.4,\"uptime_7d_pct\":88.1,\"uptime_30d_pct\":95,\"min_uptime_1d_pct\":70,\"min_uptime_7d_pct\":90,\"min_uptime_30d_pct\":93";

fn capacity(fields: &str) -> Option<Stated<SeatCapacity>> {
    let mut latest = LatestReports::default();
    latest.observe(&record("2026-10-21T12:00:00Z", SEAT_CAPACITY_EVENT, fields));
    latest.seat_capacity
}

fn readiness(fields: &str) -> Option<Stated<SeatReadiness>> {
    let mut latest = LatestReports::default();
    latest.observe(&record(
        "2026-10-21T12:00:00Z",
        SEAT_READINESS_EVENT,
        fields,
    ));
    latest.seat_readiness
}

#[test]
fn keeps_the_last_valid_report_of_each_kind() {
    let mut latest = LatestReports::default();
    latest.observe(&record(
        "2026-10-21T12:00:00.000000Z",
        SEAT_CAPACITY_EVENT,
        CAPACITY,
    ));
    latest.observe(&record(
        "2026-10-21T12:10:00.250000Z",
        SEAT_CAPACITY_EVENT,
        &CAPACITY.replace("\"available_seats\":6", "\"available_seats\":5"),
    ));
    // A later invalid record leaves the last valid one in place.
    latest.observe(&record(
        "2026-10-21T12:20:00Z",
        SEAT_CAPACITY_EVENT,
        &CAPACITY.replace("\"available_seats\":6", "\"available_seats\":10"),
    ));
    latest.observe(&record(
        "2026-10-21T12:05:00Z",
        SEAT_READINESS_EVENT,
        READINESS,
    ));
    latest.observe(&record(
        "2026-10-21T12:30:00Z",
        "relay connected",
        "\"n\":1",
    ));

    let capacity = latest.seat_capacity.unwrap();
    assert_eq!(capacity.at_ms, 1_792_584_600_250);
    assert_eq!(capacity.report.available_seats, 5);
    assert_eq!(capacity.report.max_seats, None);
    let readiness = latest.seat_readiness.unwrap();
    assert!(!readiness.report.ready);
    assert_eq!(
        readiness.report.uptime,
        Some(ReadinessCode("uptime_7d_low".into()))
    );
    assert_eq!(readiness.report.uptime_30d_pct, Some(Percent(95.0)));
}

#[test]
fn reports_from_releases_with_fewer_fields_decode() {
    let older = capacity("\"max_seats\":3,\"available_seats\":2,\"accepting_seats\":true")
        .unwrap()
        .report;
    assert_eq!(
        older,
        SeatCapacity {
            available_seats: 2,
            accepting_seats: true,
            max_seats: Some(3),
            memory_max_seats: None,
            policy_max_seats: None,
            mg_version: None,
        }
    );
    let readiness = readiness(
        "\"ready\":true,\"relay\":\"pass\",\"discovery\":\"pass\",\"bitcoin\":\"not_applicable\"",
    )
    .unwrap()
    .report;
    assert!(readiness.ready);
    assert_eq!(readiness.uptime, None);
    assert_eq!(readiness.uptime_7d_pct, None);
}

#[test]
fn seat_counts_must_agree_with_every_stated_limit() {
    let max = SETUP_PAYMENT_MAX_SEATS_PER_MG;
    assert!(
        capacity(&CAPACITY.replace("\"available_seats\":6", "\"available_seats\":9")).is_some()
    );
    for fields in [
        // More free seats than the memory limit, or than the operator limit.
        CAPACITY.replace("\"available_seats\":6", "\"available_seats\":10"),
        format!("\"max_seats\":1,{CAPACITY}"),
        // A policy limit above the policy maximum.
        CAPACITY.replace(
            "\"policy_max_seats\":12",
            &format!("\"policy_max_seats\":{}", max + 1),
        ),
        // No limit at all.
        "\"available_seats\":1,\"accepting_seats\":true".to_owned(),
    ] {
        assert_eq!(capacity(&fields), None, "{fields}");
    }
    // Before 0.1.4 the operator limit is the only one and has no upper bound.
    let older = format!(
        "\"max_seats\":{},\"available_seats\":{max},\"accepting_seats\":true",
        max + 44
    );
    assert_eq!(
        capacity(&older).map(|report| report.report.max_seats),
        Some(Some(max + 44))
    );
}

#[test]
fn rejects_out_of_range_or_unbounded_values() {
    for fields in [
        CAPACITY.replace("0.1.6", "0.1.6 \\n"),
        CAPACITY.replace("0.1.6", &"9".repeat(65)),
        CAPACITY.replace("\"available_seats\":6", "\"available_seats\":-1"),
    ] {
        assert_eq!(capacity(&fields), None, "{fields}");
    }
    for fields in [
        READINESS.replace("99.4", "100.1"),
        READINESS.replace("\"pass\"", "\"Pass\""),
        READINESS.replace("uptime_7d_low", &"x".repeat(33)),
    ] {
        assert_eq!(readiness(&fields), None, "{fields}");
    }
}

#[test]
fn a_record_without_a_valid_time_is_ignored() {
    for timestamp in ["not a time", "1969-12-31T23:59:59Z"] {
        let mut latest = LatestReports::default();
        latest.observe(&record(timestamp, SEAT_CAPACITY_EVENT, CAPACITY));
        assert_eq!(latest.seat_capacity, None, "{timestamp}");
    }
    let mut untimed = record("2026-10-21T12:00:00Z", SEAT_CAPACITY_EVENT, CAPACITY);
    untimed.as_object_mut().unwrap().remove("timestamp");
    let mut latest = LatestReports::default();
    latest.observe(&untimed);
    assert_eq!(latest.seat_capacity, None);
}

#[test]
fn a_stated_time_never_makes_a_report_fresher_than_its_arrival() {
    let stated = |at_ms| Stated { at_ms, report: () };
    assert_eq!(stated(1_000).observed_at_ms(5_000), 1_000);
    assert_eq!(stated(9_000).observed_at_ms(5_000), 5_000);
}

#[test]
fn stored_reports_validate_again_when_read() {
    let report = capacity(CAPACITY).unwrap().report;
    let stored = serde_json::to_string(&report).unwrap();
    assert_eq!(
        serde_json::from_str::<SeatCapacity>(&stored).unwrap(),
        report
    );
    let tampered = stored.replace("\"available_seats\":6", "\"available_seats\":10");
    assert!(serde_json::from_str::<SeatCapacity>(&tampered).is_err());
}

#[test]
fn renders_rfc3339_within_four_digit_years() {
    assert_eq!(
        format_rfc3339_millis(1_792_584_600_250).as_deref(),
        Some("2026-10-21T12:10:00.250Z")
    );
    assert_eq!(format_rfc3339_millis(-1), None);
    assert_eq!(format_rfc3339_millis(253_402_300_800_000), None);
}
