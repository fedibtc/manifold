//! Pipeline tests for read-only MG advertisement discovery.
//!
//! The admission order, freshness window, eligibility filter, enumeration
//! bounds, and deadline semantics these tests pin are recorded in
//! `crates/fa-client/specs/ARCH-fa-client-discovery-selection.md`. Badge verification
//! deliberately does not run in this pipeline; the selection-walk tests in
//! `tests/selection.rs` pin the lazy verification order.

use fedi_decentralized_domain::HolderAuthorizationEnvelope;
use fedi_decentralized_nostr::mg::{
    AdvertisedUptime, AdvertisementV2Payload, MG_ADVERTISEMENT_V2_D_TAG,
    MG_ADVERTISEMENT_V2_EVENT_KIND, MG_ADVERTISEMENT_V2_HASHTAG, Permille, sign_advertisement_v2,
};
use fedi_decentralized_nostr::mg::{
    AdvertisementPayload, ApiEndpoint, Availability, IROH_API_ENDPOINT_TRANSPORT,
    IROH_API_ENDPOINT_URL_SCHEME, MG_ADVERTISEMENT_D_TAG, MG_ADVERTISEMENT_EVENT_KIND,
    MG_ADVERTISEMENT_HASHTAG, sign_advertisement,
};
use fedi_decentralized_nostr_clients::MG_ADVERTISEMENTS_CANDIDATE_LIMIT;
use fedimint_core::runtime::Instant;
use peerbadge_protocol::{
    Credential, CredentialDigest, CredentialProof, HolderAuthorization,
    HolderAuthorizationStatement, HolderId, IssuerId, ProtocolV1, SchnorrSignatureProof,
    SignedCredential, SubjectPubkey, Timestamp as SdkTimestamp,
};

use super::*;
use crate::discovery::discover_mg_candidates_with;

pub(crate) const NOW: u64 = 1_750_000_000;
pub(crate) const AD_PRICE_MSATS: u64 = 250_000;
const OTHER_AD_PRICE_MSATS: u64 = 125_000;

pub(crate) fn mg_keys(index: u8) -> Keys {
    mg_keys_u64(u64::from(index))
}

fn mg_keys_u64(index: u64) -> Keys {
    Keys::parse(&format!("{:064x}", 0x1000 + index)).expect("test key parses")
}

pub(crate) fn holder_keys() -> Keys {
    Keys::parse(&format!("{:064x}", 0x2001)).expect("test holder key parses")
}

pub(crate) fn issuer_keys(index: u8) -> Keys {
    Keys::parse(&format!("{:064x}", 0x3001 + u64::from(index))).expect("test issuer key parses")
}

pub(crate) fn envelope(holder: &Keys, subject: PublicKey) -> HolderAuthorizationEnvelope {
    envelope_with_issuer(holder, subject, &issuer_keys(0))
}

pub(crate) fn envelope_with_issuer(
    holder: &Keys,
    subject: PublicKey,
    issuer: &Keys,
) -> HolderAuthorizationEnvelope {
    let credential = Credential {
        issuer_id_pubkey: IssuerId(issuer.public_key()),
        info: serde_json::json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": test_peer_badge_minimum_trust_level(),
        }),
        blind_msg: serde_json::json!(holder.public_key().to_string()),
    };
    let statement = HolderAuthorizationStatement {
        holder_id_pubkey: HolderId(holder.public_key()),
        subject_pubkey: SubjectPubkey(subject),
        credential_digest: CredentialDigest(credential.digest().expect("test digest")),
        issued_at: SdkTimestamp(NOW - 60),
    };
    let signature = holder.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(
        statement.digest().expect("test digest").into(),
    ));
    HolderAuthorizationEnvelope {
        holder_authorization: HolderAuthorization {
            version: ProtocolV1,
            authorization: statement,
            proof: SchnorrSignatureProof { signature },
        },
        signed_credential: SignedCredential {
            version: ProtocolV1,
            credential,
            proof: CredentialProof {
                signature: blind_rsa_signatures::Signature(vec![1, 2, 3, 4]),
            },
        },
    }
}

/// Commitment-signing service key advertised by one test MG; distinct from
/// its Nostr identity and from every other test MG's service key.
pub(crate) fn service_pubkey(mg: &Keys) -> secp256k1::XOnlyPublicKey {
    let mut secret_bytes = mg.secret_key().secret_bytes();
    secret_bytes[0] = 5;
    Keypair::from_secret_key(
        SECP256K1,
        &SecretKey::from_byte_array(&secret_bytes).expect("valid derived test service secret"),
    )
    .x_only_public_key()
    .0
}

