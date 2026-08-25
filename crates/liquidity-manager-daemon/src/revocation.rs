//! Fresh, fail-closed revocation lookups over the shared attester profile.
//!
//! Revocations are attester-authored Nostr kind `37704`
//! [`SignedRevocation`] events published on the relays listed in the trusted
//! `IssuerAuthority.issuer.revocation` entries. Lookups run fresh at request
//! verification time; a required lookup that cannot complete makes the
//! provider unavailable instead of soft-passing on stale state.

use std::time::Duration;

use async_trait::async_trait;
use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, credential_revocation_d_tag,
};
use fedi_decentralized_service_liquidity_manager::{
    CredentialDigest, IssuerAuthority, SignedRevocation, VerificationCheck,
    VerificationCheckStatus, VerificationContext,
};
use nostr_sdk::{Client, Filter, Kind};
use tracing::warn;

pub const REVOCATION_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum revocation events accepted per credential digest in a lookup.
///
/// Revocation events are addressable (one latest event per credential digest
/// d-tag per author), so a small bound is generous. A batched lookup scales it
/// by the number of digests queried.
pub const REVOCATION_FETCH_MAX_EVENTS: usize = 16;

/// One relay lookup for revocations of one issuer's credential digests.
///
/// Digests are batched into a single query rather than one round trip apiece.
/// Relay locations are a property of the *issuer* authority, not of the
/// individual credential, so every digest attested by one issuer already
/// resolves to the same relay list — the filter simply carries every d-tag at
/// once (`#d` is an OR within the tag).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevocationLookup {
    /// Relay URL from the trusted issuer authority's revocation locations.
    pub relay_url: String,

    /// Attester issuer pubkey, canonical lowercase hex.
    pub issuer_pubkey_hex: String,

    /// Credential digests in the SDK base64url-unpadded wire form.
    ///
    /// Never empty: a lookup with no digests would query nothing and report a
    /// satisfied relay, which is the soft-pass this stage refuses.
    pub credential_digests: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RevocationFetchError {
    /// The relay could not be reached or the query failed.
    #[error("revocation lookup failed: {0}")]
    Unavailable(String),
}

/// Boundary for fetching signed revocation events from attester relays.
#[async_trait]
pub trait RevocationFetcher: Send + Sync {
    /// Fetch kind `37704` revocation events for one lookup.
    ///
    /// Implementations return the parsed [`SignedRevocation`] documents whose
    /// event author matches the lookup issuer, for **any** of the lookup's
    /// digests; signature verification and matching a revocation back to a
    /// digest stay with the caller. An `Ok` with an empty list means the relay
    /// answered and no revocation is published for any of them.
    async fn fetch_revocations(
        &self,
        lookup: &RevocationLookup,
    ) -> Result<Vec<SignedRevocation>, RevocationFetchError>;
}

/// Production revocation fetcher over `nostr-sdk`.
#[derive(Clone, Debug, Default)]
pub struct NostrRevocationFetcher;

