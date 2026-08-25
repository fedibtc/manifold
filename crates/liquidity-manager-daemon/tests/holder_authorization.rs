use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use fedi_credential_sdk_protocol::HolderContext;

use super::*;
use crate::test_support::credentials::{
    UNIT_TEST_ISSUER_RELAY, UNIT_TEST_PEER_BADGE_TRUST_LEVEL, flip_authorization_event,
    holder_authorization_for_provider, holder_nostr_keys, issue_credential_for_holder,
    issue_credential_for_holder_with_trust_level, service_credential, service_holder_authorization,
    test_issuer_authority, test_issuer_context,
};

/// A holder, its badge, and the event authorizing one provider.
struct Enrollment {
    event: Event,
    envelope: HolderAuthorizationEnvelope,
}

fn enroll(provider_pubkey: &Pubkey) -> anyhow::Result<Enrollment> {
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = holder_authorization_for_provider(&holder, &credential, provider_pubkey)?;
    Ok(Enrollment {
        event: flip_authorization_event(&holder, &authorization, &credential, provider_pubkey)?,
        envelope: HolderAuthorizationEnvelope {
            holder_authorization: service_holder_authorization(&authorization)?,
            signed_credential: service_credential(&credential)?,
        },
    })
}

#[derive(Default)]
struct FakeFetcher {
    /// Answers keyed by relay URL; a missing key fails that relay.
    answers: Mutex<Vec<(String, Result<Vec<Event>, String>)>>,
}

#[async_trait]
impl HolderAuthorizationFetcher for FakeFetcher {
    async fn fetch_candidates(
        &self,
        relay_url: &Url,
        _provider_pubkey: PublicKey,
    ) -> Result<Vec<Event>, String> {
        self.answers
            .lock()
            .expect("fake fetcher lock")
            .iter()
            .find(|(url, _)| url == &relay_url.0)
            .map(|(_, answer)| answer.clone())
            .unwrap_or_else(|| Err("no answer configured".to_owned()))
    }
}

#[tokio::test]
async fn accepts_a_well_formed_authorization_for_this_provider() -> anyhow::Result<()> {
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;

    let verified = verify_candidate(&enrollment.event, &parse_provider_pubkey(&provider_pubkey)?)
        .expect("a well-formed authorization for this provider verifies");

    assert_eq!(verified.envelope, enrollment.envelope);
    assert_eq!(verified.credential_digest.len(), 32);
    Ok(())
}