/// Iroh endpoint id advertised by test payloads.
fn endpoint_id() -> fedi_iroh_rpc::iroh::EndpointId {
    IrohSecretKey::from_bytes(&[9; 32]).public()
}

pub(crate) fn payload(
    mg: &Keys,
    envelopes: Vec<HolderAuthorizationEnvelope>,
) -> AdvertisementPayload {
    AdvertisementPayload {
        version: ProtocolV1,
        mg_id_pubkey: mg.public_key().to_string(),
        service_pubkey: service_pubkey(mg).to_string(),
        issued_at: NOW - 3_600,
        expires_at: NOW + 3_600,
        api_endpoints: vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}{}", endpoint_id()),
        }],
        availability: Availability {
            fedimintd_version: FEDIMINTD_VERSION_0_1.parse().expect("test version parses"),
            federation_sizes: FEDERATION_SIZES_0_1.to_vec(),
        },
        plans: vec![Plan::InfiniteBestEffort {
            price_msats: AD_PRICE_MSATS,
        }],
        holder_authorizations: envelopes,
    }
}

pub(crate) fn self_authorized_payload(mg: &Keys) -> AdvertisementPayload {
    payload(mg, vec![envelope(mg, mg.public_key())])
}

pub(crate) fn ad_event_at(mg: &Keys, payload: AdvertisementPayload, created_at: u64) -> Event {
    let document = sign_advertisement(payload, mg).expect("test advertisement signs");
    let content = serde_json::to_string(&document).expect("test advertisement serializes");
    EventBuilder::new(Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND), content)
        .tag(Tag::identifier(MG_ADVERTISEMENT_D_TAG))
        .tag(Tag::hashtag(MG_ADVERTISEMENT_HASHTAG))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(created_at))
        .sign_with_keys(mg)
        .expect("test advertisement event signs")
}

pub(crate) fn ad_event(mg: &Keys, payload: AdvertisementPayload) -> Event {
    ad_event_at(mg, payload, NOW - 600)
}

pub(crate) fn requirements() -> crate::MgCandidateRequirements {
    crate::MgCandidateRequirements {
        federation_size: FederationSize(MIN_FEDERATION_SIZE),
        fedimintd_versions: FedimintdVersionRange::one_core(
            FEDIMINTD_VERSION_0_1
                .parse::<FedimintdVersion>()
                .expect("test version parses")
                .core(),
        )
        .expect("test version core can form a range"),
    }
}

pub(crate) fn registry(advertisements: Vec<Event>) -> TestRegistry {
    let registry = TestRegistry::default();
    *registry.advertisements.lock().expect("test lock") = advertisements;
    registry
}

pub(crate) fn generous_deadline() -> Instant {
    Instant::now() + crate::MG_DISCOVERY_TIMEOUT
}

async fn discover(advertisements: Vec<Event>) -> crate::MgDiscovery {
    discover_mg_candidates_with(
        &registry(advertisements),
        &requirements(),
        generous_deadline(),
        NOW,
    )
    .await
    .expect("discovery completes")
}

async fn sole_rejection(advertisements: Vec<Event>) -> AdvertisementRejection {
    let mut discovery = discover(advertisements).await;
    assert!(discovery.candidates.is_empty(), "no candidate expected");
    assert_eq!(discovery.rejected.len(), 1);
    discovery.rejected.pop().expect("one rejection").reason
}

#[tokio::test]
async fn empty_relay_result_discovers_nothing() {
    let discovery = discover(Vec::new()).await;
    assert!(discovery.candidates.is_empty());
    assert!(discovery.rejected.is_empty());
    assert_eq!(discovery.seen(), 0);
}

#[tokio::test]
async fn relay_failure_returns_typed_registry_error() {
    let registry = TestRegistry::default();
    registry.fail.store(true, Ordering::SeqCst);
    let error = discover_mg_candidates_with(&registry, &requirements(), generous_deadline(), NOW)
        .await
        .unwrap_err();
    assert!(matches!(error, FaError::Registry(_)), "{error}");
    assert_eq!(error.code(), FaErrorCode::Registry);
}

