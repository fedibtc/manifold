//! Tests for the shared trust-score schema and holder trust envelope.

use std::sync::LazyLock;

use fedi_credential_sdk_protocol::{
    CredentialsError, HolderAuthorizationRequest, HolderContext, IssuerAuthority, IssuerContext,
    IssuerSecretKeys, PendingIssuance, SignedCredential, VerificationContext,
};
use serde_json::{Value, json};

use super::*;

const AUTHORIZED_AT: u64 = 1_000;
const VERIFY_AT: Timestamp = Timestamp(2_000);

/// Import fixed test-only keys because nextest runs each test in a separate
/// process, so generating RSA keys in a `LazyLock` would still happen once per
/// test.
static ISSUER_SECRET_KEYS: LazyLock<IssuerSecretKeys> = LazyLock::new(|| {
    serde_json::from_str(include_str!("issuer-secret-keys.json"))
        .expect("fixed test issuer keys deserialize")
});

struct Fixture {
    issuer: IssuerContext,
    authority: IssuerAuthority,
    holder: HolderContext,
    subject: Pubkey,
    verifier: VerificationContext,
}

fn fixture() -> Fixture {
    let issuer = IssuerContext::import_secret_key(&ISSUER_SECRET_KEYS).expect("import issuer");
    let authority = issuer
        .issuer_authority(vec![fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "nostr".to_owned(),
            location: "wss://relay.example".to_owned(),
        }])
        .expect("issuer authority");
    let holder = HolderContext::generate();
    let subject = Pubkey(nostr::Keys::generate().public_key().to_string());
    let mut verifier = VerificationContext::new();
    verifier
        .add_issuer_authority(&authority)
        .expect("trust issuer authority");
    Fixture {
        issuer,
        authority,
        holder,
        subject,
        verifier,
    }
}

fn badge_info() -> Value {
    trust_score_info_v1(6).expect("legal trust level")
}

fn issue_credential(fixture: &Fixture, info: Value, blind_msg: Value) -> SignedCredential {
    let (request, pending) = PendingIssuance::create_request(
        &fixture.authority.issuer.issuance_key,
        fixture.authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        blind_msg,
    )
    .expect("create issuance request");
    let response = fixture
        .issuer
        .issue_credential(info, &request)
        .expect("issue credential");
    pending
        .finalize(&fixture.authority.issuer.issuance_key, &response)
        .expect("finalize credential")
}

fn issue_holder_badge(fixture: &Fixture, info: Value) -> SignedCredential {
    issue_credential(
        fixture,
        info,
        json!(fixture.holder.public_key().to_string()),
    )
}

fn envelope_for(fixture: &Fixture, credential: SignedCredential) -> HolderAuthorizationEnvelope {
    let holder_authorization = fixture
        .holder
        .authorize_credential_use_at_time(
            HolderAuthorizationRequest {
                subject_pubkey: fixture.subject.0.parse().expect("subject pubkey"),
            },
            &credential,
            AUTHORIZED_AT,
        )
        .expect("authorize credential use");
    HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential: credential,
    }
}

#[test]
fn valid_envelope_verifies_and_returns_badge() {
    let fixture = fixture();
    let envelope = envelope_for(&fixture, issue_holder_badge(&fixture, badge_info()));

    let badge =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect("valid envelope verifies");

    assert_eq!(badge.trust_level, 6);
    assert_eq!(badge.holder_pubkey, fixture.holder.public_key());
}

#[test]
fn peer_badge_trust_policy_covers_below_at_and_above_minimum() {
    let policy = PeerBadgeTrustPolicy::try_new(9).expect("schema-valid policy");
    let holder_pubkey = fixture().holder.public_key();

    let below = TrustScoreBadgeV1 {
        trust_level: 8,
        holder_pubkey,
    };
    assert_eq!(
        policy.require(&below),
        Err(PeerBadgeTrustPolicyError {
            minimum: 9,
            actual: 8,
        })
    );

    let at = TrustScoreBadgeV1 {
        trust_level: 9,
        holder_pubkey,
    };
    policy.require(&at).expect("minimum is admitted");

    let above = TrustScoreBadgeV1 {
        trust_level: 10,
        holder_pubkey,
    };
    policy.require(&above).expect("above minimum is admitted");
}