#[async_trait]
impl RevocationFetcher for NostrRevocationFetcher {
    async fn fetch_revocations(
        &self,
        lookup: &RevocationLookup,
    ) -> Result<Vec<SignedRevocation>, RevocationFetchError> {
        // A digest-less lookup would build `limit(0)` and an unconstrained `#d`,
        // returning nothing — which `run_revocation_stage` reads as an answering
        // relay with no revocation published. Refusing is the fail-closed
        // reading; the stage never constructs one, so this only catches a caller
        // building lookups directly.
        if lookup.credential_digests.is_empty() {
            return Err(RevocationFetchError::Unavailable(
                "revocation lookup names no credential digests".to_owned(),
            ));
        }
        let issuer = nostr_sdk::PublicKey::parse(&lookup.issuer_pubkey_hex).map_err(|error| {
            RevocationFetchError::Unavailable(format!("bad issuer key: {error}"))
        })?;
        let client = Client::default();
        client
            .add_relay(&lookup.relay_url)
            .await
            .map_err(|error| RevocationFetchError::Unavailable(error.to_string()))?;
        // `try_connect_relay` rather than the fire-and-forget `connect`:
        // `fetch_events` returns an empty result rather than an error when no
        // relay is reachable. Without proving the connection first, an
        // unreachable relay would be indistinguishable from "this credential
        // has no revocation published" — and `run_revocation_stage` treats an
        // answering relay as satisfying the lookup, so a revoked credential
        // would pass. That is the stale soft-pass this stage exists to refuse.
        client
            .try_connect_relay(&lookup.relay_url, REVOCATION_FETCH_TIMEOUT)
            .await
            .map_err(|error| {
                RevocationFetchError::Unavailable(format!("relay unreachable: {error}"))
            })?;

        // One filter carrying every d-tag: `#d` matches any listed value, and
        // the author term still ANDs across, so this is exactly the union of
        // the per-digest queries it replaces.
        let max_events =
            REVOCATION_FETCH_MAX_EVENTS.saturating_mul(lookup.credential_digests.len());
        let filter = Filter::new()
            .kind(Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND))
            .author(issuer)
            .identifiers(
                lookup
                    .credential_digests
                    .iter()
                    .map(|digest| credential_revocation_d_tag(digest)),
            )
            .limit(max_events);
        let events = client
            .fetch_events(filter, REVOCATION_FETCH_TIMEOUT)
            .await
            .map_err(|error| RevocationFetchError::Unavailable(error.to_string()));
        client.disconnect().await;
        let events = events?;

        let mut revocations = Vec::new();
        for event in events.into_iter().take(max_events) {
            if event.pubkey != issuer {
                continue;
            }
            match serde_json::from_str::<SignedRevocation>(&event.content) {
                Ok(revocation) => revocations.push(revocation),
                Err(error) => {
                    warn!(relay = %lookup.relay_url, %error, "discarding malformed revocation event");
                }
            }
        }
        Ok(revocations)
    }
}

/// Result of the revocation stage for one verification pass.
#[derive(Debug)]
pub struct RevocationStageResult {
    /// One check per required (issuer, digest) lookup.
    pub checks: Vec<VerificationCheck>,

    /// True when a required lookup has no supported Nostr location or no listed
    /// Nostr relay can answer. The caller must map this to
    /// `provider_unavailable`; there is no stale soft-pass.
    pub unavailable: bool,
}