#[tokio::test]
async fn happy_path_returns_every_eligible_candidate() {
    let first = mg_keys(1);
    let second = mg_keys(2);
    let discovery = discover(vec![
        ad_event(&first, self_authorized_payload(&first)),
        ad_event(&second, self_authorized_payload(&second)),
    ])
    .await;

    assert!(discovery.rejected.is_empty(), "{:?}", discovery.rejected);
    assert_eq!(discovery.candidates.len(), 2);
    assert_eq!(discovery.seen(), 2);
    let mut expected = [first.public_key(), second.public_key()];
    expected.sort();
    let mut returned = discovery
        .candidates
        .iter()
        .map(|candidate| candidate.mg_id())
        .collect::<Vec<_>>();
    returned.sort();
    assert_eq!(
        returned,
        expected.to_vec(),
        "every eligible author appears exactly once, in some order",
    );
    let candidate = &discovery.candidates[0];
    assert_eq!(
        candidate.mg_name(),
        fedi_decentralized_service_manifold_guardian::MgName::from_mg_id(candidate.mg_id()),
    );
    assert_eq!(candidate.advertised_price_msats(), AD_PRICE_MSATS);
    assert_eq!(candidate.issued_at(), Timestamp(NOW - 3_600));
    assert_eq!(candidate.expires_at(), Timestamp(NOW + 3_600));
    assert_eq!(
        candidate.claimed_issuer(),
        issuer_keys(0).public_key(),
        "the claimed issuer is read locally from the first envelope",
    );
    assert_eq!(
        candidate.api_endpoints(),
        vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}{}", endpoint_id()),
        }],
    );
    assert_eq!(
        candidate.locator(),
        &Locator::new(
            EndpointAddr::new(endpoint_id()),
            service_pubkey(if candidate.mg_id() == first.public_key() {
                &first
            } else {
                &second
            }),
        ),
        "the candidate locator matches one built directly from the same \
         endpoint id and service pubkey",
    );
}

/// The resource cap deliberately admits a publisher-controlled prefix, not a
/// fair sample. Keep this counterexample here so a future "fairness" change
/// must make an explicit policy choice rather than silently changing the
/// accepted spam boundary.
#[tokio::test]
async fn candidate_cap_can_omit_an_honest_eligible_advertisement() {
    let now = fedimint_core::time::duration_since_epoch().as_secs();
    let options = MgDiscoveryOptions::with_timeout(std::time::Duration::from_secs(10 * 60));
    let mut advertisements = (0..u64::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT))
        .map(|index| {
            let spammer = mg_keys_u64(index);
            let mut payload = self_authorized_payload(&spammer);
            payload.issued_at = now - 60 * 60;
            payload.expires_at = now + 60 * 60;
            ad_event_at(&spammer, payload, now - 30)
        })
        .collect::<Vec<_>>();
    let honest = mg_keys_u64(u64::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT));
    let mut honest_payload = self_authorized_payload(&honest);
    honest_payload.issued_at = now - 60 * 60;
    honest_payload.expires_at = now + 60 * 60;
    let honest_event = ad_event_at(&honest, honest_payload, now - 30);

    let honest_discovery = MgRegistryQuery::new(registry(vec![honest_event.clone()]))
        .discover_mg_candidates(&requirements(), options)
        .await
        .expect("the honest advertisement is eligible alone");
    assert_eq!(
        honest_discovery
            .candidates
            .iter()
            .map(|candidate| candidate.mg_id())
            .collect::<Vec<_>>(),
        vec![honest.public_key()]
    );
    advertisements.push(honest_event);

    let discovery = MgRegistryQuery::new(registry(advertisements))
        .discover_mg_candidates(&requirements(), options)
        .await
        .expect("the retained prefix completes discovery");

    assert_eq!(
        discovery.seen(),
        usize::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT),
        "discovery retains only the bounded prefix"
    );
    assert_eq!(
        discovery.candidates.len(),
        usize::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT),
        "every retained spam event remains statically eligible"
    );
    assert!(
        discovery
            .candidates
            .iter()
            .all(|candidate| candidate.mg_id() != honest.public_key()),
        "the honest eligible advertisement after the spam prefix is omitted"
    );
}

#[tokio::test]
async fn invalid_event_signature_is_rejected() {
    let mg = mg_keys(1);
    let event = ad_event(&mg, self_authorized_payload(&mg));
    let mut tampered = serde_json::to_value(&event).expect("event serializes");
    tampered["created_at"] = serde_json::json!(NOW - 599);
    let tampered = serde_json::from_value::<Event>(tampered).expect("event deserializes");

    let reason = sole_rejection(vec![tampered]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::InvalidEventSignature
    ));
}

