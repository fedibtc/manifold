use fedi_decentralized_service_fleet_manager::{FederationId, SeatId};
use fman_core::wallet::{Msats, PayoutRequestId};

use crate::payout_job::{PayoutJob, PayoutJobOperation, PayoutScope};
use crate::payout_job_status::PayoutJobStatus;
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::{OutgoingOperation, OutgoingRail, OutgoingState, WalletDrainStatus};

fn operation_id() -> PayoutOperationId {
    PayoutOperationId::parse("0f7c1b9a3e5d4c2b8a6f0e1d2c3b4a5960718293a4b5c6d7e8f90a1b2c3d4e5f")
        .unwrap()
}
fn job() -> PayoutJob {
    PayoutJob {
        request_id: PayoutRequestId::parse("fixture-payout-request").unwrap(),
        scope: PayoutScope::PaymentFederation {
            federation_id: FederationId("fed1fixturepayment".into()),
        },
        destination: "operator@example.com".into(),
        operation: Some(PayoutJobOperation {
            operation_id: operation_id(),
            amount_msat: 250_000,
            committed_at_ms: 1_753_600_002_000,
        }),
        created_at_ms: 1_753_600_001_000,
    }
}
fn fixture(path: &str) -> serde_json::Value {
    serde_json::from_str(match path {
        "job" => {
            include_str!("../../../../operator-ui/packages/types/fixtures/fman_payout_job.json")
        }
        "status" => include_str!(
            "../../../../operator-ui/packages/types/fixtures/fman_payout_job_status.json"
        ),
        "federations" => include_str!(
            "../../../../operator-ui/packages/types/fixtures/fman_payment_federations.json"
        ),
        "guardian" => {
            include_str!("../../../../operator-ui/packages/types/fixtures/fman_guardian_fees.json")
        }
        _ => unreachable!(),
    })
    .unwrap()
}

#[test]
fn internal_payout_projection_matches_committed_operator_contracts() {
    assert_eq!(
        serde_json::to_value(job().to_wire()).unwrap(),
        fixture("job")
    );
    let status = PayoutJobStatus {
        job: job(),
        payout: Some(OutgoingOperation::new(
            operation_id(),
            OutgoingRail::Lnv2,
            OutgoingState::Succeeded,
            250_000,
            251_000,
            false,
        )),
    };
    assert_eq!(
        serde_json::to_value(status.to_wire()).unwrap(),
        fixture("status")
    );
}

#[test]
fn guardian_scope_invite_conversion_has_the_operator_shape() {
    let invite = fedimint_core::invite_code::InviteCode::new(
        fedimint_core::util::SafeUrl::parse("https://guardian.example").unwrap(),
        fedimint_core::PeerId::from(0),
        "02".repeat(32).parse().unwrap(),
        None,
    );
    let scope = PayoutScope::GuardianFee {
        federation_id: FederationId(invite.federation_id().to_string()),
        seat_id: SeatId::new("01".repeat(32)).unwrap(),
        invite_code: invite.clone(),
    };
    assert_eq!(
        serde_json::to_value(scope.to_wire()).unwrap(),
        serde_json::json!({
            "kind": "guardian_fee",
            "federation_id": invite.federation_id().to_string(),
            "seat_id": "01".repeat(32),
            "invite_code": invite.to_string(),
        })
    );
}

#[test]
fn internal_drain_projection_matches_committed_operator_contracts() {
    let payment = WalletDrainStatus::new(
        Ok(Msats(350)),
        Ok(Msats(0)),
        Ok(vec![OutgoingOperation::new(
            PayoutOperationId::parse(
                "17d55b3cb3e9cd25035f6b8cf296284d4445ba9ea8568ccf5ab198d4df27a5ce",
            )
            .unwrap(),
            OutgoingRail::Lnv1,
            OutgoingState::Pending,
            3_960_152,
            3_980_000,
            true,
        )]),
        1,
    );
    assert_eq!(
        serde_json::to_value(payment.to_wire()).unwrap(),
        fixture("federations")["federations"][0]["wallet"]
    );
    let guardian =
        WalletDrainStatus::new(Ok(Msats(8_000_000)), Ok(Msats(7_950_000)), Ok(vec![]), 0);
    assert_eq!(
        serde_json::to_value(guardian.to_wire()).unwrap(),
        fixture("guardian")["wallet"]
    );
}
