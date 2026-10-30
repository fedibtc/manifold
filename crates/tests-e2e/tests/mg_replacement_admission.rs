//! Concrete request-bound replacement admission with fresh issuer revocation.

use std::time::Duration;

use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_decentralized_domain::HolderAuthorizationEnvelope;
use fedi_decentralized_manifold_environment::{DEV_NOSTR_RELAYS_ENV, ManifoldEnvironment};
use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG, credential_revocation_d_tag,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use mg_core::replacement_guardian::ReplacementGuardianVerifier;
use mg_nostr::replacement_guardian::NostrReplacementGuardianVerifier;
use nostr_sdk::{EventBuilder, Keys, Kind, PublicKey, Tag};
use peerbadge_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, PendingIssuance, SubjectPubkey,
};
use serde_json::json;

#[path = "mg_replacement_admission/direct.rs"]
mod direct;

#[path = "mg_replacement_admission/relay_probe.rs"]
mod relay_probe;

fn issue(
    issuer: &IssuerContext,
    holder: &HolderContext,
    destination: PublicKey,
    trust_level: u64,
) -> HolderAuthorizationEnvelope {
    let authority = issuer.issuer_authority(Vec::new()).unwrap();
    let info = json!({"schema": "fedi-trust-score-v1.0", "trust_level": trust_level});
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        json!(holder.public_key().to_string()),
    )
    .unwrap();
    let response = issuer.issue_credential(info, &request).unwrap();
    let signed_credential = pending
        .finalize(&authority.issuer.issuance_key, &response)
        .unwrap();
    let holder_authorization = holder
        .authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: SubjectPubkey(destination),
            },
            &signed_credential,
        )
        .unwrap();
    HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential,
    }
}