#[tokio::test]
async fn wrong_event_role_is_rejected() {
    let mg = mg_keys(1);
    let document = sign_advertisement(self_authorized_payload(&mg), &mg).expect("test ad signs");
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        serde_json::to_string(&document).expect("test ad serializes"),
    )
    .tag(Tag::identifier("not-the-mg-ad"))
    .sign_with_keys(&mg)
    .expect("test event signs");

    let reason = sole_rejection(vec![event]).await;
    assert!(matches!(reason, AdvertisementRejection::WrongEventRole));
}

#[tokio::test]
async fn unparsable_document_is_rejected() {
    let mg = mg_keys(1);
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        "not an advertisement document",
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_D_TAG))
    .sign_with_keys(&mg)
    .expect("test event signs");

    let reason = sole_rejection(vec![event]).await;
    assert!(matches!(reason, AdvertisementRejection::UnparsableDocument));
}

#[tokio::test]
async fn invalid_advertisement_proof_is_rejected() {
    let mg = mg_keys(1);
    let mut document =
        sign_advertisement(self_authorized_payload(&mg), &mg).expect("test ad signs");
    document.payload.availability.fedimintd_version =
        "9.9.9+fedi".parse().expect("test version parses");
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        serde_json::to_string(&document).expect("test ad serializes"),
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_D_TAG))
    .sign_with_keys(&mg)
    .expect("test event signs");

    let reason = sole_rejection(vec![event]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::InvalidAdvertisementProof
    ));
}

#[tokio::test]
async fn payload_author_mismatch_is_rejected() {
    // A validly signed document by one key republished under another author.
    let real_mg = mg_keys(1);
    let republisher = mg_keys(2);
    let document =
        sign_advertisement(self_authorized_payload(&real_mg), &real_mg).expect("test ad signs");
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        serde_json::to_string(&document).expect("test ad serializes"),
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_D_TAG))
    .sign_with_keys(&republisher)
    .expect("test event signs");

    let reason = sole_rejection(vec![event]).await;
    assert!(matches!(reason, AdvertisementRejection::AuthorMismatch));
}

#[tokio::test]
async fn missing_holder_authorization_is_rejected() {
    let mg = mg_keys(1);
    let event = ad_event(&mg, payload(&mg, Vec::new()));

    let reason = sole_rejection(vec![event]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::MissingHolderAuthorization
    ));
}

#[tokio::test]
async fn expired_advertisement_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.expires_at = NOW;
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(reason, AdvertisementRejection::Expired));
}

#[tokio::test]
async fn future_issued_advertisement_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.issued_at = NOW + 60;
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(reason, AdvertisementRejection::IssuedInFuture));
}

#[tokio::test]
async fn over_max_age_advertisement_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.issued_at = NOW - MG_ADVERTISEMENT_MAX_AGE.as_secs() - 1;
    payload.expires_at = NOW + 3_600;
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(reason, AdvertisementRejection::Stale));
}

#[tokio::test]
async fn unsupported_federation_size_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.availability.federation_sizes = vec![10, 13];
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::UnsupportedFederationSize
    ));
}

#[tokio::test]
async fn unsupported_fedimintd_version_is_rejected() {
    for version in ["9.9.9+fedi", "0.11.1", "0.11.1+acme"] {
        let mg = mg_keys(1);
        let mut payload = self_authorized_payload(&mg);
        payload.availability.fedimintd_version = version.parse().expect("test version parses");
        let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
        assert!(matches!(
            reason,
            AdvertisementRejection::UnsupportedFedimintdVersion
        ));
    }
}

#[tokio::test]
async fn missing_infinite_best_effort_plan_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.plans = vec![Plan::SubscriptionBased {
        initial_price_msats: AD_PRICE_MSATS,
        renewal_price_msats: AD_PRICE_MSATS,
        period: "every-30-days".to_owned(),
    }];
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::NoInfiniteBestEffortPlan
    ));
}

#[tokio::test]
async fn malformed_service_pubkey_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.service_pubkey = "not-a-service-pubkey".to_owned();
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(
        reason,
        AdvertisementRejection::MalformedServicePubkey
    ));
}

