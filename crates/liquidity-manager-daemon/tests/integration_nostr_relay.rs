mod common;
mod test_support;

use common::nostr_relay::{NostrRelayFixture, find_advertisement_event};
use tracing_subscriber::EnvFilter;

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("fedi_decentralized_liquidity_manager_daemon=debug,info")
        }))
        .with_test_writer()
        .try_init();
}

#[tokio::test(flavor = "multi_thread")]
async fn fetches_signed_revocation_from_real_nostr_relay() -> anyhow::Result<()> {
    use fedi_credential_sdk_protocol::{
        CredentialDigest, HolderContext, PendingIssuance, RevocationLocation, VerificationContext,
    };
    use fedi_decentralized_liquidity_manager_daemon::revocation::{
        NostrRevocationFetcher, RevocationFetcher, RevocationLookup, credential_digest_wire_string,
    };
    use fedi_decentralized_nostr::attester::{
        CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG,
        credential_revocation_d_tag,
    };

    init_logging();
    let relay = NostrRelayFixture::start().await?;

    // Issue a trust-score credential and revoke it with a fresh attester.
    let issuer = common::credentials::test_issuer_context();
    let authority = issuer.issuer_authority(vec![RevocationLocation {
        protocol: "nostr".to_owned(),
        location: relay.ws_url().to_owned(),
    }])?;
    let holder = HolderContext::generate();
    let info = serde_json::json!({
        "schema": "fedi-trust-score-v1.0",
        "trust_level": 6,
    });
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        serde_json::json!(holder.public_key().to_string()),
    )?;
    let response = issuer.issue_credential(info, &request)?;
    let credential = pending.finalize(&authority.issuer.issuance_key, &response)?;
    let revocation = issuer.revoke_credential(&credential)?;
    let digest = credential_digest_wire_string(&CredentialDigest(credential.credential.digest()?));

    // Publish the attester-authored kind 37704 revocation event.
    let issuer_keys = nostr_sdk::Keys::parse(&issuer.export_secret_key()?.issuer_id_secret_key)?;
    let issuer_pubkey_hex = issuer_keys.public_key().to_string();
    let client = nostr_sdk::Client::new(issuer_keys);
    client.add_relay(relay.ws_url()).await?;
    client.connect().await;
    let builder = nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND),
        serde_json::to_string(&revocation)?,
    )
    .tags([
        nostr_sdk::Tag::identifier(credential_revocation_d_tag(&digest)),
        nostr_sdk::Tag::hashtag(CREDENTIAL_REVOCATION_HASHTAG),
    ]);
    let output = client.send_event_builder(builder).await?;
    anyhow::ensure!(!output.success.is_empty(), "relay accepted the event");
    client.disconnect().await;

    // The production fetcher retrieves it, and feeding it to the verification
    // context makes the revoked credential fail verification.
    let fetched = NostrRevocationFetcher
        .fetch_revocations(&RevocationLookup {
            relay_url: relay.ws_url().to_owned(),
            issuer_pubkey_hex: issuer_pubkey_hex.clone(),
            credential_digests: vec![digest],
        })
        .await?;
    assert_eq!(fetched.len(), 1);

    let mut verifier = VerificationContext::new();
    verifier.add_issuer_authority(&authority)?;
    verifier.verify_credential(&credential)?;
    verifier.add_revocation(&fetched[0])?;
    let error = verifier
        .verify_credential(&credential)
        .expect_err("revoked credential fails verification");
    assert!(matches!(
        error,
        fedi_credential_sdk_protocol::CredentialsError::CredentialRevoked
    ));
    relay.close().await?;
    Ok(())
}