/// Run the fail-closed revocation stage for the required credential digests.
///
/// Required pairs are grouped by issuer and resolved in **one** lookup per
/// issuer: relay locations hang off the issuer authority, so every digest from
/// one issuer shares a relay list, and a single filter can carry every d-tag.
/// Within a group, every `protocol == "nostr"` location is queried until one
/// relay answers; an answering relay (even with no revocation published)
/// satisfies the lookup for every digest in that group. An authority without a
/// Nostr location cannot satisfy FLIP's required lookup and makes the stage
/// unavailable. Verified revocations are fed into the verification context so
/// later `verify_credential` calls fail with `CredentialRevoked`. Foreign-author
/// or unverifiable events are discarded.
///
/// Batching changes the number of round trips, never the accounting: the result
/// still carries exactly one check per entry in `required`, in the same order,
/// so a caller cannot tell from `checks` whether digests shared a fetch.
pub async fn run_revocation_stage(
    fetcher: &dyn RevocationFetcher,
    verifier: &mut VerificationContext,
    authorities: &[IssuerAuthority],
    required: &[(String, CredentialDigest)],
) -> RevocationStageResult {
    // One outcome per `required` entry, filled out of order as issuer groups
    // resolve and emitted in `required` order below.
    let mut outcomes: Vec<Option<(VerificationCheckStatus, String)>> = vec![None; required.len()];
    let mut unavailable = false;

    for (issuer_pubkey_hex, indices) in group_by_issuer(required) {
        // Distinct digests for the query; every index still gets its own check.
        let mut digest_strings: Vec<String> = Vec::new();
        for &index in &indices {
            let digest = credential_digest_wire_string(&required[index].1);
            if !digest_strings.contains(&digest) {
                digest_strings.push(digest);
            }
        }

        let Some(authority) = authorities
            .iter()
            .find(|authority| authority.issuer.issuer_id_pubkey.0.to_string() == issuer_pubkey_hex)
        else {
            unavailable = true;
            for &index in &indices {
                outcomes[index] = Some((
                    VerificationCheckStatus::Failed,
                    "no trusted issuer authority is installed for this issuer".to_owned(),
                ));
            }
            continue;
        };

        let relay_locations: Vec<&str> = authority
            .issuer
            .revocation
            .iter()
            .filter(|location| location.protocol == "nostr")
            .map(|location| location.location.as_str())
            .collect();
        if relay_locations.is_empty() {
            // FLIP only implements signed Nostr revocation events. An authority
            // that offers only another mechanism cannot prove that this
            // credential remains unrevoked, so the caller must fail closed.
            unavailable = true;
            for &index in &indices {
                outcomes[index] = Some((
                    VerificationCheckStatus::Failed,
                    "issuer authority lists no supported Nostr revocation locations".to_owned(),
                ));
            }
            continue;
        }

        let mut satisfied = false;
        let mut last_error = String::new();
        for relay_url in &relay_locations {
            let lookup = RevocationLookup {
                relay_url: (*relay_url).to_owned(),
                issuer_pubkey_hex: issuer_pubkey_hex.clone(),
                credential_digests: digest_strings.clone(),
            };
            match fetcher.fetch_revocations(&lookup).await {
                Ok(revocations) => {
                    for revocation in revocations {
                        if revocation.proof.issuer_id_pubkey.0.to_string() != issuer_pubkey_hex {
                            continue;
                        }
                        if let Err(error) = verifier.add_revocation(&revocation) {
                            warn!(relay = %relay_url, %error, "discarding unverifiable revocation");
                        }
                    }
                    satisfied = true;
                    break;
                }
                Err(error) => {
                    last_error = error.to_string();
                    warn!(relay = %relay_url, issuer = %issuer_pubkey_hex, %last_error, "revocation lookup failed");
                }
            }
        }

        if satisfied {
            for &index in &indices {
                let digest_string = credential_digest_wire_string(&required[index].1);
                outcomes[index] = Some((
                    VerificationCheckStatus::Passed,
                    format!("fresh revocation lookup completed for digest {digest_string}"),
                ));
            }
        } else {
            unavailable = true;
            for &index in &indices {
                outcomes[index] = Some((
                    VerificationCheckStatus::Failed,
                    format!("every listed revocation relay failed: {last_error}"),
                ));
            }
        }
    }

    let checks = required
        .iter()
        .zip(outcomes)
        .map(|((issuer_pubkey_hex, _), outcome)| {
            let (status, detail) =
                outcome.expect("every required entry belongs to an issuer group");
            revocation_check(status, issuer_pubkey_hex, detail)
        })
        .collect();

    RevocationStageResult {
        checks,
        unavailable,
    }
}

/// Group `required` indices by issuer, preserving first-appearance order.
///
/// Order is preserved so the batched stage queries issuers in the same sequence
/// the unbatched one did, which keeps failure ordering (and so the first
/// reported error) stable.
fn group_by_issuer(required: &[(String, CredentialDigest)]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, (issuer_pubkey_hex, _)) in required.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|(issuer, _)| issuer == issuer_pubkey_hex)
        {
            Some((_, indices)) => indices.push(index),
            None => groups.push((issuer_pubkey_hex.clone(), vec![index])),
        }
    }
    groups
}

/// Serialize a credential digest to its base64url-unpadded wire string.
///
/// The credential SDK owns this encoding (its serde form); the d-tag built
/// from it must match what attesters publish. A non-string serialization is
/// unrepresentable for `CredentialDigest`, and quietly substituting another
/// value here would make the fail-closed lookup query the wrong d-tag and
/// soft-pass, so anything else panics instead.
pub fn credential_digest_wire_string(digest: &CredentialDigest) -> String {
    match serde_json::to_value(digest) {
        Ok(serde_json::Value::String(digest)) => digest,
        other => unreachable!("CredentialDigest must serialize as a JSON string, got {other:?}"),
    }
}