#[test]
fn peer_badge_trust_policy_rejects_out_of_schema_minimum() {
    let error = PeerBadgeTrustPolicy::try_new(TRUST_SCORE_LEVEL_MAX + 1)
        .expect_err("out-of-schema minimum is rejected");

    assert_eq!(error.minimum(), TRUST_SCORE_LEVEL_MIN);
    assert_eq!(error.maximum(), TRUST_SCORE_LEVEL_MAX);
    assert_eq!(error.actual(), TRUST_SCORE_LEVEL_MAX + 1);
}

#[test]
fn subject_mismatch_is_rejected() {
    let fixture = fixture();
    let envelope = envelope_for(&fixture, issue_holder_badge(&fixture, badge_info()));
    let other_subject = Pubkey(nostr::Keys::generate().public_key().to_string());

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &other_subject, VERIFY_AT)
            .expect_err("subject mismatch is rejected");

    assert!(matches!(error, HolderTrustEnvelopeError::SubjectMismatch));
}

#[test]
fn credential_digest_mismatch_is_rejected() {
    let fixture = fixture();
    let authorized_credential = issue_holder_badge(&fixture, badge_info());
    let other_credential =
        issue_holder_badge(&fixture, trust_score_info_v1(9).expect("legal trust level"));
    let mut envelope = envelope_for(&fixture, authorized_credential);
    envelope.signed_credential = other_credential;

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("digest mismatch is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Credential(
            CredentialsError::AuthorizationCredentialDigestMismatch
        )
    ));
}

#[test]
fn holder_mismatch_is_rejected() {
    let fixture = fixture();
    let credential = issue_holder_badge(&fixture, badge_info());
    let other_holder = HolderContext::generate();
    let other_credential = issue_credential(
        &fixture,
        badge_info(),
        json!(other_holder.public_key().to_string()),
    );
    let holder_authorization = other_holder
        .authorize_credential_use_at_time(
            HolderAuthorizationRequest {
                subject_pubkey: fixture.subject.0.parse().expect("subject pubkey"),
            },
            &other_credential,
            AUTHORIZED_AT,
        )
        .expect("authorize own credential with other holder");
    // Pair the other holder's valid authorization with the first holder's
    // credential: the SDK holder-binding check runs before the digest check.
    let envelope = HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential: credential,
    };

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("holder mismatch is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Credential(CredentialsError::HolderIdMismatch)
    ));
}

#[test]
fn revoked_credential_is_rejected() {
    let mut fixture = fixture();
    let credential = issue_holder_badge(&fixture, badge_info());
    let revocation = fixture
        .issuer
        .revoke_credential(&credential)
        .expect("revoke credential");
    fixture
        .verifier
        .add_revocation(&revocation)
        .expect("ingest revocation");
    let envelope = envelope_for(&fixture, credential);

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("revoked credential is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Credential(CredentialsError::CredentialRevoked)
    ));
}

#[test]
fn authorization_not_yet_valid_is_rejected() {
    let fixture = fixture();
    let envelope = envelope_for(&fixture, issue_holder_badge(&fixture, badge_info()));

    let error = verify_holder_trust_envelope(
        &fixture.verifier,
        &envelope,
        &fixture.subject,
        Timestamp(AUTHORIZED_AT - 1),
    )
    .expect_err("future authorization is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Credential(CredentialsError::AuthorizationNotYetValid)
    ));
}

#[test]
fn wrong_schema_id_is_rejected() {
    let fixture = fixture();
    let envelope = envelope_for(
        &fixture,
        issue_holder_badge(
            &fixture,
            json!({
                "schema": "fedi-other-schema-v1.0",
                "trust_level": 6,
            }),
        ),
    );

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("wrong schema is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Schema(TrustScoreSchemaError::WrongSchema)
    ));
}

