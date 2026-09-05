use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, IssuerSecretKeys, PendingIssuance,
    RevocationLocation,
};
use fedi_decentralized_domain::{HolderAuthorizationEnvelope, TRUST_SCORE_SCHEMA_V1};
#[cfg(feature = "test-support")]
use fedi_decentralized_domain::{TRUST_SCORE_LEVEL_MAX, TRUST_SCORE_LEVEL_MIN};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::attester::{CREDENTIAL_REVOCATION_HASHTAG, ISSUER_AUTHORITY_HASHTAG};
use nostr_sdk::{EventBuilder, Keys, Tag, Timestamp};
use serde_json::json;

use super::*;

const AUTHORIZED_AT: u64 = 1_000;
const VERIFY_AT: u64 = 2_000;
const REVOCATION_RELAY: &str = "wss://revocations.example";

fn relay(url: &str) -> RelayUrl {
    RelayUrl::parse(url).expect("test relay URL")
}

static ISSUER_SECRET_KEYS: LazyLock<IssuerSecretKeys> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../domain/src/trust_score/issuer-secret-keys.json"
    ))
    .expect("fixed test issuer keys deserialize")
});

struct Fixture {
    issuer: IssuerContext,
    authority: IssuerAuthority,
    envelope: HolderAuthorizationEnvelope,
    authority_event: Event,
}

fn fixture() -> Fixture {
    fixture_with_trust_level(9)
}

fn fixture_with_trust_level(trust_level: u64) -> Fixture {
    let issuer =
        IssuerContext::import_secret_key(&ISSUER_SECRET_KEYS).expect("import fixed test issuer");
    let authority = issuer
        .issuer_authority(vec![RevocationLocation {
            protocol: "nostr".to_owned(),
            location: REVOCATION_RELAY.to_owned(),
        }])
        .expect("issue authority");
    let holder = HolderContext::generate();
    let info = json!({
        "schema": TRUST_SCORE_SCHEMA_V1,
        "trust_level": trust_level,
    });
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        json!(holder.public_key().to_string()),
    )
    .expect("create issuance request");
    let response = issuer
        .issue_credential(info, &request)
        .expect("issue credential");
    let credential = pending
        .finalize(&authority.issuer.issuance_key, &response)
        .expect("finalize credential");
    let subject = Keys::generate().public_key();
    let holder_authorization = holder
        .authorize_credential_use_at_time(
            HolderAuthorizationRequest {
                subject_pubkey: SubjectPubkey(subject),
            },
            &credential,
            AUTHORIZED_AT,
        )
        .expect("authorize credential");
    let envelope = HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential: credential,
    };
    let authority_event = issuer_event(
        ISSUER_AUTHORITY_EVENT_KIND,
        ISSUER_AUTHORITY_D_TAG,
        ISSUER_AUTHORITY_HASHTAG,
        serde_json::to_string(&authority).expect("serialize authority"),
        &issuer,
    );
    Fixture {
        issuer,
        authority,
        envelope,
        authority_event,
    }
}

fn issuer_event(
    kind: u16,
    d_tag: &str,
    hashtag: &str,
    content: String,
    issuer: &IssuerContext,
) -> Event {
    issuer_event_at(kind, d_tag, hashtag, content, issuer, 1_000)
}

fn issuer_event_at(
    kind: u16,
    d_tag: &str,
    hashtag: &str,
    content: String,
    issuer: &IssuerContext,
    created_at: u64,
) -> Event {
    let keys = Keys::parse(
        &issuer
            .export_secret_key()
            .expect("export issuer")
            .issuer_id_secret_key,
    )
    .expect("parse issuer identity");
    EventBuilder::new(Kind::Custom(kind), content)
        .tag(Tag::identifier(d_tag))
        .tag(Tag::hashtag(hashtag))
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(&keys)
        .expect("sign issuer event")
}