#[tokio::test]
async fn advertisement_without_a_parseable_iroh_endpoint_is_rejected() {
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.api_endpoints = vec![
        // Wrong transport, even with a parseable-looking URL.
        ApiEndpoint {
            transport: "https".to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}{}", endpoint_id()),
        },
        // Right transport, unparseable endpoint id.
        ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}endpoint"),
        },
    ];
    let reason = sole_rejection(vec![ad_event(&mg, payload)]).await;
    assert!(matches!(reason, AdvertisementRejection::NoDialableEndpoint));
}

#[tokio::test]
async fn later_endpoint_in_the_list_can_supply_the_locator() {
    // Unparseable entries are skipped, not fatal: the first parseable iroh
    // endpoint supplies the locator.
    let mg = mg_keys(1);
    let mut payload = self_authorized_payload(&mg);
    payload.api_endpoints = vec![
        ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}unparseable"),
        },
        ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!(
                "{IROH_API_ENDPOINT_URL_SCHEME}{}?relay=https://relay.example/",
                endpoint_id()
            ),
        },
    ];
    let discovery = discover(vec![ad_event(&mg, payload)]).await;
    assert_eq!(discovery.candidates.len(), 1);
    assert_eq!(
        discovery.candidates[0].locator(),
        &Locator::new(EndpointAddr::new(endpoint_id()), service_pubkey(&mg)),
        "URL components after the endpoint id are ignored",
    );
}

#[tokio::test]
async fn author_dedupe_keeps_the_newest_advertisement() {
    let mg = mg_keys(1);
    let mut older_payload = self_authorized_payload(&mg);
    older_payload.plans = vec![Plan::InfiniteBestEffort {
        price_msats: OTHER_AD_PRICE_MSATS,
    }];
    let older = ad_event_at(&mg, older_payload, NOW - 900);
    let newer = ad_event_at(&mg, self_authorized_payload(&mg), NOW - 600);

    // Relay order must not matter: present the older event first.
    let discovery = discover(vec![older, newer]).await;

    assert_eq!(discovery.candidates.len(), 1);
    assert_eq!(
        discovery.candidates[0].advertised_price_msats(),
        AD_PRICE_MSATS,
        "the newest advertisement's offer wins",
    );
    assert_eq!(discovery.rejected.len(), 1);
    assert!(matches!(
        discovery.rejected[0].reason,
        AdvertisementRejection::Superseded
    ));
    assert_eq!(discovery.seen(), 2);
}

#[tokio::test]
async fn author_dedupe_breaks_equal_created_at_ties_by_lowest_event_id() {
    // NIP-01 replacement order: at equal created_at the event with the
    // lexicographically lowest id wins, independent of relay order.
    let mg = mg_keys(1);
    let mut cheaper_payload = self_authorized_payload(&mg);
    cheaper_payload.plans = vec![Plan::InfiniteBestEffort {
        price_msats: OTHER_AD_PRICE_MSATS,
    }];
    let cheaper = ad_event_at(&mg, cheaper_payload, NOW - 600);
    let dearer = ad_event_at(&mg, self_authorized_payload(&mg), NOW - 600);
    assert_eq!(cheaper.created_at, dearer.created_at);

    let winner_price = if cheaper.id.as_bytes() < dearer.id.as_bytes() {
        OTHER_AD_PRICE_MSATS
    } else {
        AD_PRICE_MSATS
    };
    for events in [vec![cheaper.clone(), dearer.clone()], vec![dearer, cheaper]] {
        let discovery = discover(events).await;
        assert_eq!(discovery.candidates.len(), 1);
        assert_eq!(
            discovery.candidates[0].advertised_price_msats(),
            winner_price,
            "the lowest event id wins an equal-created_at tie",
        );
        assert_eq!(discovery.rejected.len(), 1);
        assert!(matches!(
            discovery.rejected[0].reason,
            AdvertisementRejection::Superseded
        ));
    }
}

#[tokio::test]
async fn candidate_cap_is_enforced_locally() {
    // The pipeline must observe at most `MG_ADVERTISEMENTS_CANDIDATE_LIMIT`
    // events from a misbehaving injected registry. Constructing thousands of
    // fully signed advertisements would dominate the test's runtime without
    // sharpening the assertion, so the overflow filler is one cheap
    // wrong-role event cloned past the cap: every clone is still an observed
    // (rejected) candidate, while the few real advertisements at the front
    // prove admission keeps working inside the bounded window.
    let admitted = 3_usize;
    let over_limit = usize::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT) + 3;
    let mut events = (0..admitted)
        .map(|index| {
            let mg = mg_keys(u8::try_from(index + 1).expect("small test index"));
            ad_event(&mg, self_authorized_payload(&mg))
        })
        .collect::<Vec<_>>();
    // The test relay answers each format's query by kind, so the filler
    // keeps the legacy kind and has the wrong role through its `d` tag.
    let filler = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        "wrong-role filler",
    )
    .tag(Tag::identifier("not-the-mg-ad"))
    .sign_with_keys(&mg_keys(0xff))
    .expect("test filler event signs");
    events.extend(std::iter::repeat_n(filler, over_limit - admitted));

    let discovery = discover(events).await;

    assert_eq!(
        discovery.seen(),
        usize::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT),
        "the transport bound is re-applied before admission work",
    );
    assert_eq!(discovery.candidates.len(), admitted);
}

