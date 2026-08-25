use super::*;

use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::OutgoingState;
use bitcoin::secp256k1::PublicKey;
use fedimint_lnv2_client::SelectGatewayError;
use lightning_invoice::RoutingFees;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{Layer, Registry};

#[tokio::test]
async fn completed_v1_invoice_is_rejected_before_payment_start() {
    let starts = AtomicUsize::new(0);
    let result = start_fresh_v1_payment(true, || async {
        starts.fetch_add(1, Ordering::SeqCst);
        Ok(OperationId([1; 32]))
    })
    .await;

    assert!(result.is_err());
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[test]
fn returned_v1_operation_must_match_exact_committed_payout() {
    assert!(validate_committed_v1_payout_amount(Ok(1_000), 1_000).is_ok());
    for result in [Err(anyhow::anyhow!("invalid operation binding")), Ok(999)] {
        assert!(
            validate_committed_v1_payout_amount(result, 1_000).is_err(),
            "returned operation validation must fail closed"
        );
    }
}

#[test]
fn v1_gateway_fee_validation_rejects_pinned_client_panics_and_overflow() {
    let fees = |base_msat, proportional_millionths| RoutingFees {
        base_msat,
        proportional_millionths,
    };

    validate_v1_gateway_fees(Amount::ZERO, &fees(u32::MAX, 1_000_000)).unwrap();
    validate_v1_gateway_fees(Amount::from_msats(1), &fees(0, 1_000_000)).unwrap();
    assert!(
        validate_v1_gateway_fees(Amount::from_msats(1), &fees(0, 1_000_001)).is_err(),
        "a rate above one million makes the pinned client's divisor zero"
    );

    validate_v1_gateway_fees(Amount::from_msats(u64::MAX), &fees(0, 0)).unwrap();
    validate_v1_gateway_fees(Amount::from_msats(u64::MAX / 2), &fees(1, 1_000_000)).unwrap();
    assert!(
        validate_v1_gateway_fees(Amount::from_msats(u64::MAX), &fees(1, 1_000_000)).is_err(),
        "base and proportional fee addition must not overflow"
    );
    assert!(
        validate_v1_gateway_fees(Amount::from_msats(u64::MAX), &fees(1, 0)).is_err(),
        "base-fee addition must not overflow"
    );
    assert!(
        validate_v1_gateway_fees(Amount::from_msats(u64::MAX), &fees(0, 1_000_000)).is_err(),
        "proportional-fee and contract additions must not overflow"
    );
}

#[test]
fn payout_diagnostics_emit_only_closed_gateway_and_rail_context() {
    let gateway_public_key =
        PublicKey::from_str("0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .unwrap();
    let operation_id = OperationId([3; 32]);
    let events = capture_events(|| {
        emit_v2_fallback("federation", V2Fallback::ModuleAbsentOrUnsupported);
        emit_v2_fallback(
            "federation",
            v2_fallback_reason(&SelectGatewayError::NoGatewaysAvailable),
        );
        emit_v2_fallback(
            "federation",
            v2_fallback_reason(&SelectGatewayError::GatewaysUnresponsive),
        );
        emit_v2_fallback(
            "federation",
            v2_fallback_reason(&SelectGatewayError::FailedToRequestGateways(
                "credential-bearing gateway error".to_owned(),
            )),
        );
        emit_payout_start(
            "federation",
            OutgoingRail::Lnv2,
            &gateway_public_key,
            operation_id,
            1_000,
            15,
        );
        emit_payout_start(
            "federation",
            OutgoingRail::Lnv1,
            &gateway_public_key,
            operation_id,
            1_000,
            15,
        );
        emit_payout_rail(
            "federation",
            &OutgoingOperation::new(
                PayoutOperationId::parse(&"03".repeat(32)).unwrap(),
                OutgoingRail::Lnv1,
                OutgoingState::Succeeded,
                1_000,
                1_015,
                false,
            ),
        );
    });

    assert_eq!(
        events.iter().map(|event| event.level).collect::<Vec<_>>(),
        vec![
            Level::INFO,
            Level::INFO,
            Level::WARN,
            Level::WARN,
            Level::INFO,
            Level::INFO,
            Level::INFO,
        ]
    );

    for event in &events[..4] {
        assert_fields(
            event,
            &[
                "fallback_rail",
                "federation_id",
                "gateway_selection",
                "message",
                "rail",
            ],
        );
        assert_field(event, "federation_id", "federation");
        assert_field(event, "rail", "lnv2");
        assert_field(event, "fallback_rail", "lnv1");
    }
    assert_field(
        &events[0],
        "gateway_selection",
        "module_absent_or_unsupported",
    );
    assert_field(&events[1], "gateway_selection", "no_announcements");
    assert_field(
        &events[2],
        "gateway_selection",
        "no_responsive_supported_gateway",
    );
    assert_field(&events[3], "gateway_selection", "announcement_query_failed");

    for (event, rail) in [(&events[4], "lnv2"), (&events[5], "lnv1")] {
        assert_fields(
            event,
            &[
                "federation_id",
                "gateway_public_key",
                "gateway_fee_quote_msat",
                "message",
                "operation_id",
                "rail",
                "recipient_amount_msat",
            ],
        );
        assert_field(event, "federation_id", "federation");
        assert_field(event, "rail", rail);
        assert_field(event, "gateway_public_key", &gateway_public_key.to_string());
        assert_field(event, "operation_id", &operation_id.fmt_full().to_string());
        assert_field(event, "recipient_amount_msat", "1000");
        assert_field(event, "gateway_fee_quote_msat", "15");
    }

    assert_fields(
        &events[6],
        &[
            "contract_amount_msat",
            "encumbered_msat",
            "federation_id",
            "has_active_state_machines",
            "known_contract_fee_effect_msat",
            "message",
            "operation_id",
            "rail",
            "rail_state",
            "recipient_amount_msat",
        ],
    );
    assert_field(&events[6], "federation_id", "federation");
    assert_field(&events[6], "rail", "lnv1");
    assert_field(
        &events[6],
        "operation_id",
        &operation_id.fmt_full().to_string(),
    );
    assert_field(&events[6], "rail_state", "Succeeded");
    assert_field(&events[6], "has_active_state_machines", "false");
    assert_field(&events[6], "recipient_amount_msat", "1000");
    assert_field(&events[6], "contract_amount_msat", "1015");
    assert_field(&events[6], "known_contract_fee_effect_msat", "15");

    assert!(
        !format!("{events:?}").contains("credential-bearing gateway error"),
        "diagnostics must not leak arbitrary upstream error text"
    );
}

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct EventCapture(Arc<Mutex<Vec<CapturedEvent>>>);

impl<S> Layer<S> for EventCapture
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCapture(&mut fields));
        self.0.lock().unwrap().push(CapturedEvent {
            level: *event.metadata().level(),
            fields,
        });
    }
}

struct FieldCapture<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCapture<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }
}

fn capture_events(action: impl FnOnce()) -> Vec<CapturedEvent> {
    let events = EventCapture::default();
    let subscriber = Registry::default().with(events.clone());
    tracing::subscriber::with_default(subscriber, action);
    events.0.lock().unwrap().clone()
}

fn assert_fields(event: &CapturedEvent, expected: &[&str]) {
    assert_eq!(
        event
            .fields
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>(),
        "{event:?}"
    );
}

fn assert_field(event: &CapturedEvent, name: &str, expected: &str) {
    assert_eq!(
        event.fields.get(name).map(String::as_str),
        Some(expected),
        "{event:?}"
    );
}