#[derive(Default)]
struct FakeSource {
    authority_calls: AtomicUsize,
    revocation_calls: AtomicUsize,
    last_revocation_relay_count: AtomicUsize,
    authority_unavailable: AtomicBool,
    revocation_unavailable: AtomicBool,
    authorities: Mutex<Vec<Event>>,
    revocations: Mutex<Vec<Event>>,
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl PeerBadgeEventSource for FakeSource {
    async fn fetch_issuer_authority_candidates(
        &self,
        _issuer: PublicKey,
        _deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError> {
        self.authority_calls.fetch_add(1, Ordering::SeqCst);
        if self.authority_unavailable.load(Ordering::SeqCst) {
            return Err(NostrClientError::MissingEvent {
                context: "fake authority",
            });
        }
        Ok(self.authorities.lock().expect("authority lock").clone())
    }

    async fn fetch_revocation_candidates(
        &self,
        _issuer: PublicKey,
        _credential_digest: &str,
        relay_urls: &[RelayUrl],
        _deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError> {
        self.revocation_calls.fetch_add(1, Ordering::SeqCst);
        self.last_revocation_relay_count
            .store(relay_urls.len(), Ordering::SeqCst);
        if self.revocation_unavailable.load(Ordering::SeqCst) {
            return Err(NostrClientError::MissingEvent {
                context: "fake revocation",
            });
        }
        Ok(self.revocations.lock().expect("revocation lock").clone())
    }
}

fn verification_deadline() -> Instant {
    Instant::now() + Duration::from_secs(1)
}

fn verifier(fixture: &Fixture, source: Arc<FakeSource>) -> PeerBadgeVerifier {
    source
        .authorities
        .lock()
        .expect("authority lock")
        .push(fixture.authority_event.clone());
    PeerBadgeVerifier::with_source(
        PeerBadgeIssuerRoots::new([fixture.authority.issuer.issuer_id_pubkey.0])
            .expect("trusted issuer root"),
        source,
        &ManifoldEnvironment::Development.profile().unwrap(),
    )
}

fn signed_revocation_event(fixture: &Fixture) -> Event {
    let revocation = fixture
        .issuer
        .revoke_credential(&fixture.envelope.signed_credential)
        .expect("revoke credential");
    let digest = credential_digest_wire_string(&revocation.revocation.credential_digest)
        .expect("serialize digest");
    issuer_event(
        CREDENTIAL_REVOCATION_EVENT_KIND,
        &credential_revocation_d_tag(&digest),
        CREDENTIAL_REVOCATION_HASHTAG,
        serde_json::to_string(&revocation).expect("serialize revocation"),
        &fixture.issuer,
    )
}

fn malformed_revocation_event(fixture: &Fixture) -> Event {
    let revocation = fixture
        .issuer
        .revoke_credential(&fixture.envelope.signed_credential)
        .expect("revoke credential");
    let digest = credential_digest_wire_string(&revocation.revocation.credential_digest)
        .expect("serialize digest");
    issuer_event(
        CREDENTIAL_REVOCATION_EVENT_KIND,
        &credential_revocation_d_tag(&digest),
        CREDENTIAL_REVOCATION_HASHTAG,
        "{}".to_owned(),
        &fixture.issuer,
    )
}

#[tokio::test]
async fn empty_revocation_result_verifies_complete_envelope_and_returns_typed_subject() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, source);

    let verified = verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("valid envelope verifies");

    assert_eq!(
        verified.issuer(),
        &fixture.authority.issuer.issuer_id_pubkey
    );
    assert_eq!(
        verified.subject(),
        &fixture
            .envelope
            .holder_authorization
            .authorization
            .subject_pubkey
    );
    assert_eq!(verified.badge().trust_level, 9);
}

#[tokio::test]
async fn rejects_authentic_badge_below_minimum_trust_level() {
    let fixture = fixture_with_trust_level(8);
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, source);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::InsufficientTrustLevel(error))
            if error.minimum() == 9 && error.actual() == 8
    ));
}

#[tokio::test]
async fn accepts_authentic_badge_above_minimum_trust_level() {
    let fixture = fixture_with_trust_level(10);
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, source);

    let verified = verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("badge above the configured minimum verifies");
    assert_eq!(verified.badge().trust_level, 10);
}

#[tokio::test]
async fn refetches_and_uses_current_authority_on_every_verification() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));

    verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("first verification");
    source.authorities.lock().expect("authority lock").clear();
    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::MissingAuthority)
    ));

    assert_eq!(source.authority_calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.revocation_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refetches_and_uses_current_revocation_on_every_verification() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));

    verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("first verification");
    source
        .revocations
        .lock()
        .expect("revocation lock")
        .push(signed_revocation_event(&fixture));

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::CredentialRevoked)
    ));
    assert_eq!(source.authority_calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.revocation_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recognizes_valid_revocation_after_malformed_candidate() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));
    source.revocations.lock().expect("revocation lock").extend([
        malformed_revocation_event(&fixture),
        signed_revocation_event(&fixture),
    ]);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::CredentialRevoked)
    ));
}

