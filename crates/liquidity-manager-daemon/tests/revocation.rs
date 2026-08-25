use fedi_credential_sdk_protocol::HolderContext;
use fedi_decentralized_service_liquidity_manager::RevocationLocation;

use super::test_fakes::FakeRevocationFetcher;
use super::*;
use crate::test_support::credentials::{
    issue_credential_for_holder, test_foreign_issuer_context, test_issuer_context,
};

struct StageFixture {
    issuer: fedi_credential_sdk_protocol::IssuerContext,
    authority: fedi_credential_sdk_protocol::IssuerAuthority,
    service_authority: IssuerAuthority,
    credential: fedi_credential_sdk_protocol::SignedCredential,
    service_credential: fedi_decentralized_service_liquidity_manager::SignedCredential,
    issuer_pubkey_hex: String,
    digest: CredentialDigest,
}

impl StageFixture {
    /// Issue another credential from the same issuer, for a fresh holder.
    fn another_digest(&self) -> anyhow::Result<CredentialDigest> {
        let holder = HolderContext::generate();
        let credential = issue_credential_for_holder(&self.issuer, &self.authority, &holder)?;
        Ok(CredentialDigest(credential.credential.digest()?))
    }
}

fn stage_fixture(relays: Vec<&str>) -> anyhow::Result<StageFixture> {
    let issuer = test_issuer_context();
    let authority = issuer.issuer_authority(
        relays
            .into_iter()
            .map(|relay| fedi_credential_sdk_protocol::RevocationLocation {
                protocol: "nostr".to_owned(),
                location: relay.to_owned(),
            })
            .collect(),
    )?;
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(&issuer, &authority, &holder)?;
    let issuer_pubkey_hex = authority.issuer.issuer_id_pubkey.0.to_string();
    let digest = CredentialDigest(credential.credential.digest()?);
    let service_authority: IssuerAuthority =
        serde_json::from_value(serde_json::to_value(&authority)?)?;
    let service_credential = serde_json::from_value(serde_json::to_value(&credential)?)?;
    Ok(StageFixture {
        issuer,
        authority,
        service_authority,
        credential,
        service_credential,
        issuer_pubkey_hex,
        digest,
    })
}

fn verifier_for(authority: &IssuerAuthority) -> VerificationContext {
    let mut verifier = VerificationContext::new();
    verifier
        .add_issuer_authority(authority)
        .expect("trust authority");
    verifier
}

#[tokio::test]
async fn revoked_digest_fails_later_credential_verification() -> anyhow::Result<()> {
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    let revocation = fixture.issuer.revoke_credential(&fixture.credential)?;
    let service_revocation: SignedRevocation =
        serde_json::from_value(serde_json::to_value(&revocation)?)?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![service_revocation]);
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[(fixture.issuer_pubkey_hex.clone(), fixture.digest.clone())],
    )
    .await;

    assert!(!result.unavailable);
    assert_eq!(result.checks.len(), 1);
    assert_eq!(result.checks[0].status, VerificationCheckStatus::Passed);
    let error = verifier
        .verify_credential(&fixture.service_credential)
        .expect_err("revoked credential fails verification");
    assert!(matches!(
        error,
        fedi_decentralized_service_liquidity_manager::CredentialsError::CredentialRevoked
    ));
    Ok(())
}

#[tokio::test]
async fn all_relays_failing_is_unavailable() -> anyhow::Result<()> {
    let fixture = stage_fixture(vec!["wss://relay-a.example", "wss://relay-b.example"])?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_err("wss://relay-a.example", "connection refused");
    fetcher.respond_err("wss://relay-b.example", "connection refused");
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[(fixture.issuer_pubkey_hex.clone(), fixture.digest.clone())],
    )
    .await;

    assert!(result.unavailable);
    assert_eq!(result.checks[0].status, VerificationCheckStatus::Failed);
    assert!(
        verifier
            .verify_credential(&fixture.service_credential)
            .is_ok(),
        "no stale revocation state is applied on failure; the caller must \
         reject with provider_unavailable instead"
    );
    Ok(())
}

