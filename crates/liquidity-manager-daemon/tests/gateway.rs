//! Unit tests for the gatewayd adapter's payment-log reading.

use std::str::FromStr;

use bitcoin::{OutPoint, Txid};
use fedimint_core::Amount;

use super::*;

const TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn txid() -> Txid {
    Txid::from_str(TXID).expect("valid test txid")
}

fn operation_id(byte: u8) -> OperationId {
    OperationId([byte; 32])
}

fn deposit_address() -> Address<NetworkUnchecked> {
    "bcrt1q0xcqpzrky6eff2g52qdye53xkk9jxkvrl4xfg5"
        .parse()
        .expect("valid test address")
}

/// Builds the log entry gatewayd would return for one logged event.
fn log_entry<E: Event>(id: u64, event: &E) -> PersistedLogEntry {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "kind": E::KIND,
        "module": serde_json::Value::Null,
        "ts_usecs": 0,
        "payload": serde_json::to_value(event).expect("event serializes"),
    }))
    .expect("entry deserializes")
}

fn receive(operation_id: OperationId, outpoint: Option<OutPoint>) -> ReceivePaymentEvent {
    ReceivePaymentEvent {
        operation_id,
        value: bitcoin::Amount::from_sat(25_000),
        fee: bitcoin::Amount::from_sat(500),
        address: deposit_address(),
        outpoint,
    }
}

fn receive_update(
    operation_id: OperationId,
    status: ReceivePaymentStatus,
) -> ReceivePaymentUpdateEvent {
    ReceivePaymentUpdateEvent {
        operation_id,
        status,
    }
}

/// A wallet v1 federation reports one claim per confirmed peg-in, and the
/// event names the output it claimed.
#[test]
fn a_wallet_v1_deposit_is_read_as_a_claim_of_its_own_output() {
    let entries = vec![log_entry(
        1,
        &DepositConfirmed {
            txid: txid(),
            out_idx: 2,
            amount: Amount::from_sats(25_000),
        },
    )];

    assert_eq!(
        deposit_claims_from_log(&entries),
        vec![GatewayDepositClaim {
            txid: TXID.to_owned(),
            out_idx: 2,
            amount: Sats(25_000),
        }]
    );
}

/// A walletv2 receive is an attempt until the federation accepts the claiming
/// transaction, so the submitted event alone attributes nothing.
#[test]
fn a_walletv2_receive_is_a_claim_only_once_the_federation_accepts_it() {
    let operation = operation_id(1);
    let outpoint = OutPoint {
        txid: txid(),
        vout: 2,
    };
    let submitted = vec![log_entry(1, &receive(operation, Some(outpoint)))];

    assert_eq!(
        deposit_claims_from_log(&submitted),
        vec![],
        "a submitted receive is not yet a claimed deposit"
    );

    let accepted = vec![
        log_entry(2, &receive_update(operation, ReceivePaymentStatus::Success)),
        log_entry(1, &receive(operation, Some(outpoint))),
    ];

    assert_eq!(
        deposit_claims_from_log(&accepted),
        vec![GatewayDepositClaim {
            txid: TXID.to_owned(),
            out_idx: 2,
            // The module issues ecash for the output value less its receive
            // fee, so that is what the federation credited.
            amount: Sats(24_500),
        }]
    );
}

/// A receive the federation rejected credited nothing.
#[test]
fn an_aborted_walletv2_receive_is_not_a_claim() {
    let operation = operation_id(1);
    let entries = vec![
        log_entry(2, &receive_update(operation, ReceivePaymentStatus::Aborted)),
        log_entry(
            1,
            &receive(
                operation,
                Some(OutPoint {
                    txid: txid(),
                    vout: 2,
                }),
            ),
        ),
    ];

    assert_eq!(deposit_claims_from_log(&entries), vec![]);
}

/// An accepted receive that names no outpoint identifies no output, so it
/// cannot attribute a deposit to an item.
#[test]
fn a_walletv2_receive_without_an_outpoint_is_not_a_claim() {
    let operation = operation_id(1);
    let entries = vec![
        log_entry(2, &receive_update(operation, ReceivePaymentStatus::Success)),
        log_entry(1, &receive(operation, None)),
    ];

    assert_eq!(deposit_claims_from_log(&entries), vec![]);
}

/// One gateway serves both module versions, so one read reports the claims of
/// whichever federation it was asked about.
#[test]
fn both_module_versions_reduce_to_the_same_claim_shape() {
    let operation = operation_id(1);
    let entries = vec![
        log_entry(3, &receive_update(operation, ReceivePaymentStatus::Success)),
        log_entry(
            2,
            &receive(
                operation,
                Some(OutPoint {
                    txid: txid(),
                    vout: 1,
                }),
            ),
        ),
        log_entry(
            1,
            &DepositConfirmed {
                txid: txid(),
                out_idx: 0,
                amount: Amount::from_sats(10_000),
            },
        ),
    ];

    let claims = deposit_claims_from_log(&entries);
    assert_eq!(
        claims
            .iter()
            .map(|claim| (claim.out_idx, claim.amount))
            .collect::<Vec<_>>(),
        vec![(1, Sats(24_500)), (0, Sats(10_000))]
    );
}