#[tokio::test]
async fn recognizes_valid_revocation_before_malformed_candidate() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));
    source.revocations.lock().expect("revocation lock").extend([
        signed_revocation_event(&fixture),
        malformed_revocation_event(&fixture),
    ]);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::CredentialRevoked)
    ));
}

#[tokio::test]
async fn rejects_nonempty_revocation_result_without_valid_match() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));
    source
        .revocations
        .lock()
        .expect("revocation lock")
        .push(malformed_revocation_event(&fixture));

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::InvalidRevocation)
    ));
}

#[tokio::test]
async fn rejects_invalid_newest_authority_instead_of_falling_back() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));
    source
        .authorities
        .lock()
        .expect("authority lock")
        .push(issuer_event_at(
            ISSUER_AUTHORITY_EVENT_KIND,
            ISSUER_AUTHORITY_D_TAG,
            ISSUER_AUTHORITY_HASHTAG,
            "{}".to_owned(),
            &fixture.issuer,
            2_000,
        ));

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::InvalidAuthority)
    ));
    assert_eq!(source.revocation_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_untrusted_issuer_before_any_network_lookup() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    let verifier = PeerBadgeVerifier::with_source(
        PeerBadgeIssuerRoots::new([Keys::generate().public_key()]).expect("other root"),
        Arc::clone(&source) as Arc<dyn PeerBadgeEventSource>,
        &ManifoldEnvironment::Development.profile().unwrap(),
    );

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::UntrustedIssuer { .. })
    ));
    assert_eq!(source.authority_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fails_closed_when_revocation_lookup_is_unavailable() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    source.revocation_unavailable.store(true, Ordering::SeqCst);
    let verifier = verifier(&fixture, source);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::RevocationUnavailable(_))
    ));
}

#[tokio::test]
async fn fails_closed_when_authority_has_no_nostr_revocation_location() {
    let mut fixture = fixture();
    fixture.authority = fixture
        .issuer
        .issuer_authority(Vec::new())
        .expect("issue authority without revocation location");
    fixture.authority_event = issuer_event(
        ISSUER_AUTHORITY_EVENT_KIND,
        ISSUER_AUTHORITY_D_TAG,
        ISSUER_AUTHORITY_HASHTAG,
        serde_json::to_string(&fixture.authority).expect("serialize authority"),
        &fixture.issuer,
    );
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, source);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::MissingRevocationLocation)
    ));
}

#[tokio::test]
async fn rejects_authority_exceeding_revocation_location_bound() {
    let mut fixture = fixture();
    fixture.authority = fixture
        .issuer
        .issuer_authority(
            (0..=MAX_REVOCATION_LOCATIONS)
                .map(|index| RevocationLocation {
                    protocol: "nostr".to_owned(),
                    location: format!("wss://revocations-{index}.example"),
                })
                .collect(),
        )
        .expect("issue authority with many revocation locations");
    fixture.authority_event = issuer_event(
        ISSUER_AUTHORITY_EVENT_KIND,
        ISSUER_AUTHORITY_D_TAG,
        ISSUER_AUTHORITY_HASHTAG,
        serde_json::to_string(&fixture.authority).expect("serialize authority"),
        &fixture.issuer,
    );
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::InvalidAuthority)
    ));
    assert_eq!(source.revocation_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepts_authority_at_revocation_location_bound() {
    let mut fixture = fixture();
    fixture.authority = fixture
        .issuer
        .issuer_authority(
            (0..MAX_REVOCATION_LOCATIONS)
                .map(|index| RevocationLocation {
                    protocol: "nostr".to_owned(),
                    location: format!("wss://revocations-{index}.example"),
                })
                .collect(),
        )
        .expect("issue authority at revocation location bound");
    fixture.authority_event = issuer_event(
        ISSUER_AUTHORITY_EVENT_KIND,
        ISSUER_AUTHORITY_D_TAG,
        ISSUER_AUTHORITY_HASHTAG,
        serde_json::to_string(&fixture.authority).expect("serialize authority"),
        &fixture.issuer,
    );
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, Arc::clone(&source));

    verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("authority at the location bound verifies");
    assert_eq!(
        source.last_revocation_relay_count.load(Ordering::SeqCst),
        MAX_REVOCATION_LOCATIONS
    );
}