#[tokio::test]
async fn one_answering_relay_satisfies_the_lookup() -> anyhow::Result<()> {
    let fixture = stage_fixture(vec!["wss://relay-a.example", "wss://relay-b.example"])?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_err("wss://relay-a.example", "connection refused");
    fetcher.respond_ok("wss://relay-b.example", vec![]);
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[(fixture.issuer_pubkey_hex.clone(), fixture.digest.clone())],
    )
    .await;

    assert!(!result.unavailable);
    assert_eq!(result.checks[0].status, VerificationCheckStatus::Passed);
    assert!(
        verifier
            .verify_credential(&fixture.service_credential)
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn no_nostr_locations_is_unavailable() -> anyhow::Result<()> {
    let fixture = stage_fixture(vec![])?;
    // Rebuild the authority with a non-nostr location only.
    let authority = fixture.issuer.issuer_authority(vec![RevocationLocation {
        protocol: "https".to_owned(),
        location: "https://attester.example/revocations".to_owned(),
    }])?;
    let service_authority: IssuerAuthority =
        serde_json::from_value(serde_json::to_value(&authority)?)?;
    let fetcher = FakeRevocationFetcher::default();
    let mut verifier = verifier_for(&service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&service_authority),
        &[(fixture.issuer_pubkey_hex.clone(), fixture.digest.clone())],
    )
    .await;

    assert!(
        result.unavailable,
        "an unsupported revocation mechanism cannot establish freshness"
    );
    assert_eq!(result.checks[0].name, "revocation_freshness");
    assert_eq!(result.checks[0].status, VerificationCheckStatus::Failed);
    assert_eq!(
        result.checks[0].detail.as_deref(),
        Some("issuer authority lists no supported Nostr revocation locations")
    );
    Ok(())
}

#[tokio::test]
async fn foreign_issuer_revocations_are_discarded() -> anyhow::Result<()> {
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    // A revocation signed by a different issuer for the same digest must
    // not enter the verification context.
    let foreign_issuer = test_foreign_issuer_context();
    let foreign_holder = HolderContext::generate();
    let foreign_authority = foreign_issuer.issuer_authority(vec![])?;
    let foreign_credential =
        issue_credential_with(&foreign_issuer, &foreign_authority, &foreign_holder)?;
    let foreign_revocation = foreign_issuer.revoke_credential(&foreign_credential)?;
    let service_foreign: SignedRevocation =
        serde_json::from_value(serde_json::to_value(&foreign_revocation)?)?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![service_foreign]);
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[(fixture.issuer_pubkey_hex.clone(), fixture.digest.clone())],
    )
    .await;

    assert!(!result.unavailable);
    assert!(
        verifier
            .verify_credential(&fixture.service_credential)
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn digests_from_one_issuer_share_a_single_fetch() -> anyhow::Result<()> {
    // Relay locations hang off the issuer authority, so two digests from
    // one issuer already resolve to the same relay list. Querying them
    // separately was a round trip spent to learn nothing new.
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    let second = fixture.another_digest()?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![]);
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[
            (fixture.issuer_pubkey_hex.clone(), fixture.digest.clone()),
            (fixture.issuer_pubkey_hex.clone(), second.clone()),
        ],
    )
    .await;

    let lookups = fetcher.lookups();
    assert_eq!(lookups.len(), 1, "both digests share one round trip");
    assert_eq!(
        lookups[0].credential_digests,
        vec![
            credential_digest_wire_string(&fixture.digest),
            credential_digest_wire_string(&second),
        ],
        "the single lookup carries every digest"
    );

    // Batching must not collapse the accounting: one check per required
    // pair, each naming its own digest.
    assert!(!result.unavailable);
    assert_eq!(result.checks.len(), 2);
    assert!(
        result
            .checks
            .iter()
            .all(|check| check.status == VerificationCheckStatus::Passed)
    );
    for (check, digest) in result.checks.iter().zip([&fixture.digest, &second]) {
        let detail = check.detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains(&credential_digest_wire_string(digest)),
            "check should name its own digest, got {detail}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn digests_from_different_issuers_are_fetched_separately() -> anyhow::Result<()> {
    // Grouping is per issuer because the relay list is: a foreign issuer's
    // revocations live somewhere else entirely and cannot ride along.
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    let foreign_issuer = test_foreign_issuer_context();
    let foreign_authority = foreign_issuer.issuer_authority(vec![
        fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "nostr".to_owned(),
            location: "wss://relay-foreign.example".to_owned(),
        },
    ])?;
    let foreign_holder = HolderContext::generate();
    let foreign_credential =
        issue_credential_with(&foreign_issuer, &foreign_authority, &foreign_holder)?;
    let foreign_digest = CredentialDigest(foreign_credential.credential.digest()?);
    let foreign_pubkey_hex = foreign_authority.issuer.issuer_id_pubkey.0.to_string();
    let service_foreign_authority: IssuerAuthority =
        serde_json::from_value(serde_json::to_value(&foreign_authority)?)?;

    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![]);
    fetcher.respond_ok("wss://relay-foreign.example", vec![]);
    let mut verifier = verifier_for(&fixture.service_authority);
    verifier.add_issuer_authority(&service_foreign_authority)?;

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        &[
            fixture.service_authority.clone(),
            service_foreign_authority.clone(),
        ],
        &[
            (fixture.issuer_pubkey_hex.clone(), fixture.digest.clone()),
            (foreign_pubkey_hex.clone(), foreign_digest.clone()),
        ],
    )
    .await;

    let lookups = fetcher.lookups();
    assert_eq!(lookups.len(), 2, "one fetch per issuer, not one overall");
    assert_eq!(lookups[0].issuer_pubkey_hex, fixture.issuer_pubkey_hex);
    assert_eq!(lookups[1].issuer_pubkey_hex, foreign_pubkey_hex);
    assert!(!result.unavailable);
    assert_eq!(result.checks.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_repeated_digest_still_gets_its_own_check() -> anyhow::Result<()> {
    // Callers are not required to deduplicate, and a caller that does not
    // must still see its own entry answered rather than silently dropped.
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![]);
    let mut verifier = verifier_for(&fixture.service_authority);

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        std::slice::from_ref(&fixture.service_authority),
        &[
            (fixture.issuer_pubkey_hex.clone(), fixture.digest.clone()),
            (fixture.issuer_pubkey_hex.clone(), fixture.digest.clone()),
        ],
    )
    .await;

    let lookups = fetcher.lookups();
    assert_eq!(lookups.len(), 1);
    assert_eq!(
        lookups[0].credential_digests.len(),
        1,
        "the duplicate is queried once"
    );
    assert_eq!(result.checks.len(), 2, "but still answered twice");
    Ok(())
}