/// One batched lookup returns revocations for every digest it names.
///
/// This is the property the batched revocation stage rests on, and no fake can
/// establish it: `Filter::identifiers` serializes to a `#d` array whose
/// OR-within-tag semantics belong to the relay implementation, not to the SDK.
/// A relay that answered only the first d-tag would silently soft-pass every
/// other credential in the batch.
#[tokio::test(flavor = "multi_thread")]
async fn one_batched_lookup_returns_revocations_for_every_digest() -> anyhow::Result<()> {
    use fedi_credential_sdk_protocol::{
        CredentialDigest, HolderContext, PendingIssuance, RevocationLocation,
    };
    use fedi_decentralized_liquidity_manager_daemon::revocation::{
        NostrRevocationFetcher, RevocationFetcher, RevocationLookup, credential_digest_wire_string,
    };
    use fedi_decentralized_nostr::attester::{
        CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG,
        credential_revocation_d_tag,
    };

    init_logging();
    let relay = NostrRelayFixture::start().await?;

    let issuer = common::credentials::test_issuer_context();
    let authority = issuer.issuer_authority(vec![RevocationLocation {
        protocol: "nostr".to_owned(),
        location: relay.ws_url().to_owned(),
    }])?;

    let issuer_keys = nostr_sdk::Keys::parse(&issuer.export_secret_key()?.issuer_id_secret_key)?;
    let issuer_pubkey_hex = issuer_keys.public_key().to_string();
    let client = nostr_sdk::Client::new(issuer_keys);
    client.add_relay(relay.ws_url()).await?;
    client.connect().await;

    // Two credentials from the same issuer, both revoked and published under
    // their own d-tags.
    let mut digests = Vec::new();
    for trust_level in [4, 6] {
        let holder = HolderContext::generate();
        let info = serde_json::json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": trust_level,
        });
        let (request, pending) = PendingIssuance::create_request(
            &authority.issuer.issuance_key,
            authority.issuer.issuer_id_pubkey.clone(),
            info.clone(),
            serde_json::json!(holder.public_key().to_string()),
        )?;
        let response = issuer.issue_credential(info, &request)?;
        let credential = pending.finalize(&authority.issuer.issuance_key, &response)?;
        let revocation = issuer.revoke_credential(&credential)?;
        let digest =
            credential_digest_wire_string(&CredentialDigest(credential.credential.digest()?));

        let builder = nostr_sdk::EventBuilder::new(
            nostr_sdk::Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND),
            serde_json::to_string(&revocation)?,
        )
        .tags([
            nostr_sdk::Tag::identifier(credential_revocation_d_tag(&digest)),
            nostr_sdk::Tag::hashtag(CREDENTIAL_REVOCATION_HASHTAG),
        ]);
        let output = client.send_event_builder(builder).await?;
        anyhow::ensure!(!output.success.is_empty(), "relay accepted the event");
        digests.push(digest);
    }
    client.disconnect().await;

    let fetched = NostrRevocationFetcher
        .fetch_revocations(&RevocationLookup {
            relay_url: relay.ws_url().to_owned(),
            issuer_pubkey_hex: issuer_pubkey_hex.clone(),
            credential_digests: digests.clone(),
        })
        .await?;

    assert_eq!(
        fetched.len(),
        digests.len(),
        "one lookup naming {} digests must return all of them, got {fetched:?}",
        digests.len()
    );

    // The batch is a union of the named d-tags, not "everything this issuer
    // ever published". Without this the assertion above would also pass on a
    // relay that ignored `#d` entirely, and the stage would be querying far
    // more than it asked for.
    let narrowed = NostrRevocationFetcher
        .fetch_revocations(&RevocationLookup {
            relay_url: relay.ws_url().to_owned(),
            issuer_pubkey_hex: issuer_pubkey_hex.clone(),
            credential_digests: vec![digests[0].clone()],
        })
        .await?;
    assert_eq!(
        narrowed.len(),
        1,
        "naming one digest must return only that one, got {narrowed:?}"
    );

    relay.close().await?;
    Ok(())
}