/// Registry wrapper that returns its events only after a delay, so a short
/// deadline has deterministically expired by the time per-advertisement
/// admission starts.
pub(crate) struct SlowRegistry {
    pub(crate) registry: TestRegistry,
    pub(crate) delay: Duration,
}

impl FaNostrClient for SlowRegistry {
    async fn fetch_mg_advertisement(
        &self,
        mg_pubkey: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Event> {
        self.registry
            .fetch_mg_advertisement(mg_pubkey, timeout)
            .await
    }

    async fn fetch_setup_payment_federations(
        &self,
        publisher: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        self.registry
            .fetch_setup_payment_federations(publisher, timeout)
            .await
    }

    async fn fetch_mg_advertisements(
        &self,
        format: MgAdvertisementFormat,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        tokio::time::sleep(self.delay).await;
        self.registry.fetch_mg_advertisements(format, timeout).await
    }
}

#[test]
fn discovery_timeout_is_clamped_to_the_runtime_timer_domain() {
    let quantum = crate::MgDiscoveryOptions::with_timeout(Duration::from_millis(1));
    let maximum = crate::MgDiscoveryOptions::with_timeout(Duration::from_millis(i32::MAX as u64));
    assert_eq!(
        crate::MgDiscoveryOptions::with_timeout(Duration::ZERO),
        quantum,
        "a sub-millisecond timeout clamps up to the runtime quantum",
    );
    assert_ne!(
        crate::MgDiscoveryOptions::with_timeout(Duration::from_millis(2)),
        quantum,
        "an in-range timeout is preserved",
    );
    assert_eq!(
        crate::MgDiscoveryOptions::with_timeout(Duration::from_millis(i32::MAX as u64 + 1)),
        maximum,
        "an oversized timeout clamps down to the runtime maximum",
    );
    assert_ne!(
        maximum,
        crate::MgDiscoveryOptions::with_timeout(Duration::from_millis(i32::MAX as u64 - 1)),
        "the maximum boundary value is preserved",
    );
}

#[tokio::test]
async fn deadline_expiry_reports_unadmitted_advertisements() {
    let first = mg_keys(1);
    let second = mg_keys(2);
    let invalid = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        "static admission must not start",
    )
    .tag(Tag::identifier("not-the-mg-ad"))
    .sign_with_keys(&mg_keys(3))
    .expect("test event signs");
    let slow = SlowRegistry {
        registry: registry(vec![
            ad_event(&first, self_authorized_payload(&first)),
            ad_event(&second, self_authorized_payload(&second)),
            invalid,
        ]),
        delay: Duration::from_millis(50),
    };

    let discovery = discover_mg_candidates_with(
        &slow,
        &requirements(),
        // A one-millisecond deadline that the slow enumeration then
        // deterministically outlives.
        Instant::now() + Duration::from_millis(1),
        NOW,
    )
    .await
    .expect("discovery completes with typed rejections");

    assert!(discovery.candidates.is_empty());
    assert_eq!(discovery.rejected.len(), 3);
    assert!(
        discovery
            .rejected
            .iter()
            .all(|rejection| matches!(rejection.reason, AdvertisementRejection::DeadlineExpired)),
        "once enumeration returns after the deadline, neither valid nor invalid events begin static authentication"
    );
}