#[test]
fn rejects_authority_relay_configuration_over_bound() {
    let error = PeerBadgeAuthorityRelays::new(
        (0..=MAX_AUTHORITY_RELAYS).map(|index| relay(&format!("wss://authority-{index}.example"))),
    )
    .expect_err("too many authority relays fail configuration");

    assert!(matches!(
        error,
        PeerBadgeVerifierConfigError::TooManyAuthorityRelays { .. }
    ));
}

#[test]
fn accepts_authority_relay_configuration_at_bound() {
    let relays = PeerBadgeAuthorityRelays::new(
        (0..MAX_AUTHORITY_RELAYS).map(|index| relay(&format!("wss://authority-{index}.example"))),
    )
    .expect("relay configuration at bound is valid");

    assert_eq!(relays.as_urls().len(), MAX_AUTHORITY_RELAYS);
}

#[test]
fn environment_construction_accepts_every_environment() {
    for environment in [
        ManifoldEnvironment::Development,
        ManifoldEnvironment::Staging,
        ManifoldEnvironment::Production,
    ] {
        let profile = environment.profile().unwrap();
        let verifier = PeerBadgeVerifier::try_from_profile(&profile)
            .expect("every environment has configured issuer identities");
        assert_eq!(
            verifier.provenance(),
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment,
                profile_revision: profile.profile_revision(),
            }
        );
    }
}

#[cfg(feature = "test-support")]
#[test]
fn explicit_test_configuration_never_reports_canonical_profile_provenance() {
    let profile = ManifoldEnvironment::Development
        .profile()
        .expect("development profile is configured");
    let verifier = PeerBadgeVerifier::new_for_test(
        profile.peer_badge_issuer_identities().iter().copied(),
        profile.nostr_relays().as_urls().iter().cloned(),
        profile.minimum_peer_badge_trust_level(),
    )
    .expect("development fixtures form a valid explicit test verifier");

    assert_eq!(
        verifier.provenance(),
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration
    );
}

#[cfg(feature = "test-support")]
#[test]
fn explicit_test_configuration_rejects_minimum_outside_schema_range() {
    let profile = ManifoldEnvironment::Development
        .profile()
        .expect("development profile is configured");
    let error = PeerBadgeVerifier::new_for_test(
        profile.peer_badge_issuer_identities().iter().copied(),
        profile.nostr_relays().as_urls().iter().cloned(),
        TRUST_SCORE_LEVEL_MAX + 1,
    )
    .err()
    .expect("out-of-range minimum must be rejected");

    assert!(matches!(
        error,
        PeerBadgeVerifierConfigError::InvalidMinimumTrustLevel(error)
            if error.minimum() == TRUST_SCORE_LEVEL_MIN
                && error.maximum() == TRUST_SCORE_LEVEL_MAX
                && error.actual() == TRUST_SCORE_LEVEL_MAX + 1
    ));
}

#[test]
fn production_verifier_uses_no_known_secret_placeholder_root() {
    let production = ManifoldEnvironment::Production.profile().unwrap();
    let verifier = PeerBadgeVerifier::try_from_profile(&production)
        .expect("production has configured issuer identities");

    for environment in [
        ManifoldEnvironment::Development,
        ManifoldEnvironment::Staging,
    ] {
        let placeholders = environment.profile().unwrap();
        for placeholder in placeholders.peer_badge_issuer_identities() {
            assert!(
                !verifier.inner.issuer_roots.contains(placeholder),
                "production must not trust the {environment} known-secret issuer root"
            );
        }
    }
}

#[test]
fn an_issuerless_profile_leaves_the_verifier_unavailable() {
    let profile = ManifoldEnvironment::Production.profile().unwrap();
    let error = match PeerBadgeVerifier::try_from_profile_parts(
        profile.environment(),
        profile.profile_revision(),
        [],
        profile.nostr_relays().as_urls().iter().cloned(),
        profile.minimum_peer_badge_trust_level(),
        profile.pinned_issuer_authorities().iter().copied(),
    ) {
        Ok(_) => panic!("a profile without issuer roots must fail construction"),
        Err(error) => error,
    };
    assert!(matches!(
        &error,
        PeerBadgeVerifierConfigError::EnvironmentIssuerRootsUnavailable {
            environment: ManifoldEnvironment::Production
        }
    ));
    assert_eq!(
        error.to_string(),
        "production PeerBadge issuer identities are not configured"
    );
}

