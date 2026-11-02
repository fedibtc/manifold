//! Tests for FA-facing Nostr query boundaries.

use std::{
    future,
    sync::Mutex,
    time::{Duration, Instant},
};

use nostr_sdk::{EventBuilder, Keys, PublicKey};
use serde_json::json;

use crate::NostrClientError;

use super::*;

#[test]
fn mg_advertisement_query_pins_kind_identifier_hashtag_and_candidate_bound() {
    for (format, kind, d, t) in [
        (MgAdvertisementFormat::Legacy, 37701, "fman-ad", "fedi-fman"),
        (MgAdvertisementFormat::V2, 37709, "mg-ad", "fedi-mg"),
    ] {
        assert_eq!(
            serde_json::to_value(mg_advertisements_filter(format)).expect("filter serializes"),
            json!({
                "kinds": [kind],
                "#d": [d],
                "#t": [t],
                "limit": MG_ADVERTISEMENTS_CANDIDATE_LIMIT,
            }),
            "the {format:?} enumeration filter pins its own role and no author",
        );
    }
}

#[test]
fn liquidity_provider_query_pins_role_without_pinning_an_author() {
    let filter = liquidity_provider_advertisements_filter();

    assert_eq!(
        serde_json::to_value(filter).expect("filter serializes"),
        json!({
            "kinds": [
                fedi_decentralized_nostr::flip::FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND
            ],
            "#d": [
                fedi_decentralized_nostr::flip::FLIP_PROVIDER_ADVERTISEMENT_D_TAG
            ],
            "#t": [
                fedi_decentralized_nostr::flip::FLIP_PROVIDER_ADVERTISEMENT_HASHTAG
            ],
            "limit": FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT,
        }),
        "provider enumeration must include every author",
    );
}

#[test]
fn setup_payment_query_pins_author_kind_identifier_and_candidate_bound() {
    let publisher =
        PublicKey::parse("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .expect("test publisher parses");
    let filter = setup_payment_federations_filter(publisher);

    assert_eq!(
        serde_json::to_value(filter).expect("filter serializes"),
        json!({
            "authors": [publisher.to_string()],
            "kinds": [
                fedi_decentralized_nostr::setup_payment_federations::
                    SETUP_PAYMENT_FEDERATIONS_EVENT_KIND
            ],
            "#d": [
                fedi_decentralized_nostr::setup_payment_federations::
                    SETUP_PAYMENT_FEDERATIONS_D_TAG
            ],
            "limit": SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT,
        })
    );
}

fn note(content: &str) -> Event {
    EventBuilder::text_note(content)
        .sign_with_keys(&Keys::generate())
        .expect("test event signs")
}

fn contents(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.content.as_str()).collect()
}

fn incomplete() -> NostrClientError {
    NostrClientError::IncompleteQuery {
        reason: "test relay stalled before EOSE",
    }
}

#[tokio::test]
async fn both_formats_are_returned_and_v2_gets_half_the_budget() {
    let budgets = Mutex::new(Vec::new());
    let events = fetch_both_mg_advertisement_formats(Duration::from_secs(8), |format, timeout| {
        budgets.lock().expect("test lock").push((format, timeout));
        async move {
            Ok(match format {
                MgAdvertisementFormat::Legacy => vec![note("legacy")],
                MgAdvertisementFormat::V2 => vec![note("v2")],
            })
        }
    })
    .await
    .expect("both queries succeed");

    assert_eq!(contents(&events), ["legacy", "v2"]);
    assert_eq!(
        *budgets.lock().expect("test lock"),
        [
            (MgAdvertisementFormat::Legacy, Duration::from_secs(8)),
            (MgAdvertisementFormat::V2, Duration::from_secs(4)),
        ],
    );
}

#[tokio::test]
async fn a_failed_v2_query_returns_the_legacy_events_alone() {
    let events =
        fetch_both_mg_advertisement_formats(Duration::from_secs(1), |format, _| async move {
            match format {
                MgAdvertisementFormat::Legacy => Ok(vec![note("legacy")]),
                MgAdvertisementFormat::V2 => Err(incomplete()),
            }
        })
        .await
        .expect("the legacy query decides the result");

    assert_eq!(contents(&events), ["legacy"]);
}

/// A v2 query that ignores its budget is cut at half the timeout, while a
/// legacy query slower than that half still completes.
#[tokio::test]
async fn a_stalled_v2_query_is_cut_at_half_the_timeout_and_legacy_keeps_the_rest() {
    let started = Instant::now();
    let events =
        fetch_both_mg_advertisement_formats(Duration::from_millis(1000), |format, _| async move {
            match format {
                MgAdvertisementFormat::Legacy => {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    Ok(vec![note("legacy")])
                }
                MgAdvertisementFormat::V2 => future::pending().await,
            }
        })
        .await
        .expect("the legacy query decides the result");

    assert_eq!(contents(&events), ["legacy"]);
    let elapsed = started.elapsed();
    // Waiting for the v2 query until the full timeout would end at 1000 ms.
    assert!(
        elapsed < Duration::from_millis(900),
        "the call ends when the legacy query does: {elapsed:?}",
    );
}

#[tokio::test]
async fn a_failed_legacy_query_fails_even_when_v2_succeeds() {
    let result =
        fetch_both_mg_advertisement_formats(Duration::from_secs(1), |format, _| async move {
            match format {
                MgAdvertisementFormat::Legacy => Err(incomplete()),
                MgAdvertisementFormat::V2 => Ok(vec![note("v2")]),
            }
        })
        .await;

    assert!(
        matches!(result, Err(NostrClientError::IncompleteQuery { .. })),
        "{result:?}",
    );
}