#[tokio::test]
async fn rejects_an_authorization_naming_another_provider() -> anyhow::Result<()> {
    let other_provider = generate_provider_pubkey();
    let enrollment = enroll(&other_provider)?;
    let our_pubkey = generate_provider_pubkey();

    let error = verify_candidate(&enrollment.event, &parse_provider_pubkey(&our_pubkey)?)
        .expect_err("an authorization naming another provider must not verify");

    assert!(
        error.to_string().contains("subject is not this provider"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn rejects_an_authorization_republished_by_a_stranger() -> anyhow::Result<()> {
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;

    // A relay hands back the holder's content re-signed by another key.
    // The statement still verifies under the holder, so only the
    // author-binding check refuses it.
    let stranger = nostr_sdk::Keys::generate();
    let republished = nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
        enrollment.event.content.clone(),
    )
    .sign_with_keys(&stranger)?;

    let error = verify_candidate(&republished, &parse_provider_pubkey(&provider_pubkey)?)
        .expect_err("a re-signed authorization must not verify");

    assert!(
        error.to_string().contains("does not match event author"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn rejects_a_different_badge_swapped_under_a_signed_authorization() -> anyhow::Result<()> {
    let provider_pubkey = generate_provider_pubkey();
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = holder_authorization_for_provider(&holder, &credential, &provider_pubkey)?;
    // A higher-scoring badge for the same holder, substituted under an
    // authorization that names the first one's digest. This is the upgrade
    // a hostile republisher would want.
    let better_credential = issue_credential_for_holder_with_trust_level(
        &issuer,
        &authority,
        &holder,
        UNIT_TEST_PEER_BADGE_TRUST_LEVEL + 1,
    )?;

    let swapped = holder_signed_event(&holder, &authorization, &better_credential)?;

    let error = verify_candidate(&swapped, &parse_provider_pubkey(&provider_pubkey)?)
        .expect_err("a different badge must not verify under this authorization");

    assert!(
        error
            .to_string()
            .contains("credential digest does not match"),
        "unexpected error: {error}"
    );
    Ok(())
}

/// A badge bound to another holder is refused.
///
/// This exercises the holder-binding check directly: the statement is
/// genuinely signed by its author and names this provider, and only the
/// attached credential's revealed holder disagrees.
///
/// The sharper form of the attack — signing a statement over the victim's
/// *digest* so the malicious entry lands in the victim's retained slot —
/// cannot be built here, because the SDK's own signing path refuses to
/// authorize a credential bound to someone else (asserted below). That
/// refusal runs on the signer's machine, so it protects nobody against a
/// client that simply does not run it; this check is the verifier-side
/// enforcement that does not depend on it.
#[tokio::test]
async fn rejects_a_badge_bound_to_another_holder() -> anyhow::Result<()> {
    let database = test_database("holder-binding").await?;
    let provider_pubkey = generate_provider_pubkey();
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;

    let victim = HolderContext::generate();
    let victim_credential = issue_credential_for_holder(&issuer, &authority, &victim)?;
    let victim_authorization =
        holder_authorization_for_provider(&victim, &victim_credential, &provider_pubkey)?;
    let victim_event = flip_authorization_event(
        &victim,
        &victim_authorization,
        &victim_credential,
        &provider_pubkey,
    )?;

    let attacker = HolderContext::generate();
    let attacker_credential = issue_credential_for_holder(&issuer, &authority, &attacker)?;
    let attacker_authorization =
        holder_authorization_for_provider(&attacker, &attacker_credential, &provider_pubkey)?;
    // The attacker's own signed statement, carrying the victim's badge.
    let swapped = holder_signed_event(&attacker, &attacker_authorization, &victim_credential)?;

    let error = verify_candidate(&swapped, &parse_provider_pubkey(&provider_pubkey)?)
        .expect_err("a badge bound to another holder must not verify");
    assert!(
        error
            .to_string()
            .contains("credential holder binding does not match"),
        "unexpected error: {error}"
    );

    // The SDK will not sign the same-slot variant, so a conforming client
    // cannot reach for it in the first place.
    let refused =
        holder_authorization_for_provider(&attacker, &victim_credential, &provider_pubkey);
    assert!(
        refused.is_err(),
        "the SDK signs an authorization only for the signer's own badge"
    );

    // End to end: the relay serves both, and only the victim's enrols.
    let relay = Url("wss://relay.example".to_owned());
    let fetcher = FakeFetcher {
        answers: Mutex::new(vec![(relay.0.clone(), Ok(vec![victim_event, swapped]))]),
    };
    let outcome = refresh(&database, &fetcher, &provider_pubkey, &[relay]).await?;
    assert_eq!(outcome.candidates_seen, 2);
    assert_eq!(outcome.candidates_verified, 1);

    let enrolled = load_verified(&database, &provider_pubkey).await?;
    assert_eq!(enrolled.len(), 1);
    assert_eq!(
        enrolled[0].envelope.holder_authorization, victim_authorization,
        "the victim's authorization keeps its slot"
    );
    Ok(())
}

/// Pins a known limit rather than a guarantee.
///
/// `Credential::digest()` covers `info` and the revealed `blind_msg`, not
/// the issuer proof bytes. Two issuances of one badge to one holder are
/// therefore the same payload under a different signature, and the
/// authorization cannot tell them apart. Nothing here is a trust
/// conclusion: whichever variant is admitted still has to pass the issuer
/// proof in [`valid_provider_trust_envelopes`] before it is published.
#[tokio::test]
async fn admits_either_issuance_of_one_badge_payload() -> anyhow::Result<()> {
    let provider_pubkey = generate_provider_pubkey();
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = holder_authorization_for_provider(&holder, &credential, &provider_pubkey)?;
    let reissued = issue_credential_for_holder(&issuer, &authority, &holder)?;
    assert_ne!(
        credential.proof.signature.0, reissued.proof.signature.0,
        "blind issuance produces a fresh signature each time"
    );

    let substituted = holder_signed_event(&holder, &authorization, &reissued)?;

    verify_candidate(&substituted, &parse_provider_pubkey(&provider_pubkey)?)
        .expect("a re-issued credential carries the same payload digest");
    Ok(())
}

/// Pins the wire-version gate, which the admission argument leans on to
/// say a candidate is the version-1 content shape and not something a
/// later revision may reinterpret.
#[tokio::test]
async fn rejects_content_carrying_an_unsupported_wire_version() -> anyhow::Result<()> {
    let provider_pubkey = generate_provider_pubkey();
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = holder_authorization_for_provider(&holder, &credential, &provider_pubkey)?;

    let content = serde_json_canonicalizer::to_string(&serde_json::json!({
        "version": 2,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;
    let future_version = nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
        content,
    )
    .sign_with_keys(&holder_nostr_keys(&holder)?)?;

    let error = verify_candidate(&future_version, &parse_provider_pubkey(&provider_pubkey)?)
        .expect_err("an unsupported wire version must not verify");

    assert!(
        error
            .to_string()
            .contains("unparsable authorization envelope"),
        "unexpected error: {error}"
    );
    Ok(())
}

fn holder_signed_event(
    holder: &HolderContext,
    authorization: &fedi_credential_sdk_protocol::HolderAuthorization,
    credential: &fedi_credential_sdk_protocol::SignedCredential,
) -> anyhow::Result<Event> {
    let content = serde_json_canonicalizer::to_string(&serde_json::json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;
    Ok(nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
        content,
    )
    .sign_with_keys(&holder_nostr_keys(holder)?)?)
}

#[tokio::test]
async fn merge_replaces_only_on_a_strictly_greater_issued_at() -> anyhow::Result<()> {
    let database = test_database("merge-monotonic").await?;
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;
    let digest = verify_candidate(&enrollment.event, &parse_provider_pubkey(&provider_pubkey)?)?
        .credential_digest;

    let at = |issued_at: u64, marker: &str| VerifiedHolderAuthorization {
        envelope: enrollment.envelope.clone(),
        credential_digest: digest.clone(),
        authorization_issued_at: issued_at,
        event_json: marker.to_owned(),
    };

    merge(&database, &[at(100, "first")]).await?;
    assert_eq!(stored_event_json(&database).await?, "first");

    merge(&database, &[at(50, "older-replay")]).await?;
    assert_eq!(
        stored_event_json(&database).await?,
        "first",
        "an older replay must not displace an enrolled authorization"
    );

    merge(&database, &[at(100, "equal-replay")]).await?;
    assert_eq!(
        stored_event_json(&database).await?,
        "first",
        "an equal-dated replay must not displace an enrolled authorization"
    );

    merge(&database, &[at(101, "newer")]).await?;
    assert_eq!(stored_event_json(&database).await?, "newer");

    // One credential is one row however many times it is re-enrolled.
    assert_eq!(retained_count(&database).await?, 1);
    Ok(())
}

#[tokio::test]
async fn merge_orders_by_unsigned_value_beyond_the_signed_range() -> anyhow::Result<()> {
    let database = test_database("merge-unsigned").await?;
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;
    let digest = verify_candidate(&enrollment.event, &parse_provider_pubkey(&provider_pubkey)?)?
        .credential_digest;

    let at = |issued_at: u64, marker: &str| VerifiedHolderAuthorization {
        envelope: enrollment.envelope.clone(),
        credential_digest: digest.clone(),
        authorization_issued_at: issued_at,
        event_json: marker.to_owned(),
    };

    // A statement may carry any u64. Stored as a signed integer these two
    // would compare backwards; stored big-endian they do not.
    merge(&database, &[at(u64::MAX, "far-future")]).await?;
    merge(&database, &[at(1, "epoch")]).await?;

    assert_eq!(stored_event_json(&database).await?, "far-future");
    Ok(())
}

#[tokio::test]
async fn refresh_unions_relays_and_reports_the_ones_that_failed() -> anyhow::Result<()> {
    let database = test_database("refresh-union").await?;
    let provider_pubkey = generate_provider_pubkey();
    let first = enroll(&provider_pubkey)?;
    let second = enroll(&provider_pubkey)?;

    let good = Url("wss://good.example".to_owned());
    let partial = Url("wss://partial.example".to_owned());
    let broken = Url("wss://broken.example".to_owned());
    let fetcher = FakeFetcher {
        answers: Mutex::new(vec![
            (good.0.clone(), Ok(vec![first.event.clone()])),
            (partial.0.clone(), Ok(vec![second.event.clone()])),
            (broken.0.clone(), Err("relay unreachable".to_owned())),
        ]),
    };

    let outcome = refresh(
        &database,
        &fetcher,
        &provider_pubkey,
        &[good, partial, broken.clone()],
    )
    .await?;

    assert_eq!(outcome.relays_answered, 2);
    assert_eq!(outcome.candidates_verified, 2);
    assert_eq!(
        outcome.retained, 2,
        "each relay contributed a distinct badge"
    );
    assert_eq!(outcome.relays_failed.len(), 1);
    assert_eq!(outcome.relays_failed[0].0, broken);
    Ok(())
}

#[tokio::test]
async fn refresh_admits_nothing_from_a_relay_serving_junk() -> anyhow::Result<()> {
    let database = test_database("refresh-junk").await?;
    let provider_pubkey = generate_provider_pubkey();
    let for_someone_else = enroll(&generate_provider_pubkey())?;

    let hostile = Url("wss://hostile.example".to_owned());
    let junk = nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
        "not an authorization",
    )
    .sign_with_keys(&nostr_sdk::Keys::generate())?;
    let fetcher = FakeFetcher {
        answers: Mutex::new(vec![(
            hostile.0.clone(),
            Ok(vec![junk, for_someone_else.event]),
        )]),
    };

    let outcome = refresh(&database, &fetcher, &provider_pubkey, &[hostile]).await?;

    assert_eq!(outcome.candidates_seen, 2);
    assert_eq!(outcome.candidates_verified, 0);
    assert_eq!(outcome.retained, 0);
    assert!(load_verified(&database, &provider_pubkey).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn load_verified_drops_rows_that_no_longer_bind_to_the_provider() -> anyhow::Result<()> {
    let database = test_database("load-rebind").await?;
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;
    let relay = Url("wss://relay.example".to_owned());
    let fetcher = FakeFetcher {
        answers: Mutex::new(vec![(relay.0.clone(), Ok(vec![enrollment.event]))]),
    };
    refresh(&database, &fetcher, &provider_pubkey, &[relay]).await?;
    assert_eq!(load_verified(&database, &provider_pubkey).await?.len(), 1);

    // The same durable row read under a different provider identity: the
    // retained event is re-verified on every read, so it cannot carry over.
    let other_provider = generate_provider_pubkey();
    assert!(
        load_verified(&database, &other_provider).await?.is_empty(),
        "a retained row must not verify under another provider key"
    );
    Ok(())
}

/// An enrolled authorization outranks whatever the last read concluded.
///
/// The rows are durable and re-verified before every use, and the
/// advertisement still carries the envelope, so a relay outage must not
/// make the provider look unauthorized to its own operator.
#[tokio::test]
async fn an_enrolled_authorization_survives_a_failed_read() -> anyhow::Result<()> {
    let database = test_database("status-ranking").await?;
    let provider_pubkey = generate_provider_pubkey();
    let enrollment = enroll(&provider_pubkey)?;
    let relay = Url("wss://relay.example".to_owned());

    // Nothing read yet is `checking`, not "no Holder has authorized".
    assert_eq!(
        status(&database, &provider_pubkey, &LastRelayRead::NotYet).await?,
        HolderAuthorizationStatus::Checking
    );

    // A completed read that found nothing says so, with the time.
    let read_at = Timestamp(1_700_000_000);
    assert_eq!(
        status(
            &database,
            &provider_pubkey,
            &LastRelayRead::Completed(read_at)
        )
        .await?,
        HolderAuthorizationStatus::NotObserved {
            read_completed_at: read_at
        }
    );

    let fetcher = FakeFetcher {
        answers: Mutex::new(vec![(relay.0.clone(), Ok(vec![enrollment.event]))]),
    };
    refresh(&database, &fetcher, &provider_pubkey, &[relay]).await?;

    // Now every read state reports the enrolled authorization.
    for last_read in [
        LastRelayRead::NotYet,
        LastRelayRead::Completed(read_at),
        LastRelayRead::Failed {
            at: read_at,
            reason: "relay unreachable".to_owned(),
        },
    ] {
        let observed = status(&database, &provider_pubkey, &last_read).await?;
        assert!(
            matches!(
                observed,
                HolderAuthorizationStatus::AuthorizationObserved {
                    authorizations: 1,
                    ..
                }
            ),
            "a {last_read:?} read must not demote an enrolled provider: {observed:?}"
        );
    }
    Ok(())
}

#[test]
fn a_read_that_no_relay_answered_is_an_error_rather_than_an_empty_answer() {
    let now = Timestamp(1_700_000_000);
    let answered = RefreshOutcome {
        relays_answered: 1,
        ..RefreshOutcome::default()
    };
    assert_eq!(
        LastRelayRead::from_outcome(&answered, now),
        LastRelayRead::Completed(now)
    );

    let all_failed = RefreshOutcome {
        relays_answered: 0,
        relays_failed: vec![(Url("wss://down.example".to_owned()), "gone".to_owned())],
        ..RefreshOutcome::default()
    };
    assert_eq!(
        LastRelayRead::from_outcome(&all_failed, now),
        LastRelayRead::Failed {
            at: now,
            reason: "gone".to_owned()
        }
    );

    // A partial answer is a completed read: one relay serving the
    // authorization is enough, and reporting an error would hide that.
    let partial = RefreshOutcome {
        relays_answered: 1,
        relays_failed: vec![(Url("wss://down.example".to_owned()), "gone".to_owned())],
        ..RefreshOutcome::default()
    };
    assert_eq!(
        LastRelayRead::from_outcome(&partial, now),
        LastRelayRead::Completed(now)
    );
}

async fn stored_event_json(database: &Database) -> anyhow::Result<String> {
    Ok(
        sqlx::query_scalar("SELECT event_json FROM holder_authorization_events")
            .fetch_one(database.pool())
            .await?,
    )
}

fn generate_provider_pubkey() -> Pubkey {
    Pubkey(nostr_sdk::Keys::generate().public_key().to_hex())
}

async fn test_database(name: &str) -> anyhow::Result<Database> {
    Database::connect(test_data_dir(name).join("flip.sqlite")).await
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("flip-holder-authorization-{name}-{nanos}"))
}