/// Withdrawal stops a real relay from serving a live advertisement.
///
/// No fake can establish this. Whether a relay honours a deletion request is
/// its own behavior — `nostr-rs-relay` declines the coordinate form — so the
/// withdrawal rests instead on the replacement rule for addressable events,
/// which is the relay's behavior too. This was a local no-op for as long as
/// only fakes covered it, and every fake passed.
///
/// The assertion is what a discovering client would conclude, not which
/// mechanism won: after withdrawal the relay serves either nothing or a
/// document whose `expires_at` has passed.
#[tokio::test(flavor = "multi_thread")]
async fn withdrawal_stops_a_real_relay_from_serving_the_advertisement() -> anyhow::Result<()> {
    use fedi_decentralized_liquidity_manager_daemon::{
        FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
        FLIP_PROVIDER_ADVERTISEMENT_HASHTAG, NostrRelayPublisher, RelayPublishRequest,
        RelayPublisher, RelayWithdrawRequest,
    };
    use fedi_decentralized_service_liquidity_manager::{Timestamp, Url};

    init_logging();
    let relay = NostrRelayFixture::start().await?;

    let keys = nostr_sdk::Keys::generate();
    let provider_pubkey_hex = keys.public_key().to_string();
    let secret_key_hex = keys.secret_key().to_secret_hex();
    let relay_url = Url(relay.ws_url().to_owned());
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let published = NostrRelayPublisher
        .publish(RelayPublishRequest {
            relay_url: relay_url.clone(),
            content: format!(
                r#"{{"payload":{{"marker":"withdrawal-fixture","expires_at":{}}}}}"#,
                issued_at + 3600
            ),
            created_at: Timestamp(issued_at),
            nostr_secret_key_hex: secret_key_hex.clone(),
        })
        .await
        .map_err(|error| anyhow::anyhow!("publish failed: {error}"))?;
    assert!(!published.event_id.is_empty());

    let before = find_advertisement_event(
        relay.ws_url(),
        &provider_pubkey_hex,
        FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
        FLIP_PROVIDER_ADVERTISEMENT_D_TAG,
        FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
    )
    .await?;
    assert!(
        before.is_some(),
        "the relay must serve the advertisement before it is withdrawn, or the \
         absence asserted below would prove nothing"
    );

    let withdrawn_at = issued_at + 1;
    NostrRelayPublisher
        .withdraw(RelayWithdrawRequest {
            relay_url,
            reason: Some("maintenance".to_owned()),
            nostr_secret_key_hex: secret_key_hex,
            expired_content: format!(
                r#"{{"payload":{{"marker":"withdrawal-fixture","expires_at":{withdrawn_at}}}}}"#
            ),
            expired_created_at: Timestamp(withdrawn_at),
        })
        .await
        .map_err(|error| anyhow::anyhow!("withdraw failed: {error}"))?;

    let after = find_advertisement_event(
        relay.ws_url(),
        &provider_pubkey_hex,
        FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
        FLIP_PROVIDER_ADVERTISEMENT_D_TAG,
        FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
    )
    .await?;
    match after {
        None => {}
        Some(event) => {
            let content = event
                .get("content")
                .and_then(serde_json::Value::as_str)
                .expect("relay events carry string content");
            let served: serde_json::Value = serde_json::from_str(content)?;
            let expires_at = served["payload"]["expires_at"]
                .as_u64()
                .expect("the served advertisement carries expires_at");
            assert!(
                expires_at <= withdrawn_at,
                "the relay still serves an unexpired advertisement after withdrawal: {event:?}"
            );
        }
    }

    relay.close().await?;
    Ok(())
}

