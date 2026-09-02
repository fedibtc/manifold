//! Concrete relay composition for `SPEC-peer-badge-verifier` and
//! `DESIGN-fman-selection`: issuer authority → holder authorization → FMan
//! advertisement → FI lazy verified selection.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_credential_sdk_protocol::{
    HolderAuthorization, HolderAuthorizationRequest, HolderContext, IssuerContext,
    IssuerSecretKeys, PendingIssuance, RevocationLocation, SignedCredential, SubjectPubkey,
    VerificationContext,
};
use fedi_decentralized_domain::HolderAuthorizationEnvelope;
use fedi_decentralized_nostr::attester::{
    ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG,
};
use fedi_decentralized_nostr::fman::{
    AdvertisementPayload, ApiEndpoint, Availability, FMAN_ADVERTISEMENT_D_TAG,
    FMAN_ADVERTISEMENT_EVENT_KIND, FMAN_ADVERTISEMENT_HASHTAG, FMAN_ADVERTISEMENT_SIGNATURE_DOMAIN,
    FMAN_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, IROH_API_ENDPOINT_TRANSPORT,
    IROH_API_ENDPOINT_URL_SCHEME, fman_authorization_d_tag, sign_advertisement,
};
use fedi_decentralized_nostr_clients::{
    FiNostrClient, HolderNostrClient, NostrFiClient, NostrHolderClient, NostrRelayClient,
    PublishFmanAuthorizationRequest,
};
use fedi_decentralized_peer_badge_verifier::{PeerBadgeVerifier, PeerBadgeVerifierProvenance};
use fedi_decentralized_service_fleet_manager::{FederationSize, FedimintdVersion, Plan};
use fedi_iroh_rpc::iroh::SecretKey as IrohSecretKey;
use fi_client::{
    FedimintdVersionRange, FmanCandidateRequirements, FmanDiscoveryOptions, FmanRegistryQuery,
    FmanSelectionRequest, PlanPreference,
};
use nostr_sdk::{
    Event, EventBuilder, Filter, Keys as NostrKeys, Kind, PublicKey as NostrPublicKey,
    SecretKey as NostrSecretKey, Tag,
    secp256k1::{Message, Secp256k1, SecretKey, schnorr::Signature},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TRUST_BADGE_SCHEMA: &str = "fedi-trust-score-v1.0";
const TRUSTED_PEER_BADGE_LEVEL: u64 = 9;

#[tokio::test]
async fn holder_trust_badge_to_concrete_fi_selection_flow() {
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe from env");
    let lease = client
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .expect("allocate Nostr relay");
    let ResourceDescriptor::NostrRelay(relay) = &lease.descriptor else {
        panic!(
            "expected Nostr relay descriptor, got {:?}",
            lease.descriptor
        );
    };
    // Issue a holder trust badge from a trusted issuer.

    let issuer = IssuerContext::import_secret_key(&test_issuer_secret_keys())
        .expect("import test credential issuer");
    let issuer_authority = issuer
        .issuer_authority(vec![RevocationLocation {
            protocol: "nostr".to_owned(),
            location: relay.url.clone(),
        }])
        .expect("build issuer authority");
    let issuer_metadata = issuer_authority
        .verify()
        .expect("issuer authority verifies");
    let issuer_pubkey = issuer_metadata.issuer_id_pubkey.0.to_string();

    let issuer_nostr_keys = NostrKeys::parse(
        &issuer
            .export_secret_key()
            .expect("export test issuer")
            .issuer_id_secret_key,
    )
    .expect("issuer identity secret parses as Nostr keys");
    let issuer_relay =
        NostrRelayClient::connect(&relay.url, issuer_nostr_keys, Duration::from_secs(5))
            .await
            .expect("connect issuer authority publisher");
    issuer_relay
        .publish_event(
            EventBuilder::new(
                Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
                serde_json::to_string(&issuer_authority).expect("serialize issuer authority"),
            )
            .tags([
                Tag::identifier(ISSUER_AUTHORITY_D_TAG),
                Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
            ]),
        )
        .await
        .expect("publish current issuer authority");

    let holder = HolderContext::generate();
    let holder_pubkey = holder.public_key().to_string();
    let holder_nostr_keys = NostrKeys::new(
        NostrSecretKey::parse(&holder.export_secret_key()).expect("holder secret key parses"),
    );
    let holder_nostr_pubkey = holder_nostr_keys.public_key();
    assert_eq!(holder_nostr_pubkey.to_string(), holder_pubkey);

    let badge_info = json!({
        "schema": TRUST_BADGE_SCHEMA,
        "trust_level": TRUSTED_PEER_BADGE_LEVEL,
    });
    let hidden_holder_claim = json!(holder_pubkey);
    let (issuance_request, pending_issuance) = PendingIssuance::create_request(
        &issuer_metadata.issuance_key,
        issuer_metadata.issuer_id_pubkey.clone(),
        badge_info,
        hidden_holder_claim,
    )
    .expect("holder creates blind issuance request");
    let issuance_response = issuer
        .issue_credential(pending_issuance.info.clone(), &issuance_request)
        .expect("issuer signs trust badge");
    let trust_badge = pending_issuance
        .finalize(&issuer_metadata.issuance_key, &issuance_response)
        .expect("holder finalizes trust badge");

    let mut verifier = VerificationContext::new();
    verifier
        .add_issuer_authority(&issuer_authority)
        .expect("verifier trusts issuer authority");
    verifier
        .verify_credential(&trust_badge)
        .expect("trust badge verifies against issuer");

    // Holder authorizes the FMan key to present the trust badge.
    let holder_nostr = NostrHolderClient::new(
        NostrRelayClient::connect(&relay.url, holder_nostr_keys, Duration::from_secs(5))
            .await
            .expect("connect holder Nostr client"),
    );
    let fman_nostr_keys = NostrKeys::generate();
    let fman_pubkey = fman_nostr_keys.public_key();
    let fman_pubkey_string = fman_pubkey.to_string();

    let holder_authorization = holder
        .authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: fman_pubkey_string
                    .parse::<SubjectPubkey>()
                    .expect("FMan pubkey parses as an SDK subject pubkey"),
            },
            &trust_badge,
        )
        .expect("holder signs authorization for FMan pubkey");
    verifier
        .verify_credential_authorization(&trust_badge, &holder_authorization)
        .expect("holder authorization verifies against the trust badge");

    let credential_digest =
        serde_json::to_value(&holder_authorization.authorization.credential_digest)
            .expect("serialize credential digest")
            .as_str()
            .expect("credential digest serializes as string")
            .to_owned();
    let authorization_d_tag = fman_authorization_d_tag(&fman_pubkey_string, &credential_digest);
    let authorization_envelope = json!({
        "version": 1,
        "holder_id_pubkey": holder_pubkey,
        "holder_authorization": holder_authorization,
        "signed_credential": trust_badge,
    });
    let authorization_event_id = holder_nostr
        .publish_fman_authorization(PublishFmanAuthorizationRequest {
            fman_pubkey,
            issuer_pubkey: issuer_pubkey.clone(),
            credential_digest: credential_digest.clone(),
            schema: TRUST_BADGE_SCHEMA.to_owned(),
            content: serde_json_canonicalizer::to_string(&authorization_envelope)
                .expect("canonicalize holder authorization envelope"),
        })
        .await
        .expect("publish holder authorization event");
    let invalid_authorization_event_id = holder_nostr
        .publish_fman_authorization(PublishFmanAuthorizationRequest {
            fman_pubkey,
            issuer_pubkey: issuer_pubkey.clone(),
            credential_digest: "invalid-digest".to_owned(),
            schema: TRUST_BADGE_SCHEMA.to_owned(),
            content: serde_json_canonicalizer::to_string(&json!({
                "version": 1,
                "holder_id_pubkey": holder_pubkey,
                "holder_authorization": "invalid",
                "signed_credential": "invalid",
            }))
            .expect("canonicalize invalid holder authorization envelope"),
        })
        .await
        .expect("publish invalid holder authorization event");

    // FMan discovers and verifies the holder-published authorization event.
    let fman_relay =
        NostrRelayClient::connect(&relay.url, fman_nostr_keys.clone(), Duration::from_secs(5))
            .await
            .expect("connect FMan Nostr client");
    let fi_nostr = NostrFiClient::new(
        NostrRelayClient::connect(&relay.url, NostrKeys::generate(), Duration::from_secs(5))
            .await
            .expect("connect FI Nostr client"),
    );
    let authorization_filter = Filter::new()
        .kind(Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND))
        .pubkey(fman_pubkey)
        .hashtag(FMAN_AUTHORIZATION_HASHTAG)
        .limit(10);
    let fetched_authorization_events = fman_relay
        .fetch_events_capped(authorization_filter, Duration::from_secs(10), 10)
        .await
        .expect("fetch holder authorization candidates for FMan");
    assert!(
        fetched_authorization_events
            .iter()
            .any(|event| event.id == invalid_authorization_event_id),
        "FMan sees the newer invalid matching authorization candidate"
    );
    let fetched_authorization_event = fetched_authorization_events
        .iter()
        .find(|event| event.id == authorization_event_id)
        .expect("FMan can still find valid authorization among candidates");
    fetched_authorization_event
        .verify()
        .expect("fetched holder authorization event signature verifies");
    assert_eq!(
        fetched_authorization_event.id, authorization_event_id,
        "FMan discovers the expected authorization event by kind and p/t tag indexes"
    );
    assert_eq!(
        fetched_authorization_event.pubkey, holder_nostr_pubkey,
        "holder authorization event is authored by the holder"
    );
    assert_eq!(
        tag_value(fetched_authorization_event, "d").as_deref(),
        Some(authorization_d_tag.as_str()),
        "holder authorization uses the documented d tag"
    );
    assert_eq!(
        tag_value(fetched_authorization_event, "p").as_deref(),
        Some(fman_pubkey_string.as_str()),
        "holder authorization p tag targets the FMan pubkey"
    );
    assert_eq!(
        tag_value(fetched_authorization_event, "issuer").as_deref(),
        Some(issuer_pubkey.as_str()),
        "holder authorization tags the issuer authority pubkey"
    );
    assert_eq!(
        tag_value(fetched_authorization_event, "credential").as_deref(),
        Some(credential_digest.as_str()),
        "holder authorization tags the credential digest"
    );
    assert_eq!(
        tag_value(fetched_authorization_event, "schema").as_deref(),
        Some(TRUST_BADGE_SCHEMA),
        "holder authorization tags the trust badge schema"
    );
    let fetched_authorization_envelope: Value =
        serde_json::from_str(&fetched_authorization_event.content)
            .expect("fetched authorization envelope is JSON");
    assert_eq!(
        fetched_authorization_envelope
            .get("holder_id_pubkey")
            .and_then(Value::as_str),
        Some(holder_pubkey.as_str()),
        "authorization envelope holder id matches event author"
    );
    let fetched_trust_badge: SignedCredential = serde_json::from_value(
        fetched_authorization_envelope
            .get("signed_credential")
            .expect("authorization envelope has inline trust badge")
            .clone(),
    )
    .expect("inline trust badge is an SDK credential");
    let fetched_holder_authorization: HolderAuthorization = serde_json::from_value(
        fetched_authorization_envelope
            .get("holder_authorization")
            .expect("authorization envelope has inline holder authorization")
            .clone(),
    )
    .expect("inline holder authorization is an SDK authorization");

    let mut fman_verifier = VerificationContext::new();
    fman_verifier
        .add_issuer_authority(&issuer_authority)
        .expect("FMan trusts issuer authority");
    fman_verifier
        .verify_credential_authorization(&fetched_trust_badge, &fetched_holder_authorization)
        .expect("FMan verifies fetched badge and holder authorization");
    assert_eq!(
        fetched_holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string(),
        fman_pubkey_string,
        "fetched holder authorization is for this FMan pubkey"
    );
    assert_eq!(
        fetched_holder_authorization
            .authorization
            .holder_id_pubkey
            .0
            .to_string(),
        holder_pubkey,
        "fetched holder authorization is signed by the event author"
    );

    // FMan advertises itself with the discovered authorization embedded inline.
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_secs();
    let expires_at = issued_at + 86_400;
    let first_service_key = SecretKey::from_slice(&[42; 32]).expect("test key is valid");
    let first_service_pubkey = first_service_key
        .x_only_public_key(&Secp256k1::new())
        .0
        .to_string();
    let first_endpoint_id = IrohSecretKey::from_bytes(&[43; 32]).public();
    let advertisement_payload = AdvertisementPayload {
        version: fedi_credential_sdk_protocol::ProtocolV1,
        fman_id_pubkey: fman_pubkey_string.clone(),
        service_pubkey: first_service_pubkey,
        issued_at,
        expires_at,
        api_endpoints: vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}{first_endpoint_id}"),
        }],
        availability: Availability {
            fedimintd_version: "0.8.0+fedi".parse().expect("version parses"),
            federation_sizes: vec![7, 10, 13],
        },
        plans: vec![Plan::InfiniteBestEffort {
            price_msats: 10_000_000,
        }],
        holder_authorizations: vec![HolderAuthorizationEnvelope {
            holder_authorization: fetched_holder_authorization.clone(),
            signed_credential: fetched_trust_badge.clone(),
        }],
    };
    let advertisement_document =
        sign_advertisement(advertisement_payload.clone(), &fman_nostr_keys)
            .expect("sign typed FMan advertisement");
    let advertisement_event_id = fman_relay
        .publish_event(
            EventBuilder::new(
                Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND),
                serde_json_canonicalizer::to_string(&advertisement_document)
                    .expect("canonicalize FMan advertisement document"),
            )
            .tags([
                Tag::parse(["d", FMAN_ADVERTISEMENT_D_TAG]).expect("valid d tag"),
                Tag::parse(["t", FMAN_ADVERTISEMENT_HASHTAG]).expect("valid t tag"),
            ]),
        )
        .await
        .expect("publish FMan advertisement event");

    // FI discovers and verifies the FMan advertisement through registry indexes.
    let fetched_advertisement_event = fi_nostr
        .fetch_fman_advertisement(fman_pubkey, Duration::from_secs(10))
        .await
        .expect("fetch FMan advertisement");
    fetched_advertisement_event
        .verify()
        .expect("fetched FMan advertisement event signature verifies");
    assert_eq!(
        fetched_advertisement_event.id, advertisement_event_id,
        "FI discovers the expected FMan advertisement by kind, author, d tag, and t tag indexes"
    );
    assert_eq!(
        fetched_advertisement_event.pubkey, fman_pubkey,
        "FMan advertisement is authored by the FMan key"
    );
    let fetched_advertisement: Value = serde_json::from_str(&fetched_advertisement_event.content)
        .expect("fetched FMan advertisement content is JSON");
    assert_eq!(
        fetched_advertisement
            .pointer("/payload/fman_id_pubkey")
            .and_then(Value::as_str),
        Some(fman_pubkey_string.as_str()),
        "advertisement payload names the FMan pubkey"
    );
    verify_fman_ad_payload(
        fman_pubkey,
        fetched_advertisement
            .get("payload")
            .expect("advertisement has payload"),
        fetched_advertisement
            .pointer("/proof/signature")
            .and_then(Value::as_str)
            .expect("advertisement has proof signature"),
    );
    let embedded = fetched_advertisement
        .pointer("/payload/holder_authorizations/0")
        .expect("advertisement embeds discovered holder authorization");
    let embedded_trust_badge: SignedCredential = serde_json::from_value(
        embedded
            .get("signed_credential")
            .expect("embedded authorization has trust badge")
            .clone(),
    )
    .expect("embedded trust badge is an SDK credential");
    let embedded_holder_authorization: HolderAuthorization = serde_json::from_value(
        embedded
            .get("holder_authorization")
            .expect("embedded authorization has holder authorization")
            .clone(),
    )
    .expect("embedded holder authorization is an SDK authorization");
    fman_verifier
        .verify_credential_authorization(&embedded_trust_badge, &embedded_holder_authorization)
        .expect("FI/FMan verifies embedded badge and holder authorization");
    assert_eq!(
        embedded_holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string(),
        fman_pubkey_string,
        "embedded holder authorization subject matches the advertisement FMan pubkey"
    );
    assert_eq!(
        serde_json::to_value(&embedded_holder_authorization)
            .expect("embedded holder authorization serializes"),
        serde_json::to_value(&fetched_holder_authorization)
            .expect("fetched holder authorization serializes"),
        "advertisement embeds the same holder authorization discovered from the holder event"
    );
    assert_eq!(
        serde_json::to_value(&embedded_trust_badge).expect("embedded trust badge serializes"),
        serde_json::to_value(&fetched_trust_badge).expect("fetched trust badge serializes"),
        "advertisement embeds the same trust badge discovered from the holder event"
    );
    assert_eq!(
        embedded_trust_badge
            .credential
            .info
            .get("schema")
            .and_then(Value::as_str),
        Some(TRUST_BADGE_SCHEMA),
        "embedded trust badge uses the documented schema"
    );
    assert_eq!(
        embedded_trust_badge
            .credential
            .info
            .get("trust_level")
            .and_then(Value::as_u64),
        Some(TRUSTED_PEER_BADGE_LEVEL),
        "embedded trust badge carries the expected trust level"
    );

    // Publish six more current, dialable advertisements so the public FI
    // selection request can fill the minimum seven-seat product federation.
    for index in 1_u8..7 {
        let fman_keys = NostrKeys::generate();
        let fman_pubkey = fman_keys.public_key();
        let authorization = holder
            .authorize_credential_use(
                HolderAuthorizationRequest {
                    subject_pubkey: SubjectPubkey(fman_pubkey),
                },
                &trust_badge,
            )
            .expect("holder authorizes another FMan");
        let endpoint_id = IrohSecretKey::from_bytes(&[50 + index; 32]).public();
        let service_key = SecretKey::from_slice(&[70 + index; 32]).expect("test key is valid");
        let payload = AdvertisementPayload {
            version: fedi_credential_sdk_protocol::ProtocolV1,
            fman_id_pubkey: fman_pubkey.to_string(),
            service_pubkey: service_key
                .x_only_public_key(&Secp256k1::new())
                .0
                .to_string(),
            issued_at,
            expires_at,
            api_endpoints: vec![ApiEndpoint {
                transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
                url: format!("{IROH_API_ENDPOINT_URL_SCHEME}{endpoint_id}"),
            }],
            availability: Availability {
                fedimintd_version: "0.8.0+fedi".parse().expect("version parses"),
                federation_sizes: vec![7, 10, 13],
            },
            plans: vec![Plan::InfiniteBestEffort {
                price_msats: 10_000_000,
            }],
            holder_authorizations: vec![HolderAuthorizationEnvelope {
                holder_authorization: authorization,
                signed_credential: trust_badge.clone(),
            }],
        };
        let document = sign_advertisement(payload, &fman_keys).expect("sign typed advertisement");
        NostrRelayClient::connect(&relay.url, fman_keys, Duration::from_secs(5))
            .await
            .expect("connect additional FMan publisher")
            .publish_event(
                EventBuilder::new(
                    Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND),
                    serde_json::to_string(&document).expect("serialize typed advertisement"),
                )
                .tags([
                    Tag::identifier(FMAN_ADVERTISEMENT_D_TAG),
                    Tag::hashtag(FMAN_ADVERTISEMENT_HASHTAG),
                ]),
            )
            .await
            .expect("publish additional FMan advertisement");
    }

    // Exercise static admission failures through the real relay alongside the
    // valid pool. Every fixture uses a distinct author so NIP-01 replacement
    // cannot hide another case.
    let unparsable_keys = NostrKeys::generate();
    NostrRelayClient::connect(&relay.url, unparsable_keys, Duration::from_secs(5))
        .await
        .expect("connect unparsable advertisement publisher")
        .publish_event(
            EventBuilder::new(Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND), "not-json").tags([
                Tag::identifier(FMAN_ADVERTISEMENT_D_TAG),
                Tag::hashtag(FMAN_ADVERTISEMENT_HASHTAG),
            ]),
        )
        .await
        .expect("publish unparsable advertisement");

    let missing_badge_keys = NostrKeys::generate();
    let mut missing_badge = advertisement_payload.clone();
    missing_badge.fman_id_pubkey = missing_badge_keys.public_key().to_string();
    missing_badge.holder_authorizations.clear();
    publish_advertisement_payload(&relay.url, missing_badge_keys, missing_badge).await;

    let unsupported_size_keys = NostrKeys::generate();
    let mut unsupported_size = advertisement_payload.clone();
    unsupported_size.fman_id_pubkey = unsupported_size_keys.public_key().to_string();
    unsupported_size.availability.federation_sizes = vec![10];
    publish_advertisement_payload(&relay.url, unsupported_size_keys, unsupported_size).await;

    // Static admission deliberately carries credential claims without
    // trusting them. Make this candidate cheapest so lazy selection must
    // examine it, then prove the real verifier rejects the stolen badge whose
    // authorized subject is the original FMan rather than this event author.
    let stolen_badge_keys = NostrKeys::generate();
    let mut stolen_badge = advertisement_payload.clone();
    stolen_badge.fman_id_pubkey = stolen_badge_keys.public_key().to_string();
    stolen_badge.service_pubkey = SecretKey::from_slice(&[99; 32])
        .expect("test key is valid")
        .x_only_public_key(&Secp256k1::new())
        .0
        .to_string();
    stolen_badge.api_endpoints = vec![ApiEndpoint {
        transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
        url: format!(
            "{IROH_API_ENDPOINT_URL_SCHEME}{}",
            IrohSecretKey::from_bytes(&[100; 32]).public()
        ),
    }];
    stolen_badge.plans = vec![Plan::InfiniteBestEffort { price_msats: 1 }];
    publish_advertisement_payload(&relay.url, stolen_badge_keys, stolen_badge).await;

    let concrete_verifier = PeerBadgeVerifier::new_for_test(
        [issuer_metadata.issuer_id_pubkey.0],
        [relay.url.parse().expect("defe relay URL parses")],
        TRUSTED_PEER_BADGE_LEVEL,
    )
    .expect("construct concrete verifier against the defe relay");
    assert_eq!(
        concrete_verifier.provenance(),
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration
    );
    let registry = NostrFiClient::new(
        NostrRelayClient::connect(&relay.url, NostrKeys::generate(), Duration::from_secs(5))
            .await
            .expect("connect final FI registry client"),
    );
    let registry_query = FmanRegistryQuery::new(registry);
    let discovery_options = FmanDiscoveryOptions::with_timeout(Duration::from_secs(30));
    let requirements = FmanCandidateRequirements {
        federation_size: FederationSize(7),
        fedimintd_versions: FedimintdVersionRange::one_core(
            "0.8.0"
                .parse::<FedimintdVersion>()
                .expect("version parses")
                .core(),
        )
        .expect("version core can form a range"),
    };
    let discovery = registry_query
        .discover_fman_candidates(&requirements, discovery_options)
        .await
        .expect("concrete FI registry query discovers advertisements");
    assert_eq!(
        discovery.candidates.len(),
        8,
        "seven valid and one credential-unverified advertisement pass static admission: {:#?}",
        discovery.rejected
    );
    assert_eq!(
        discovery
            .rejected
            .iter()
            .map(|rejected| rejected.reason.code())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "missing_holder_authorization",
            "unparsable_document",
            "unsupported_federation_size",
        ])
    );

    // The same discovery through a pooled client whose first relay is dead:
    // one live relay keeps the enumeration complete, so a single relay
    // outage cannot stop FIs from finding FMans.
    let dead_relay: nostr_sdk::RelayUrl =
        "ws://127.0.0.1:9".parse().expect("dead relay URL parses");
    let live_relay: nostr_sdk::RelayUrl = relay.url.parse().expect("defe relay URL parses");
    let pooled_registry = NostrFiClient::new(
        NostrRelayClient::connect_pool(
            &[dead_relay, live_relay],
            NostrKeys::generate(),
            Duration::from_secs(5),
        )
        .await
        .expect("connect pooled FI registry client with a dead first relay"),
    );
    let pooled_discovery_started = std::time::Instant::now();
    let pooled_discovery = FmanRegistryQuery::new(pooled_registry)
        .discover_fman_candidates(&requirements, discovery_options)
        .await
        .expect("pooled FI registry query discovers advertisements past the dead relay");
    assert_eq!(
        pooled_discovery.candidates.len(),
        8,
        "a dead first relay must not change what discovery returns: {:#?}",
        pooled_discovery.rejected
    );
    // The dead relay must be skipped, not waited out: well under the 30s
    // discovery deadline, generous slack for a loaded CI host.
    assert!(
        pooled_discovery_started.elapsed() < Duration::from_secs(15),
        "pooled discovery waited out the deadline instead of skipping the dead relay"
    );

    let query = registry_query.with_verifier(concrete_verifier);
    let preview = query
        .preview_fman_selection(
            &FmanSelectionRequest::new(
                FederationSize(7),
                requirements.fedimintd_versions.clone(),
                PlanPreference::InfiniteBestEffort,
            )
            .expect("selection request is valid"),
            discovery_options,
        )
        .await
        .expect("concrete verifier fills the FI selection preview");

    assert_eq!(preview.selected(), 7);
    assert_eq!(preview.eligible(), 8);
    assert_eq!(preview.total_advertised_msats(), 70_000_000);
    assert_eq!(
        preview
            .rejected()
            .iter()
            .map(|rejected| rejected.reason.code())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "missing_holder_authorization",
            "subject_mismatch",
            "unparsable_document",
            "unsupported_federation_size",
        ])
    );
    for seat in preview.seats() {
        assert_eq!(
            seat.candidate().badge().issuer(),
            issuer_metadata.issuer_id_pubkey.0
        );
        assert_eq!(
            seat.candidate().badge().subject(),
            seat.candidate().fman_id()
        );
        assert_eq!(
            seat.candidate().badge().badge().trust_level,
            TRUSTED_PEER_BADGE_LEVEL
        );
    }
}