#[test]
fn legacy_extra_info_keys_are_tolerated() {
    // Pre-adoption verifier drafts emitted subject_type and issuance_epoch;
    // the canonical v1.0 schema neither requires nor rejects them.
    let fixture = fixture();
    let envelope = envelope_for(
        &fixture,
        issue_holder_badge(
            &fixture,
            json!({
                "schema": TRUST_SCORE_SCHEMA_V1,
                "subject_type": "holder",
                "trust_level": 6,
                "issuance_epoch": "2026-Q1",
            }),
        ),
    );

    let badge =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect("legacy extra keys are tolerated");

    assert_eq!(badge.trust_level, 6);
}

#[test]
fn non_integer_trust_level_is_rejected() {
    let fixture = fixture();
    let envelope = envelope_for(
        &fixture,
        issue_holder_badge(
            &fixture,
            json!({
                "schema": TRUST_SCORE_SCHEMA_V1,
                "trust_level": "six",
            }),
        ),
    );

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("non-integer trust_level is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Schema(TrustScoreSchemaError::InvalidTrustLevel)
    ));
}

#[test]
fn out_of_range_trust_level_is_rejected() {
    let fixture = fixture();
    let envelope = envelope_for(
        &fixture,
        issue_holder_badge(
            &fixture,
            json!({
                "schema": TRUST_SCORE_SCHEMA_V1,
                "trust_level": TRUST_SCORE_LEVEL_MAX + 1,
            }),
        ),
    );

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("out-of-range trust_level is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Schema(TrustScoreSchemaError::TrustLevelOutOfRange)
    ));
}

#[test]
fn non_canonical_holder_binding_is_rejected() {
    let fixture = fixture();
    let non_canonical = fixture.holder.public_key().to_string().to_ascii_uppercase();
    let credential = issue_credential(&fixture, badge_info(), json!(non_canonical));
    let envelope = envelope_for(&fixture, credential);

    let error =
        verify_holder_trust_envelope(&fixture.verifier, &envelope, &fixture.subject, VERIFY_AT)
            .expect_err("non-canonical holder binding is rejected");

    assert!(matches!(
        error,
        HolderTrustEnvelopeError::Schema(TrustScoreSchemaError::InvalidHolderBinding)
    ));
}

#[test]
fn envelope_wire_shape_is_stable() {
    let fixture = fixture();
    let envelope = envelope_for(&fixture, issue_holder_badge(&fixture, badge_info()));

    let value = serde_json::to_value(&envelope).expect("serialize envelope");
    let object = value.as_object().expect("envelope is a JSON object");

    assert_eq!(object.len(), 2);
    assert!(object.contains_key("holder_authorization"));
    assert!(object.contains_key("signed_credential"));

    let round_tripped: HolderAuthorizationEnvelope =
        serde_json::from_value(value).expect("envelope round-trips");
    assert_eq!(round_tripped, envelope);
}

/// Golden vector: a badge issued by the shipped PeerBadge app (captured in
/// the credential-sdk repo, public material only). The fixture arrives through
/// the same flake pin as the parser, so the truly independent wire pin lives
/// in credential-sdk's own golden tests; this test adds the local assertions
/// (level, holder binding) and proves the full verify-then-parse chain in this
/// repo accepts a real app-issued badge.
#[test]
fn app_issued_golden_vector_verifies_and_parses() {
    #[derive(serde::Deserialize)]
    struct AppIssuedVector {
        issuer_authority: IssuerAuthority,
        signed_credential: SignedCredential,
        holder_pubkey: String,
    }

    let vector: AppIssuedVector = serde_json::from_str(include_str!(
        "../../../../.nix-deps/credential-sdk/crates/schemas/fixtures/app-issued-vector.json"
    ))
    .expect("app-issued vector deserializes");

    let mut verifier = VerificationContext::new();
    verifier
        .add_issuer_authority(&vector.issuer_authority)
        .expect("authority is accepted");
    verifier
        .verify_credential(&vector.signed_credential)
        .expect("app-issued credential verifies");

    let badge = parse_trust_score_badge_v1(&vector.signed_credential.credential)
        .expect("app-issued badge parses under fedi-trust-score-v1.0");
    assert_eq!(badge.trust_level, 9);
    assert_eq!(badge.holder_pubkey.to_string(), vector.holder_pubkey);
}