#[tokio::test]
async fn one_failing_issuer_does_not_mark_another_issuers_digests_unavailable() -> anyhow::Result<()>
{
    // Fail-closed is per issuer group. A dead relay for one issuer must not
    // fail digests that were freshly resolved somewhere else, and must
    // still set the stage-wide `unavailable` flag.
    let fixture = stage_fixture(vec!["wss://relay-a.example"])?;
    let foreign_issuer = test_foreign_issuer_context();
    let foreign_authority = foreign_issuer.issuer_authority(vec![
        fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "nostr".to_owned(),
            location: "wss://relay-dead.example".to_owned(),
        },
    ])?;
    let foreign_holder = HolderContext::generate();
    let foreign_credential =
        issue_credential_with(&foreign_issuer, &foreign_authority, &foreign_holder)?;
    let foreign_digest = CredentialDigest(foreign_credential.credential.digest()?);
    let foreign_pubkey_hex = foreign_authority.issuer.issuer_id_pubkey.0.to_string();
    let service_foreign_authority: IssuerAuthority =
        serde_json::from_value(serde_json::to_value(&foreign_authority)?)?;

    let fetcher = FakeRevocationFetcher::default();
    fetcher.respond_ok("wss://relay-a.example", vec![]);
    fetcher.respond_err("wss://relay-dead.example", "connection refused");
    let mut verifier = verifier_for(&fixture.service_authority);
    verifier.add_issuer_authority(&service_foreign_authority)?;

    let result = run_revocation_stage(
        &fetcher,
        &mut verifier,
        &[
            fixture.service_authority.clone(),
            service_foreign_authority.clone(),
        ],
        &[
            (fixture.issuer_pubkey_hex.clone(), fixture.digest.clone()),
            (foreign_pubkey_hex.clone(), foreign_digest.clone()),
        ],
    )
    .await;

    assert!(result.unavailable, "the stage as a whole is fail-closed");
    assert_eq!(result.checks[0].status, VerificationCheckStatus::Passed);
    assert_eq!(result.checks[1].status, VerificationCheckStatus::Failed);
    Ok(())
}

fn issue_credential_with(
    issuer: &fedi_credential_sdk_protocol::IssuerContext,
    authority: &fedi_credential_sdk_protocol::IssuerAuthority,
    holder: &HolderContext,
) -> anyhow::Result<fedi_credential_sdk_protocol::SignedCredential> {
    let info = serde_json::json!({
        "schema": "fedi-trust-score-v1.0",
        "trust_level": 5,
    });
    let (request, pending) = fedi_credential_sdk_protocol::PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        serde_json::json!(holder.public_key().to_string()),
    )?;
    let response = issuer.issue_credential(info, &request)?;
    Ok(pending.finalize(&authority.issuer.issuance_key, &response)?)
}
