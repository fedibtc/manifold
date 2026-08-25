//! Tests for FI-facing Nostr query boundaries.

use nostr_sdk::PublicKey;
use serde_json::json;

use super::*;

#[test]
fn fman_advertisement_query_pins_kind_identifier_hashtag_and_candidate_bound() {
    let filter = fman_advertisements_filter();

    assert_eq!(
        serde_json::to_value(filter).expect("filter serializes"),
        json!({
            "kinds": [fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_EVENT_KIND],
            "#d": [fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_D_TAG],
            "#t": [fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_HASHTAG],
            "limit": FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT,
        }),
        "the enumeration filter must pin no author",
    );
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