async fn publish_advertisement_payload(
    relay_url: &str,
    keys: NostrKeys,
    payload: AdvertisementPayload,
) {
    let document =
        sign_advertisement(payload, &keys).expect("sign rejection-fixture advertisement");
    NostrRelayClient::connect(relay_url, keys, Duration::from_secs(5))
        .await
        .expect("connect rejection-fixture publisher")
        .publish_event(
            EventBuilder::new(
                Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND),
                serde_json::to_string(&document)
                    .expect("serialize rejection-fixture advertisement"),
            )
            .tags([
                Tag::identifier(FMAN_ADVERTISEMENT_D_TAG),
                Tag::hashtag(FMAN_ADVERTISEMENT_HASHTAG),
            ]),
        )
        .await
        .expect("publish rejection-fixture advertisement");
}

fn test_issuer_secret_keys() -> IssuerSecretKeys {
    // IssuerContext::generate() does real 2048-bit RSA keygen and is too slow for
    // this exploratory e2e test. These fixed test keys are copied from the
    // credential SDK's own test fixtures.
    serde_json::from_value(json!({
        "issuer_id_secret_key": "76127aa07dc3a3dcad06c8f8835ff997adb9c542868434bc47d16f1c9ba860b8",
        "issuance_secret_key": "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDAQ1EwvOUFvlSU0vvwrRFsZoFtswUS1kdp0zxpmSF1clbKtpuY2TXkhSsOXMtAy2Ci2tCQ1_bqviht3pYTuF2KkBFa_0lbNXf1-jVbjckvWhjVfQoTNUn9QzvUPQklSBEokEXgHjvhI4vASaCWStl7Os5FfZW6MJ7CPNuSLouuIoI9aWTplP2-PD4DC9kzP3sRBSugVvx6CgPjPCq9T1eQzy52Ed18bpY0IKgvBkKnnc2j2JuvDENRDX2KxLHjpymDJhrMC_pSTxSnUMOncozdw-HI7E1I7t59gWiXz0S8Uk5kom2NS2x4QUkFKjpxQwarupAObUhtnDaLjCrszybxAgMBAAECggEAMxqxng7XoWsx-E0MgrC-DN5CUPJgyt0CJnLrf_YgGqPFxiQ7v6kc1h0_kJXBwPtOOHuJLLb6_vKEtI-RvLQoyQf6VQG-cewIcu2K-Ub6zwdXyoduAiUMAbG5WXTP1YUOaoXOzP-8Ut-r6fSoJsrGfCbpZTc4cUEzMdYTVwvgPOyhJr66lD26wWMnJD7hk8qi54lhpWG2fkwR61eSKhO_sBLUYXPywxkGVLRfXVpXZxxr8EDMDsxeD03Y6rZOMAS3-g4xv8-dIGFjbIPH_VsZn8g8eRmtAaaVLoDGfphaOfP5JSYw76QLzj5Y0Slzf3wUaaK3dxbAQoUIKi_RaCb7sQKBgQDRcOQ9hqQTF0g5TovWw8nLwJyCPrbqcjDT6MuQYDWKzKzPeQ6fPcjbpCgme7YCUZZ8AT2n9yZaFWOjNxGyRKps-YcBI2nhmQWzuV_UcmayxtehJ0ee3PyukKs8aJieuBwb9xFzZ5-ekSiDbghmA-wSvHDXoLFf1HDZXhH3XpxgBwKBgQDrANa5p1wmzNcW4Lvh8qkFhE9eGTbKugpxw94I6Qj2RQImupVBySSt1v_pi2771R66foBvspnzaEf505BNppYZ9jh3zLS3jjhztkkK76MOilho0cFHF0328s3AgNI8LFQDYpVp-_rCDb6NwPPLAhEewyecL690xvE_NbUMlTATRwKBgQDCnaZYzZ3053ODXMtwe2ouXQKRvHj4Dbf1kaJmvB_EpEAIYjMGIcFc54Mvj1EngmzVOcnzJCONHccCSQ-2mTvMG2op0qB2s1yrDpxPqyZnBYIlC3zvz-U0yNV1QrRe-DGWgtTCag3WqIf-6OYA9bAOEPDCTV3E8IEUWudS96VTTQKBgQDYbNlT-XHAuf2MsEPX_ubykbuWaZowcc2UoFIn2pXKWBt3F3bGMzx4bP0aVLNNciTuk_os5EssA-nlhpXrLXQnTL8MdZYpRe1vg30ZeUCt73MkdaiOlEPVHh-nHfyANkLZKz13cfyqIoZPflgHqkuiDRC5oqDv5xfeotOuVucDmQKBgH_9bUklrSGmRvIKwPyuaP52vSOWginmXzjRKvOGIleg6RRQs4tlbsVluHeQx7bZQQ4b578NYyK78FWfX1AG1OrbscHN8vUrSTN_viPGn6gXpxL0KDaX8okd7zdixwwxqYD0juxmLlaRSTGTAwUF0f-EkPDuNdisG-gkbbsBRJat",
    }))
    .expect("test issuer keys deserialize")
}

fn tag_value(event: &Event, name: &str) -> Option<String> {
    event
        .tags
        .iter()
        .map(Tag::as_slice)
        .find(|tag| tag.first().map(String::as_str) == Some(name))?
        .get(1)
        .cloned()
}

fn verify_fman_ad_payload(fman_pubkey: NostrPublicKey, payload: &Value, signature: &str) {
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature)
        .expect("advertisement proof signature is base64url-unpadded");
    let signature =
        Signature::from_slice(&signature_bytes).expect("advertisement proof signature parses");
    let message = fman_ad_message(payload);
    Secp256k1::verification_only()
        .verify_schnorr(
            &signature,
            &message,
            &fman_pubkey
                .xonly()
                .expect("FMan pubkey converts to x-only key"),
        )
        .expect("advertisement proof signature verifies");
}

fn fman_ad_message(payload: &Value) -> Message {
    let mut hasher = Sha256::new();
    hasher.update(FMAN_ADVERTISEMENT_SIGNATURE_DOMAIN);
    hasher.update(
        serde_json_canonicalizer::to_vec(payload)
            .expect("canonicalize advertisement payload for signing"),
    );
    Message::from_digest_slice(&hasher.finalize()).expect("sha256 digest is a secp message")
}