#[tokio::test]
async fn rejects_tampered_holder_authorization() {
    let mut fixture = fixture();
    fixture
        .envelope
        .holder_authorization
        .authorization
        .subject_pubkey = SubjectPubkey(Keys::generate().public_key());
    let source = Arc::new(FakeSource::default());
    let verifier = verifier(&fixture, source);

    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::InvalidEnvelope(_))
    ));
}

fn pinned_verifier(fixture: &Fixture, source: Arc<FakeSource>) -> PeerBadgeVerifier {
    let document =
        serde_json::to_string(&fixture.authority).expect("serialize fixture authority document");
    let roots = PeerBadgeIssuerRoots::new([fixture.authority.issuer.issuer_id_pubkey.0])
        .expect("trusted issuer root");
    let (identity, pinned) =
        pin_committed_authority(&document, &roots).expect("pin committed authority document");
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    PeerBadgeVerifier::with_source_and_provenance(
        roots,
        PeerBadgeTrustPolicy::try_new(profile.minimum_peer_badge_trust_level())
            .expect("canonical profile minimum trust level is valid"),
        BTreeMap::from([(identity, pinned)]),
        source,
        PeerBadgeVerifierProvenance::ManifoldProfile {
            environment: profile.environment(),
            profile_revision: profile.profile_revision(),
        },
    )
}

#[tokio::test]
async fn pinned_issuer_verifies_without_any_authority_lookup() {
    let fixture = fixture();
    // Deliberately no authority event in the fake source: a pinned issuer
    // must never look one up, so a relay overwrite (or outage) is irrelevant.
    let source = Arc::new(FakeSource::default());
    let verifier = pinned_verifier(&fixture, source.clone());
    let verified = verifier
        .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
        .await
        .expect("pinned issuer verifies");
    assert_eq!(verified.badge().trust_level, 9);
    assert_eq!(source.authority_calls.load(Ordering::SeqCst), 0);
    // Revocation stays enforced, routed by the pinned authority's locations.
    assert_eq!(source.revocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(source.last_revocation_relay_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pinned_issuer_revocation_still_rejects() {
    let fixture = fixture();
    let source = Arc::new(FakeSource::default());
    source
        .revocations
        .lock()
        .expect("revocation lock")
        .push(signed_revocation_event(&fixture));
    let verifier = pinned_verifier(&fixture, source.clone());
    assert!(matches!(
        verifier
            .verify_at(&fixture.envelope, VERIFY_AT, verification_deadline())
            .await,
        Err(PeerBadgeVerificationError::CredentialRevoked)
    ));
    assert_eq!(source.authority_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn committed_authority_outside_identity_roots_is_rejected() {
    let fixture = fixture();
    let document =
        serde_json::to_string(&fixture.authority).expect("serialize fixture authority document");
    let unrelated_root =
        PeerBadgeIssuerRoots::new([Keys::generate().public_key()]).expect("unrelated root");
    assert!(matches!(
        pin_committed_authority(&document, &unrelated_root),
        Err(PeerBadgeVerifierConfigError::InvalidCommittedAuthority { .. })
    ));
}

#[test]
fn tampered_committed_authority_is_rejected() {
    let fixture = fixture();
    let mut tampered: serde_json::Value =
        serde_json::to_value(&fixture.authority).expect("authority to JSON");
    tampered["issuer"]["revocation"] = serde_json::json!([
        { "protocol": "nostr", "location": "wss://attacker.example" }
    ]);
    let roots = PeerBadgeIssuerRoots::new([fixture.authority.issuer.issuer_id_pubkey.0])
        .expect("trusted issuer root");
    assert!(matches!(
        pin_committed_authority(&tampered.to_string(), &roots),
        Err(PeerBadgeVerifierConfigError::InvalidCommittedAuthority { .. })
    ));
}

#[test]
fn every_canonical_profile_pins_each_configured_issuer() {
    for environment in [
        ManifoldEnvironment::Development,
        ManifoldEnvironment::Staging,
        ManifoldEnvironment::Production,
    ] {
        let profile = environment.profile().unwrap();
        let verifier = PeerBadgeVerifier::try_from_profile(&profile)
            .unwrap_or_else(|error| panic!("{environment} verifier must construct: {error}"));
        assert_eq!(
            verifier.inner.pinned_authorities.len(),
            profile.peer_badge_issuer_identities().len(),
            "{environment} must pin one authority per issuer root",
        );
        for issuer in profile.peer_badge_issuer_identities() {
            assert!(verifier.inner.pinned_authorities.contains_key(issuer));
        }
    }
}