/// Enrollment against a real relay, including the vocabulary boundary.
///
/// The unit tests drive ingest through a fake fetcher, so they never exercise
/// the filter this daemon actually sends. This does: a Holder publishes a
/// kind-37705 authorization with the FLIP tags, the daemon reconciles it off
/// the relay, and the envelope reaches the advertisement path.
///
/// It also pins the service separation. An authorization published
/// under the FMan hashtag, naming this same provider in its `p` tag and its
/// signed statement, must not enrol here — otherwise the two services share one
/// index and an FMan-targeted publication would leak into a FLIP.
#[tokio::test(flavor = "multi_thread")]
async fn enrols_a_holder_authorization_from_a_real_nostr_relay() -> anyhow::Result<()> {
    use fedi_credential_sdk_protocol::HolderContext;
    use fedi_decentralized_liquidity_manager_daemon::Database;
    use fedi_decentralized_liquidity_manager_daemon::holder_authorization::{
        NostrHolderAuthorizationFetcher, provider_trust_envelopes, refresh,
    };
    use fedi_decentralized_nostr::flip::{
        FLIP_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, flip_authorization_d_tag,
    };
    use fedi_decentralized_service_liquidity_manager::{Pubkey, Url};

    init_logging();
    let relay = NostrRelayFixture::start().await?;

    let provider_keys = nostr_sdk::Keys::generate();
    let provider_pubkey = Pubkey(provider_keys.public_key().to_hex());
    let issuer = common::credentials::test_issuer_context();
    let authority = common::credentials::test_issuer_authority(&issuer, relay.ws_url())?;
    let holder = HolderContext::generate();
    let credential =
        common::credentials::issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = common::credentials::holder_authorization_for_provider(
        &holder,
        &credential,
        &provider_pubkey,
    )?;

    let credential_digest = serde_json::to_value(&authorization.authorization.credential_digest)?
        .as_str()
        .expect("credential digest serializes as a string")
        .to_owned();
    let content = serde_json_canonicalizer::to_string(&serde_json::json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;

    // Publish as the Holder app does.
    let holder_keys = nostr_sdk::Keys::parse(&holder.export_secret_key())?;
    let client = nostr_sdk::Client::new(holder_keys);
    client.add_relay(relay.ws_url()).await?;
    client.connect().await;
    let d_tag = flip_authorization_d_tag(&provider_pubkey.0, &credential_digest);
    client
        .send_event_builder(
            nostr_sdk::EventBuilder::new(
                nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
                content.clone(),
            )
            .tags([
                nostr_sdk::Tag::parse(["d", d_tag.as_str()])?,
                nostr_sdk::Tag::parse(["t", FLIP_AUTHORIZATION_HASHTAG])?,
                nostr_sdk::Tag::parse(["p", provider_pubkey.0.as_str()])?,
            ]),
        )
        .await?;

    // The same authorization, indexed as an FMan one.
    client
        .send_event_builder(
            nostr_sdk::EventBuilder::new(
                nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
                content,
            )
            .tags([
                nostr_sdk::Tag::parse([
                    "d",
                    fedi_decentralized_nostr::fman::fman_authorization_d_tag(
                        &provider_pubkey.0,
                        &credential_digest,
                    )
                    .as_str(),
                ])?,
                nostr_sdk::Tag::parse([
                    "t",
                    fedi_decentralized_nostr::fman::FMAN_AUTHORIZATION_HASHTAG,
                ])?,
                nostr_sdk::Tag::parse(["p", provider_pubkey.0.as_str()])?,
            ]),
        )
        .await?;

    let data_dir = std::env::temp_dir().join(format!(
        "flip-holder-authorization-relay-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let outcome = refresh(
        &database,
        &NostrHolderAuthorizationFetcher,
        &provider_pubkey,
        &[Url(relay.ws_url().to_owned())],
    )
    .await?;

    assert_eq!(outcome.relays_answered, 1);
    assert!(
        outcome.relays_failed.is_empty(),
        "{:?}",
        outcome.relays_failed
    );
    assert_eq!(
        outcome.candidates_seen, 1,
        "only the FLIP-tagged authorization matches the filter this daemon sends"
    );
    assert_eq!(outcome.candidates_verified, 1);
    assert_eq!(outcome.retained, 1);

    let envelopes = provider_trust_envelopes(&database, &provider_pubkey).await?;
    assert_eq!(envelopes.len(), 1);
    assert_eq!(
        envelopes[0]
            .holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string(),
        provider_pubkey.0
    );

    relay.close().await?;
    Ok(())
}
