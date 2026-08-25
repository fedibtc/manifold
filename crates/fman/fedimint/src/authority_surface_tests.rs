//! Source-level guard for the value-moving client-authority enumeration.

const VALUE_MOVING_SOURCES: &[(&str, &str)] = &[
    ("lib.rs", include_str!("lib.rs")),
    ("claim_worker.rs", include_str!("claim_worker.rs")),
    ("payee.rs", include_str!("payee.rs")),
    ("guardian_fee.rs", include_str!("guardian_fee.rs")),
    ("payout_store.rs", include_str!("payout_store.rs")),
    ("payout_worker.rs", include_str!("payout_worker.rs")),
    ("payout_native.rs", include_str!("payout_native.rs")),
    ("payout_observer.rs", include_str!("payout_observer.rs")),
    ("drain_status.rs", include_str!("drain_status.rs")),
    ("core/wallet.rs", include_str!("../../core/src/wallet.rs")),
    (
        "core/fleet/payout.rs",
        include_str!("../../core/src/fleet/payout.rs"),
    ),
    (
        "core/guardian_fee.rs",
        include_str!("../../core/src/guardian_fee.rs"),
    ),
];

#[test]
fn value_moving_modules_do_not_import_child_or_guardian_authority() {
    const FORBIDDEN: &[&str] = &[
        "SeatApiAuth",
        "FedimintApi",
        "api_auth",
        "meta_submit",
        "run_dkg(",
        "child_data",
    ];

    for (path, source) in VALUE_MOVING_SOURCES {
        for forbidden in FORBIDDEN {
            assert!(
                !source.contains(forbidden),
                "{path} introduced forbidden authority vocabulary `{forbidden}`"
            );
        }
    }
}

#[test]
fn known_value_moving_client_calls_stay_explicit() {
    const CALLS: &[(&str, &str, usize)] = &[
        ("payee.rs", ".reissue_external_notes(", 1),
        ("payee.rs", "mint.receive(", 1),
        ("guardian_fee.rs", ".withdraw_idle_balance(", 1),
        ("guardian_fee.rs", ".withdraw(AccountType::BtcDepositor", 1),
        ("payout_native.rs", ".send(\n", 1),
        ("payout_native.rs", ".pay_bolt11_invoice(", 1),
        ("payout_observer.rs", ".await_outgoing_payment(", 1),
        (
            "payout_observer.rs",
            ".await_final_send_operation_state(",
            1,
        ),
    ];

    for (path, needle, expected) in CALLS {
        let source = VALUE_MOVING_SOURCES
            .iter()
            .find_map(|(candidate, source)| (*candidate == *path).then_some(*source))
            .expect("enumerated source exists");
        assert_eq!(
            source.matches(needle).count(),
            *expected,
            "update the value-moving authority proof when `{needle}` changes in {path}"
        );
    }
}
