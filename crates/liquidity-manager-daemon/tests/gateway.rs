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

/// Reduces one self-contained log, as a reader that starts at the newest
/// entry and never needs a second page would.
fn deposit_claims_from_log(entries: &[PersistedLogEntry]) -> Vec<GatewayDepositClaim> {
    DepositClaimReader::default().read_page(entries)
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

/// The walk recedes one position past the oldest entry it has read, and stops
/// only when a page reaches the start of the log.
#[test]
fn a_page_ends_the_walk_only_at_the_start_of_the_log() {
    let success = receive_update(operation_id(1), ReceivePaymentStatus::Success);

    assert_eq!(
        page_before(&[]),
        None,
        "an empty page has nothing behind it"
    );
    assert_eq!(
        page_before(&[log_entry(5, &success), log_entry(4, &success)]),
        Some(EventLogId::LOG_START.saturating_add(3)),
        "the next read ends just before the oldest entry of this page"
    );
    assert_eq!(
        page_before(&[log_entry(0, &success)]),
        None,
        "a page holding the first log position has nothing behind it"
    );
}

/// A walletv2 claim can straddle a page boundary.
///
/// The federation's acceptance is logged after the receive it accepts, so a
/// backwards walk reads the update on one page and the receive on the next.
/// The reader carries the accepted operation ids forward, so the older page
/// still resolves into a claim.
#[test]
fn a_receive_resolves_against_an_acceptance_read_on_an_earlier_page() {
    let operation = operation_id(1);
    let outpoint = OutPoint {
        txid: txid(),
        vout: 2,
    };
    let newer_page = vec![log_entry(
        2,
        &receive_update(operation, ReceivePaymentStatus::Success),
    )];
    let older_page = vec![log_entry(1, &receive(operation, Some(outpoint)))];

    assert_eq!(
        deposit_claims_from_log(&older_page),
        vec![],
        "read on its own the receive has no acceptance to resolve against"
    );

    let mut reader = DepositClaimReader::default();
    assert_eq!(reader.read_page(&newer_page), vec![]);
    assert_eq!(
        reader.read_page(&older_page),
        vec![GatewayDepositClaim {
            txid: TXID.to_owned(),
            out_idx: 2,
            amount: Sats(24_500),
        }]
    );
}

/// One transaction can pay two items' deposit addresses, so the output index
/// is what tells the two apart.
#[test]
fn a_query_matches_the_one_output_its_item_funded() {
    let claim = GatewayDepositClaim {
        txid: TXID.to_owned(),
        out_idx: 1,
        amount: Sats(25_000),
    };

    assert!(
        DepositClaimQuery {
            txid: TXID,
            out_idx: Some(1),
            min_amount: Sats(25_000),
        }
        .matches(&claim)
    );
    assert!(
        !DepositClaimQuery {
            txid: TXID,
            out_idx: Some(0),
            min_amount: Sats(25_000),
        }
        .matches(&claim),
        "a sibling output of the same transaction funded a different item"
    );
    assert!(
        DepositClaimQuery {
            txid: TXID,
            out_idx: None,
            min_amount: Sats(25_000),
        }
        .matches(&claim),
        "with no verified output index the txid is the whole attribution"
    );
    assert!(
        !DepositClaimQuery {
            txid: TXID,
            out_idx: Some(1),
            min_amount: Sats(25_001),
        }
        .matches(&claim),
        "a credit short of the committed amount does not cover the item"
    );
}
