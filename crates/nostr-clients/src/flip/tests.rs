//! Tests for FLIP-facing Nostr client filters.

use std::{num::NonZeroU16, time::Duration};

use nostr_sdk::PublicKey;
use serde_json::json;

use super::*;

#[test]
fn effective_candidate_limit_defaults_and_clamps() {
    assert_eq!(
        effective_candidate_limit(None),
        MG_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT
    );
    assert_eq!(effective_candidate_limit(NonZeroU16::new(2)), 2);
    assert_eq!(
        effective_candidate_limit(NonZeroU16::new(MG_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX + 1)),
        MG_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX
    );
}

#[test]
fn mg_trust_material_filter_targets_mg_ad_candidates() {
    let mg_pubkey =
        PublicKey::parse("3b6a27bcceb6a42d62a3a8d02a6f0d73629f8429508871f4aee44d7fc3fc9d0d")
            .expect("test pubkey parses");
    let filter = mg_trust_material_filter(FetchMgTrustMaterialRequest {
        mg_pubkey,
        candidate_limit: None,
        timeout: Duration::from_secs(1),
    });
    let value = serde_json::to_value(filter).expect("filter serializes");

    assert_eq!(
        value,
        json!({
            "authors": [mg_pubkey.to_string()],
            "kinds": [fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_EVENT_KIND],
            "#d": [fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_D_TAG],
            "#t": [fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_HASHTAG],
            "limit": MG_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT,
        })
    );
}
