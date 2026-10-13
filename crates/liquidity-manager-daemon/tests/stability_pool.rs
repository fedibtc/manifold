/// The config-revision fence is exactly this: the hash the client actually
/// carries must equal the hash the request was accepted against.
///
/// A regression that normalised either side — trimming, case folding, prefix
/// matching — would keep every other test green while letting a client with a
/// different module map mint the address the provider funds.
#[test]
fn the_config_hash_fence_is_an_exact_comparison() {
    let hash = "a3f1".repeat(16);

    assert!(
        matches!(config_hash_check(&hash, &hash), TargetCheck::Usable),
        "the accepted hash is usable"
    );

    // Different value.
    let other = "b4e2".repeat(16);
    let TargetCheck::Unusable(reason) = config_hash_check(&other, &hash) else {
        panic!("a different config hash must be refused");
    };
    assert!(
        reason.contains(&other) && reason.contains(&hash),
        "the refusal names both hashes so an operator can tell them apart: {reason}"
    );

    // Same bytes, different case. `hex::encode` emits lowercase, so an
    // uppercase observation means something re-encoded it, and the fence
    // must not accept it silently.
    assert!(
        matches!(
            config_hash_check(&hash.to_uppercase(), &hash),
            TargetCheck::Unusable(_)
        ),
        "case folding would weaken the fence"
    );

    // Whitespace and prefixes, the two other normalisations that would look
    // harmless.
    assert!(
        matches!(
            config_hash_check(&format!(" {hash} "), &hash),
            TargetCheck::Unusable(_)
        ),
        "trimming would weaken the fence"
    );
    assert!(
        matches!(
            config_hash_check(&hash[..hash.len() - 2], &hash),
            TargetCheck::Unusable(_)
        ),
        "a prefix is not a match"
    );
}
use fedimint_core::core::ModuleKind;

use super::*;

const TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn address(text: &str) -> Address<NetworkUnchecked> {
    text.parse().expect("valid test address")
}

fn our_address() -> Address<NetworkUnchecked> {
    address("bcrt1q0xcqpzrky6eff2g52qdye53xkk9jxkvrl4xfg5")
}

fn operation_id(byte: u8) -> OperationId {
    OperationId([byte; 32])
}

/// Builds the entry the target client persists for one event logged by the
/// module of kind `module`.
fn log_entry<E: Event>(id: u64, module: Option<ModuleKind>, event: &E) -> PersistedLogEntry {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "kind": E::KIND,
        "module": module.map(|kind| serde_json::json!({ "kind": kind, "id": 1 })),
        "ts_usecs": 0,
        "payload": serde_json::to_value(event).expect("event serializes"),
    }))
    .expect("entry deserializes")
}

fn receive(id: u64, operation: u8, to: Address<NetworkUnchecked>) -> PersistedLogEntry {
    log_entry(
        id,
        ReceivePaymentEvent::MODULE,
        &ReceivePaymentEvent {
            operation_id: operation_id(operation),
            value: bitcoin::Amount::from_sat(25_100),
            fee: bitcoin::Amount::from_sat(600),
            address: to,
            outpoint: Some(bitcoin::OutPoint {
                txid: TXID.parse().expect("valid test txid"),
                vout: 0,
            }),
        },
    )
}

fn receive_update(id: u64, operation: u8, status: ReceivePaymentStatus) -> PersistedLogEntry {
    log_entry(
        id,
        ReceivePaymentUpdateEvent::MODULE,
        &ReceivePaymentUpdateEvent {
            operation_id: operation_id(operation),
            status,
        },
    )
}

/// The client's other addresses are not the item's, whatever their claims
/// say.
#[test]
fn a_claim_into_another_address_is_not_the_peg_in() {
    let other = address("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080");
    let entries = vec![
        receive(1, 1, other),
        receive_update(2, 1, ReceivePaymentStatus::Success),
    ];

    assert_eq!(peg_in_receive(&entries, &our_address()), None);
}

/// A submitted claim is not a peg-in until the federation accepts it, and
/// what it credits is the output less the receive fee.
#[test]
fn a_submitted_claim_is_pending_at_its_credited_amount() {
    let entries = vec![receive(1, 1, our_address())];

    assert_eq!(
        peg_in_receive(&entries, &our_address()),
        Some(PegInReceive::Pending {
            amount: Sats(24_500)
        })
    );
}

/// The scanner resubmits a rejected claim as a new receive, so a rejection
/// is not final and the accepted resubmission is the one that counts.
#[test]
fn the_accepted_resubmission_of_a_rejected_claim_is_the_peg_in() {
    let entries = vec![
        receive(1, 1, our_address()),
        receive_update(2, 1, ReceivePaymentStatus::Aborted),
        receive(3, 2, our_address()),
        receive_update(4, 2, ReceivePaymentStatus::Success),
    ];

    assert_eq!(
        peg_in_receive(&entries, &our_address()),
        Some(PegInReceive::Accepted {
            operation_id: operation_id(2),
            amount: Sats(24_500),
        })
    );
}

/// Event kinds are only unique within a module, so an acceptance another
/// module logs under the same kind must not settle the walletv2 claim.
#[test]
fn an_acceptance_logged_by_another_module_is_not_the_walletv2_one() {
    let entries = vec![
        receive(1, 1, our_address()),
        log_entry(
            2,
            Some(ModuleKind::from_static_str("lnv2")),
            &ReceivePaymentUpdateEvent {
                operation_id: operation_id(1),
                status: ReceivePaymentStatus::Success,
            },
        ),
    ];

    assert_eq!(
        peg_in_receive(&entries, &our_address()),
        Some(PegInReceive::Pending {
            amount: Sats(24_500)
        })
    );
}

/// The gate that keeps a stability allocation off a federation without the
/// module reads a spelled-out kind, so a silent upstream rename would turn
/// it into a check that always fails and rejects every target.
#[test]
fn the_required_module_kind_is_the_one_upstream_registers() {
    assert_eq!(
        STABILITY_POOL_MODULE_KIND,
        stability_pool_client::common::KIND.as_str()
    );
}
