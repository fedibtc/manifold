use crate::allocation_store::StabilityPoolAllocationStep;
use crate::stability_deposit::{StabilityDepositOperationId, StabilityDepositSubmission};
use fedi_decentralized_service_liquidity_manager::Sats;

#[test]
fn serde_uses_full_hex_and_decodes_existing_rows() {
    let existing = "\"79f07f079505910d34f875b4b80907252dc29ca2645486d1d1520229b761c562\"";
    let operation_id: StabilityDepositOperationId =
        serde_json::from_str(existing).expect("existing full-hex ID decodes");

    assert_eq!(
        serde_json::to_string(&operation_id).expect("operation ID encodes"),
        existing
    );
}

#[test]
fn complete_submission_tuple_round_trips_without_regeneration() {
    let submission = StabilityDepositSubmission::generate(Sats(42_000), 1_234);
    let mut step = StabilityPoolAllocationStep::default();
    step.begin_submission(submission);

    let encoded = serde_json::to_string(&step).expect("step encodes");
    let decoded: StabilityPoolAllocationStep =
        serde_json::from_str(&encoded).expect("step decodes");

    assert_eq!(
        decoded
            .submitting_submission()
            .expect("tuple is complete")
            .expect("submission is in flight"),
        submission
    );
}

#[test]
fn generated_operation_ids_are_not_request_derived_constants() {
    assert_ne!(
        StabilityDepositOperationId::generate(),
        StabilityDepositOperationId::generate()
    );
}

#[test]
fn malformed_and_zero_persisted_tuples_fail_closed_after_decoding() {
    for encoded in [
        r#"{"sp_deposit_status":"submitting","sp_deposit_operation_id":"not-hex","sp_deposit_amount":42,"sp_deposit_min_fee_rate_ppb":1}"#,
        r#"{"sp_deposit_status":"submitting","sp_deposit_operation_id":"79f07f079505910d34f875b4b80907252dc29ca2645486d1d1520229b761c562","sp_deposit_amount":0,"sp_deposit_min_fee_rate_ppb":1}"#,
    ] {
        let step: StabilityPoolAllocationStep =
            serde_json::from_str(encoded).expect("raw diagnostic state remains readable");
        assert!(
            step.submitting_submission().is_err(),
            "invalid tuple must not reach submission"
        );
    }
}