fn revocation_check(
    status: VerificationCheckStatus,
    issuer_pubkey_hex: &str,
    detail: impl Into<String>,
) -> VerificationCheck {
    VerificationCheck {
        name: "revocation_freshness".to_owned(),
        status,
        subject: Some(issuer_pubkey_hex.to_owned()),
        detail: Some(detail.into()),
    }
}

#[cfg(test)]
mod real_fetcher_tests {
    use super::*;

    /// A port nothing listens on, so the connection is refused immediately.
    const DEAD_RELAY: &str = "ws://127.0.0.1:1";

    #[tokio::test]
    async fn an_unreachable_relay_is_an_error_not_an_absent_revocation() {
        // The stage treats an answering relay as satisfying the lookup, so an
        // unreachable relay that returned `Ok(vec![])` would let a revoked
        // credential pass — the stale soft-pass this stage exists to refuse.
        let lookup = RevocationLookup {
            relay_url: DEAD_RELAY.to_owned(),
            issuer_pubkey_hex: nostr_sdk::Keys::generate().public_key().to_string(),
            credential_digests: vec!["2u0Za9RCXVW0zzoUpG-4iBGCLGVdnvBpJUZoGDaK5dY".to_owned()],
        };

        let result = NostrRevocationFetcher.fetch_revocations(&lookup).await;

        assert!(
            result.is_err(),
            "an unreachable relay must not read as an absent revocation, got {result:?}"
        );
    }

    #[tokio::test]
    async fn a_digest_less_lookup_is_an_error_not_an_empty_result() {
        // `limit(0)` with an unconstrained `#d` returns nothing, which the
        // stage would read as "relay answered, nothing revoked" — a soft-pass
        // reached without contacting a relay at all.
        let lookup = RevocationLookup {
            relay_url: DEAD_RELAY.to_owned(),
            issuer_pubkey_hex: nostr_sdk::Keys::generate().public_key().to_string(),
            credential_digests: Vec::new(),
        };

        let result = NostrRevocationFetcher.fetch_revocations(&lookup).await;

        assert!(
            result.is_err(),
            "a lookup naming no digests must not read as an absent revocation, got {result:?}"
        );
    }
}

#[cfg(test)]
pub(crate) mod test_fakes {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Programmable fake revocation fetcher keyed by relay URL.
    ///
    /// Records every lookup it is handed so tests can assert how many round
    /// trips the stage made and which digests shared one.
    #[derive(Default)]
    pub(crate) struct FakeRevocationFetcher {
        responses: Mutex<HashMap<String, Result<Vec<SignedRevocation>, String>>>,
        lookups: Mutex<Vec<RevocationLookup>>,
    }

    impl FakeRevocationFetcher {
        pub(crate) fn respond_ok(&self, relay_url: &str, revocations: Vec<SignedRevocation>) {
            self.responses
                .lock()
                .expect("fake fetcher lock")
                .insert(relay_url.to_owned(), Ok(revocations));
        }

        pub(crate) fn respond_err(&self, relay_url: &str, error: &str) {
            self.responses
                .lock()
                .expect("fake fetcher lock")
                .insert(relay_url.to_owned(), Err(error.to_owned()));
        }

        /// Every lookup this fetcher was handed, in call order.
        pub(crate) fn lookups(&self) -> Vec<RevocationLookup> {
            self.lookups.lock().expect("fake fetcher lock").clone()
        }
    }

    #[async_trait]
    impl RevocationFetcher for FakeRevocationFetcher {
        async fn fetch_revocations(
            &self,
            lookup: &RevocationLookup,
        ) -> Result<Vec<SignedRevocation>, RevocationFetchError> {
            self.lookups
                .lock()
                .expect("fake fetcher lock")
                .push(lookup.clone());
            match self
                .responses
                .lock()
                .expect("fake fetcher lock")
                .get(&lookup.relay_url)
            {
                Some(Ok(revocations)) => Ok(revocations.clone()),
                Some(Err(error)) => Err(RevocationFetchError::Unavailable(error.clone())),
                None => Err(RevocationFetchError::Unavailable(format!(
                    "no fake response programmed for {}",
                    lookup.relay_url
                ))),
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/revocation.rs"]
mod tests;