/// Advertisements carry no capacity count, so nothing downstream can spread
/// picks by "least used": discovery itself must not hand every FA the same
/// order. A deterministic (say, pubkey-sorted) result would survive every
/// other test in this file, so pin the shuffle directly.
#[tokio::test]
async fn candidates_come_back_in_a_fresh_random_order() {
    let mgs = (1..=8).map(mg_keys).collect::<Vec<_>>();
    let events = mgs
        .iter()
        .map(|mg| ad_event(mg, self_authorized_payload(mg)))
        .collect::<Vec<_>>();
    let mut sorted = mgs.iter().map(Keys::public_key).collect::<Vec<_>>();
    sorted.sort();

    let mut orders = Vec::new();
    for _ in 0..20 {
        let discovery = discover(events.clone()).await;
        assert_eq!(discovery.candidates.len(), 8);
        orders.push(
            discovery
                .candidates
                .iter()
                .map(|candidate| candidate.mg_id())
                .collect::<Vec<_>>(),
        );
    }

    // Twenty draws from 8! permutations: a fixed order here is not luck.
    assert!(
        orders.iter().any(|order| order != &sorted),
        "candidates were returned in MG-id order every time",
    );
    assert!(
        orders.iter().any(|order| order != &orders[0]),
        "candidates were returned in the same order every time",
    );
}

fn uptime() -> AdvertisedUptime {
    AdvertisedUptime {
        day: Permille::try_from(998).expect("in range"),
        week: Permille::try_from(991).expect("in range"),
        month: Permille::try_from(964).expect("in range"),
    }
}

/// The v2 rendering of `payload`, priced differently so a test can tell which
/// format discovery chose.
fn v2_offer(payload: AdvertisementPayload) -> AdvertisementV2Payload {
    let mut offer = AdvertisementV2Payload::from(payload);
    offer.plans = vec![Plan::InfiniteBestEffort {
        price_msats: OTHER_AD_PRICE_MSATS,
    }];
    offer.uptime_permille = Some(uptime());
    offer
}

/// A kind-37709 event authored by `author`, its envelope signed by `signer`.
fn v2_event_signed_by(
    author: &Keys,
    signer: &Keys,
    offer: &AdvertisementV2Payload,
    created_at: u64,
) -> Event {
    let content = sign_advertisement_v2(offer, signer)
        .expect("test v2 advertisement signs")
        .to_json();
    EventBuilder::new(Kind::Custom(MG_ADVERTISEMENT_V2_EVENT_KIND), content)
        .tag(Tag::identifier(MG_ADVERTISEMENT_V2_D_TAG))
        .tag(Tag::hashtag(MG_ADVERTISEMENT_V2_HASHTAG))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(created_at))
        .sign_with_keys(author)
        .expect("test v2 event signs")
}

fn v2_event(mg: &Keys, offer: &AdvertisementV2Payload) -> Event {
    v2_event_signed_by(mg, mg, offer, NOW - 600)
}

fn reasons(discovery: &crate::MgDiscovery) -> Vec<&'static str> {
    let mut reasons = discovery
        .rejected
        .iter()
        .map(|rejected| rejected.reason.code())
        .collect::<Vec<_>>();
    reasons.sort_unstable();
    reasons
}

/// A fresh v2 advertisement wins over the same author's legacy one, even a
/// newer legacy one, and carries the uptime report.
#[tokio::test]
async fn fresh_v2_advertisement_is_preferred_over_legacy() {
    let mg = mg_keys(1);
    let legacy_only = mg_keys(2);
    let discovery = discover(vec![
        // The legacy event is newer than the v2 one.
        ad_event_at(&mg, self_authorized_payload(&mg), NOW - 60),
        v2_event(&mg, &v2_offer(self_authorized_payload(&mg))),
        ad_event(&legacy_only, self_authorized_payload(&legacy_only)),
    ])
    .await;

    assert_eq!(reasons(&discovery), ["superseded"]);
    assert_eq!(discovery.seen(), 3, "each event is counted once");
    let candidate = |author: &Keys| {
        discovery
            .candidates
            .iter()
            .find(|candidate| candidate.mg_id() == author.public_key())
            .expect("author is a candidate")
    };
    assert_eq!(
        candidate(&mg).advertised_price_msats(),
        OTHER_AD_PRICE_MSATS
    );
    assert_eq!(candidate(&mg).self_reported_uptime(), Some(uptime()));
    assert_eq!(
        candidate(&legacy_only).advertised_price_msats(),
        AD_PRICE_MSATS
    );
    assert_eq!(
        candidate(&legacy_only).self_reported_uptime(),
        None,
        "a legacy advertisement has no uptime report",
    );
}

/// An author whose v2 advertisement is stale or invalid falls back to its
/// legacy advertisement, with no uptime and no field taken from the v2 one.
#[tokio::test]
async fn stale_or_invalid_v2_advertisement_falls_back_to_legacy() {
    let stale = mg_keys(1);
    let mut stale_offer = v2_offer(self_authorized_payload(&stale));
    stale_offer.issued_at = NOW - MG_ADVERTISEMENT_MAX_AGE.as_secs() - 1;
    let republished = mg_keys(2);
    let other = mg_keys(3);
    let discovery = discover(vec![
        ad_event(&stale, self_authorized_payload(&stale)),
        v2_event(&stale, &stale_offer),
        ad_event(&republished, self_authorized_payload(&republished)),
        // Another MG's valid envelope republished under this author.
        v2_event_signed_by(
            &republished,
            &other,
            &v2_offer(self_authorized_payload(&republished)),
            NOW - 600,
        ),
    ])
    .await;

    assert_eq!(reasons(&discovery), ["author_mismatch", "stale"]);
    assert_eq!(discovery.candidates.len(), 2);
    for candidate in &discovery.candidates {
        assert_eq!(candidate.advertised_price_msats(), AD_PRICE_MSATS);
        assert_eq!(candidate.self_reported_uptime(), None);
    }
}

#[tokio::test]
async fn v2_only_author_is_admitted_and_bad_v2_signatures_are_rejected() {
    let mg = mg_keys(1);
    let discovery = discover(vec![v2_event(&mg, &v2_offer(self_authorized_payload(&mg)))]).await;
    assert!(discovery.rejected.is_empty(), "{:?}", discovery.rejected);
    assert_eq!(discovery.candidates.len(), 1);
    assert_eq!(discovery.candidates[0].mg_id(), mg.public_key());

    // A valid envelope with one payload byte changed.
    let event = v2_event(&mg, &v2_offer(self_authorized_payload(&mg)));
    let mut envelope = serde_json::from_str::<serde_json::Value>(&event.content).unwrap();
    let tampered = envelope["payload"].as_str().unwrap().replacen(
        &OTHER_AD_PRICE_MSATS.to_string(),
        &AD_PRICE_MSATS.to_string(),
        1,
    );
    envelope["payload"] = serde_json::json!(tampered);
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_V2_EVENT_KIND),
        envelope.to_string(),
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_V2_D_TAG))
    .sign_with_keys(&mg)
    .unwrap();
    assert!(matches!(
        sole_rejection(vec![event]).await,
        AdvertisementRejection::InvalidAdvertisementProof
    ));

    // A v1 document under the v2 coordinate is not a v2 advertisement.
    let document = sign_advertisement(self_authorized_payload(&mg), &mg).unwrap();
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_V2_EVENT_KIND),
        serde_json::to_string(&document).unwrap(),
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_V2_D_TAG))
    .sign_with_keys(&mg)
    .unwrap();
    assert!(matches!(
        sole_rejection(vec![event]).await,
        AdvertisementRejection::UnparsableDocument
    ));

    // The v2 kind with the legacy `d` tag has the wrong role.
    let event = EventBuilder::new(
        Kind::Custom(MG_ADVERTISEMENT_V2_EVENT_KIND),
        v2_event(&mg, &v2_offer(self_authorized_payload(&mg))).content,
    )
    .tag(Tag::identifier(MG_ADVERTISEMENT_D_TAG))
    .sign_with_keys(&mg)
    .unwrap();
    assert!(matches!(
        sole_rejection(vec![event]).await,
        AdvertisementRejection::WrongEventRole
    ));
}

/// Each format has its own candidate bound: a legacy query filled to its
/// cap does not crowd out a v2 advertisement.
#[tokio::test]
async fn each_format_has_its_own_candidate_bound() {
    let mut events = (0..u64::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT))
        .map(|index| {
            let spammer = mg_keys_u64(10_000 + index);
            ad_event(&spammer, self_authorized_payload(&spammer))
        })
        .collect::<Vec<_>>();
    let honest = mg_keys(1);
    events.push(v2_event(
        &honest,
        &v2_offer(self_authorized_payload(&honest)),
    ));

    let discovery = discover(events).await;
    assert_eq!(
        discovery.seen(),
        usize::from(MG_ADVERTISEMENTS_CANDIDATE_LIMIT) + 1
    );
    assert!(
        discovery
            .candidates
            .iter()
            .any(|candidate| candidate.mg_id() == honest.public_key()
                && candidate.self_reported_uptime() == Some(uptime())),
        "the v2 advertisement is admitted beside a full legacy query",
    );
}
