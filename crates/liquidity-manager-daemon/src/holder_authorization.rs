//! Holder-published kind-37705 authorization ingest.
//!
//! Enrollment is the Holder publishing an authorization naming this provider's
//! pubkey, followed by the operator asking the daemon to reconcile against the
//! relays its deployment environment pins. This mirrors FMan's flow
//! deliberately (`crates/fman/nostr/src/lib.rs`): the two services run one
//! enrollment protocol, and [`verify_candidate`] is a close counterpart of
//! FMan's so a reviewer can diff them.
//!
//! The relay set is the environment's, never the operator's advertisement relay
//! config. Those answer different questions — where a Holder publishes, versus
//! where this provider advertises — and conflating them yields a refresh that
//! looks healthy while querying relays the Holder never wrote to. The
//! environment may pin several, so a refresh unions candidates across them and
//! reports per-relay failure.
//!
//! Everything else matches, including what this module deliberately does not
//! do. It establishes that a holder signed a statement naming this provider,
//! covering an inline credential payload issued to that same holder. It does
//! not judge the badge, and nothing downstream of it does either — see
//! [`provider_trust_envelopes`].

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use fedi_decentralized_nostr::flip::{
    FLIP_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, HolderAuthorizationEventContent,
};
use fedi_decentralized_service_liquidity_manager::{
    HolderAuthorizationEnvelope, HolderAuthorizationStatus, Pubkey, ServiceResult, Timestamp, Url,
};
use nostr_sdk::{Client, Event, Filter, JsonUtil, Kind, PublicKey};
use sqlx::Row;

use crate::daemon::Worker;
use crate::database::Database;
use crate::internal_error;

/// Per-relay budget for one reconciliation. A refresh is operator-driven and
/// interactive, so this bounds how long the whole verb can take when a
/// configured relay is black-holing rather than refusing.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Candidate cap per relay, matching FMan's. The relay chooses what to return,
/// so this is a work bound on a hostile answer, not a completeness promise.
const FETCH_LIMIT: usize = 64;

/// One kind-37705 candidate that passed every local check.
#[derive(Clone, Debug)]
pub struct VerifiedHolderAuthorization {
    /// The authorization and its inline backing credential.
    pub envelope: HolderAuthorizationEnvelope,

    /// Digest of the authorized credential; the durable row identity.
    pub credential_digest: Vec<u8>,

    /// Signed statement timestamp, which orders replacement.
    pub authorization_issued_at: u64,

    /// The complete event, retained so every later read re-verifies it.
    pub event_json: String,
}

/// Verify one discovered kind-37705 candidate against the provider pubkey.
///
/// Passing establishes four bindings: the statement was signed by the key that
/// authored the event, it names this provider as its subject, its credential
/// digest covers the inline credential payload, and that credential is bound to
/// the same holder that signed the statement. It establishes nothing else about
/// the badge — not that the credential has a valid issuer proof, and not that it
/// is unrevoked. The relying app owns those
/// ([`provider_trust_envelopes`]).
///
/// The holder-binding check is what stops one holder presenting another's
/// badge. Without it a hostile key can sign a valid statement naming a victim's
/// credential digest, attach the victim's published credential, and displace
/// the victim's enrolled authorization with a later-dated copy — every other
/// check here passes, because the statement really is signed and the digest
/// really does cover the attached credential. Comparing the credential's
/// revealed holder binding to the statement's holder needs no issuer trust, so
/// it stays inside what this daemon is willing to judge.
///
/// Event kind, timestamps, and tags are not checked. The fetch filter is an
/// untrusted relay hint, so an off-kind or wrongly tagged event still has to
/// carry a valid inner authorization to pass here.
///
/// # Errors
///
/// Returns an error when the event signature, content shape, author binding,
/// authorization proof, subject, holder binding, or credential digest fails.
pub fn verify_candidate(
    event: &Event,
    provider_pubkey: &PublicKey,
) -> anyhow::Result<VerifiedHolderAuthorization> {
    event.verify().context("invalid event signature")?;
    let content: HolderAuthorizationEventContent =
        serde_json::from_str(&event.content).context("unparsable authorization envelope")?;
    anyhow::ensure!(
        content.holder_id_pubkey == event.pubkey.to_string(),
        "envelope holder pubkey does not match event author"
    );
    let statement = content
        .authorization
        .holder_authorization
        .verify()
        .map_err(|err| anyhow::anyhow!("holder authorization proof invalid: {err}"))?;
    anyhow::ensure!(
        statement.holder_id_pubkey.0 == event.pubkey,
        "authorization statement holder does not match event author"
    );
    anyhow::ensure!(
        statement.subject_pubkey.0 == *provider_pubkey,
        "authorization subject is not this provider"
    );
    let credential_holder = content
        .authorization
        .signed_credential
        .credential
        .blind_msg
        .as_str()
        .context("credential holder binding is not a string")?
        .parse::<PublicKey>()
        .context("credential holder binding is not a public key")?;
    anyhow::ensure!(
        credential_holder == statement.holder_id_pubkey.0,
        "credential holder binding does not match authorization holder"
    );
    let digest = content
        .authorization
        .signed_credential
        .credential
        .digest()
        .map_err(|err| anyhow::anyhow!("credential digest failed: {err}"))?;
    anyhow::ensure!(
        statement.credential_digest.0 == digest,
        "credential digest does not match the authorized trust badge"
    );
    Ok(VerifiedHolderAuthorization {
        credential_digest: digest.to_vec(),
        authorization_issued_at: statement.issued_at.0,
        event_json: event.as_json(),
        envelope: content.authorization,
    })
}

/// Relay fetch boundary, so tests drive ingest without a relay.
#[async_trait]
pub trait HolderAuthorizationFetcher: Send + Sync {
    /// Fetch untrusted kind-37705 candidates naming `provider_pubkey`.
    ///
    /// # Errors
    ///
    /// Returns an operator-readable reason when the relay cannot be reached or
    /// the query fails.
    async fn fetch_candidates(
        &self,
        relay_url: &Url,
        provider_pubkey: PublicKey,
    ) -> Result<Vec<Event>, String>;
}

/// Nostr-backed fetcher.
#[derive(Clone, Debug, Default)]
pub struct NostrHolderAuthorizationFetcher;

#[async_trait]
impl HolderAuthorizationFetcher for NostrHolderAuthorizationFetcher {
    async fn fetch_candidates(
        &self,
        relay_url: &Url,
        provider_pubkey: PublicKey,
    ) -> Result<Vec<Event>, String> {
        let client = Client::default();
        client
            .add_relay(&relay_url.0)
            .await
            .map_err(|error| error.to_string())?;
        // Prove the connection before querying. `fetch_events` returns an empty
        // result rather than an error when no relay is reachable, which would
        // make an unreachable relay indistinguishable from one that has no
        // authorization published. Both leave the provider unadvertised, so
        // this is an operator-legibility choice rather than a fail-open one.
        client
            .try_connect_relay(&relay_url.0, FETCH_TIMEOUT)
            .await
            .map_err(|error| format!("relay unreachable: {error}"))?;

        let filter = Filter::new()
            .kind(Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND))
            .pubkey(provider_pubkey)
            .hashtag(FLIP_AUTHORIZATION_HASHTAG)
            .limit(FETCH_LIMIT);
        let events = client.fetch_events(filter, FETCH_TIMEOUT).await;
        client.disconnect().await;

        Ok(events
            .map_err(|error| error.to_string())?
            .into_iter()
            .take(FETCH_LIMIT)
            .collect())
    }
}

/// What one reconciliation did, for the operator-facing response.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefreshOutcome {
    /// Relays that answered, whether or not they carried anything.
    pub relays_answered: usize,

    /// Relays that could not be reached or refused, with the reason.
    pub relays_failed: Vec<(Url, String)>,

    /// Candidates offered across every answering relay.
    pub candidates_seen: usize,

    /// Candidates that passed [`verify_candidate`].
    pub candidates_verified: usize,

    /// Authorizations durably retained after the merge.
    pub retained: usize,
}

/// Reconcile retained authorizations against every configured relay.
///
/// Relay answers are additive: any relay may supply a candidate, and each one
/// still has to pass [`verify_candidate`] locally, so a hostile relay cannot
/// promote anything by answering. A relay that fails is reported and does not
/// fail the refresh — the union of what the reachable relays served is the best
/// available answer, and refusing it would let one bad relay block enrollment.
///
/// # Errors
///
/// Returns an error when the provider pubkey is malformed or the durable merge
/// fails. Relay failures are reported in the outcome instead.
pub async fn refresh(
    database: &Database,
    fetcher: &dyn HolderAuthorizationFetcher,
    provider_pubkey: &Pubkey,
    relays: &[Url],
) -> ServiceResult<RefreshOutcome> {
    let provider_key = parse_provider_pubkey(provider_pubkey)?;
    let mut outcome = RefreshOutcome::default();
    let mut winners: BTreeMap<Vec<u8>, VerifiedHolderAuthorization> = BTreeMap::new();

    for relay_url in relays {
        let candidates = match fetcher.fetch_candidates(relay_url, provider_key).await {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(relay = %relay_url.0, %error, "holder authorization fetch failed");
                outcome.relays_failed.push((relay_url.clone(), error));
                continue;
            }
        };
        outcome.relays_answered += 1;
        outcome.candidates_seen += candidates.len();
        for event in candidates {
            let event_id = event.id;
            match verify_candidate(&event, &provider_key) {
                Ok(candidate) => {
                    outcome.candidates_verified += 1;
                    let replace = winners.get(&candidate.credential_digest).is_none_or(
                        |current: &VerifiedHolderAuthorization| {
                            candidate.authorization_issued_at > current.authorization_issued_at
                        },
                    );
                    if replace {
                        winners.insert(candidate.credential_digest.clone(), candidate);
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        relay = %relay_url.0,
                        %event_id,
                        ?error,
                        "skipping holder authorization candidate"
                    );
                }
            }
        }
    }

    if !winners.is_empty() {
        merge(database, &winners.into_values().collect::<Vec<_>>()).await?;
    }
    outcome.retained = retained_count(database).await?;
    Ok(outcome)
}

/// Retain each candidate, replacing a stored authorization for the same
/// credential only when the new signed `issued_at` is strictly greater.
///
/// This is what makes a replayed older authorization inert once a newer one is
/// enrolled. It is not a freshness check: nothing here compares an `issued_at`
/// against the clock, so the first authorization admitted for a credential is
/// whatever the relays served, however old.
async fn merge(
    database: &Database,
    candidates: &[VerifiedHolderAuthorization],
) -> ServiceResult<()> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    for candidate in candidates {
        sqlx::query(
            "INSERT INTO holder_authorization_events \
             (credential_digest, authorization_issued_at, event_json) VALUES (?, ?, ?) \
             ON CONFLICT (credential_digest) DO UPDATE SET \
               authorization_issued_at = excluded.authorization_issued_at, \
               event_json = excluded.event_json, \
               ingested_at = unixepoch() \
             WHERE excluded.authorization_issued_at > \
                   holder_authorization_events.authorization_issued_at",
        )
        .bind(&candidate.credential_digest)
        .bind(candidate.authorization_issued_at.to_be_bytes().to_vec())
        .bind(&candidate.event_json)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    }
    tx.commit().await.map_err(internal_error)
}

async fn retained_count(database: &Database) -> ServiceResult<usize> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM holder_authorization_events")
        .fetch_one(database.pool())
        .await
        .map_err(internal_error)?;
    usize::try_from(count).map_err(|_| internal_error(format!("negative row count {count}")))
}

/// Re-verify every retained event against the current provider identity.
///
/// A retained row is a relay answer this daemon once accepted, not an
/// assertion. Reading it back re-runs [`verify_candidate`], so a row that no
/// longer binds — most plainly, one enrolled under a different provider key —
/// cannot re-enter runtime state.
///
/// # Errors
///
/// Returns an error when the provider pubkey is malformed or a row cannot be
/// read. A row that fails verification is dropped with a warning rather than
/// failing the load, so one bad row cannot strand the rest.
pub async fn load_verified(
    database: &Database,
    provider_pubkey: &Pubkey,
) -> ServiceResult<Vec<VerifiedHolderAuthorization>> {
    let provider_key = parse_provider_pubkey(provider_pubkey)?;
    let rows = sqlx::query(
        "SELECT event_json FROM holder_authorization_events \
         ORDER BY ingested_at ASC, credential_digest ASC",
    )
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;

    let mut verified = Vec::new();
    for row in rows {
        let event_json: String = row.get("event_json");
        let candidate = Event::from_json(&event_json)
            .context("parse retained holder authorization")
            .and_then(|event| verify_candidate(&event, &provider_key));
        match candidate {
            Ok(candidate) => verified.push(candidate),
            Err(error) => tracing::warn!(
                ?error,
                "dropping a retained holder authorization that no longer verifies"
            ),
        }
    }
    Ok(verified)
}

/// What the last reconciliation in this runtime generation concluded.
///
/// Runtime state, not durable: a restart genuinely has not read the relay yet,
/// and saying so is the point. What survives a restart is the enrolled rows,
/// which outrank this entirely.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LastRelayRead {
    /// Nothing has been reconciled since this runtime generation started.
    #[default]
    NotYet,

    /// A reconciliation finished with at least one relay answering.
    Completed(Timestamp),

    /// Every relay tried refused or was unreachable.
    Failed {
        /// When the attempt finished.
        at: Timestamp,

        /// Operator-readable reason from the last relay tried.
        reason: String,
    },
}

impl LastRelayRead {
    /// Record what one reconciliation concluded.
    #[must_use]
    pub fn from_outcome(outcome: &RefreshOutcome, now: Timestamp) -> Self {
        if outcome.relays_answered > 0 {
            return Self::Completed(now);
        }
        match outcome.relays_failed.last() {
            Some((_, reason)) => Self::Failed {
                at: now,
                reason: reason.clone(),
            },
            // No relay answered and none failed means none was tried, which
            // the environment profile makes impossible: its relay set is
            // non-empty by construction.
            None => Self::Completed(now),
        }
    }
}

/// Project enrolled authorizations the way the operator console reads them.
///
/// Counts describe what this daemon admitted, not who deserves trust: any
/// holder of a genuine badge naming this provider appears here. The projection
/// exists so an operator can see the Holder's publication arrive, and nothing
/// branches on it.
///
/// An enrolled authorization outranks whatever the last read concluded,
/// because the rows are durable and re-verified on every use: a relay outage
/// must not make a provider look unauthorized when its advertisement still
/// carries the envelope.
#[must_use]
pub fn status_of(
    authorizations: &[VerifiedHolderAuthorization],
    newest_ingested_at: Option<Timestamp>,
    last_read: &LastRelayRead,
) -> HolderAuthorizationStatus {
    if authorizations.is_empty() {
        return match last_read {
            LastRelayRead::NotYet => HolderAuthorizationStatus::Checking,
            LastRelayRead::Completed(at) => HolderAuthorizationStatus::NotObserved {
                read_completed_at: *at,
            },
            LastRelayRead::Failed { at, reason } => HolderAuthorizationStatus::RelayError {
                reason: reason.clone(),
                failed_at: *at,
            },
        };
    }
    let mut holders = authorizations
        .iter()
        .map(|authorization| {
            Pubkey(
                authorization
                    .envelope
                    .holder_authorization
                    .authorization
                    .holder_id_pubkey
                    .0
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    holders.sort_by(|left, right| left.0.cmp(&right.0));
    holders.dedup();
    HolderAuthorizationStatus::AuthorizationObserved {
        authorizations: u32::try_from(authorizations.len()).unwrap_or(u32::MAX),
        holders,
        newest_ingested_at: newest_ingested_at.unwrap_or(Timestamp(0)),
    }
}

/// Read the enrollment state for the active provider key.
///
/// # Errors
///
/// Returns an error when the provider pubkey is malformed or a retained row
/// cannot be read.
pub async fn status(
    database: &Database,
    provider_pubkey: &Pubkey,
    last_read: &LastRelayRead,
) -> ServiceResult<HolderAuthorizationStatus> {
    let enrolled = load_verified(database, provider_pubkey).await?;
    let newest = newest_ingested_at(database).await?;
    Ok(status_of(&enrolled, newest, last_read))
}

/// When the most recently ingested authorization was taken in.
async fn newest_ingested_at(database: &Database) -> ServiceResult<Option<Timestamp>> {
    let newest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(ingested_at) FROM holder_authorization_events")
            .fetch_one(database.pool())
            .await
            .map_err(internal_error)?;
    Ok(newest
        .map(u64::try_from)
        .transpose()
        .map_err(|_| internal_error("negative ingested_at"))?
        .map(Timestamp))
}

/// Run one reconciliation against the environment's relays and record what it
/// concluded.
///
/// The single place enrollment happens, so the startup read and the operator's
/// refresh cannot drift in which relays they consult or what they record.
///
/// The relay set is the deployment environment's, never the operator's
/// advertisement relay configuration. Those answer different questions — where
/// a Holder publishes, versus where this provider advertises — and reconciling
/// against the latter reports an answering relay with no candidates while the
/// Holder's event sits somewhere never queried.
///
/// # Errors
///
/// Returns an error when no provider identity is installed, the environment
/// profile cannot be resolved, or the durable merge fails. Relay failures are
/// reported in the outcome instead.
pub(crate) async fn reconcile_now(context: &crate::DaemonContext) -> ServiceResult<RefreshOutcome> {
    let provider_pubkey = crate::identity::load_provider_identity(&context.database).await?;
    let relays = context
        .args
        .manifold_environment
        .profile()
        .map_err(|error| internal_error(format!("resolve the environment relay profile: {error}")))?
        .nostr_relays()
        .as_urls()
        .iter()
        .map(|relay| Url(relay.to_string()))
        .collect::<Vec<_>>();

    let outcome = refresh(
        &context.database,
        context.holder_authorization_fetcher.as_ref(),
        &provider_pubkey,
        &relays,
    )
    .await?;
    *context.holder_authorization_read.write().await =
        LastRelayRead::from_outcome(&outcome, crate::now_timestamp());
    // Enrollment is what readiness waits on, so a reconciliation that admits
    // the first authorization must publish rather than wait for another
    // trigger.
    crate::advertisement::reconcile_after_config_change(context).await?;
    Ok(outcome)
}

/// Reconcile once when the runtime starts, after a provider identity exists.
///
/// Without this, a provider only ever reads the relay when an operator presses
/// refresh in a console. A deployment whose operator never opens that screen
/// never reads at all, and a restarted one reports "checking" indefinitely
/// while the authorization a Holder published was readable the whole time.
/// Readiness waits on enrollment, so that is the difference between advertising
/// and not.
///
/// This reads once rather than polling: an authorization is published once and
/// retained durably, so there is nothing to poll for. An operator who expects a
/// newly published one still presses refresh.
pub(crate) async fn run_initial_read_task(context: crate::DaemonContext) -> anyhow::Result<()> {
    let mut identity_installed = context.identity_installed.subscribe();
    while !*identity_installed.borrow() {
        tokio::select! {
            changed = identity_installed.changed() => {
                changed.map_err(|_| anyhow::anyhow!("provider identity watch closed"))?;
            }
            () = context.shutdown.cancelled() => return Ok(()),
        }
    }

    // Raced against shutdown, not merely started before it. Teardown awaits
    // every background task before releasing the data dir, and a relay connect
    // costs up to `FETCH_TIMEOUT` against an unreachable host — so an
    // un-raced read would make process shutdown and, worse, a live restore
    // wait on the network.
    tokio::select! {
        () = context.shutdown.cancelled() => return Ok(()),
        result = reconcile_now(&context) => match result {
            Ok(outcome) => {
                tracing::info!(
                    relays_answered = outcome.relays_answered,
                    candidates_verified = outcome.candidates_verified,
                    "initial Holder-authorization read completed"
                );
                context.record_worker_success(Worker::HolderAuthorizationInitialRead).await;
            }
            Err(error) => {
                tracing::warn!(?error, "initial Holder-authorization read failed");
                context
                    .record_worker_failure(Worker::HolderAuthorizationInitialRead, error.to_string())
                    .await;
            }
        },
    }
    Ok(())
}

/// The inline advertisement trust envelopes for the active provider key.
///
/// Each envelope carries the three bindings [`verify_candidate`] establishes
/// and nothing more. This provider does **not** judge the badge before
/// publishing it: the credential's issuer proof, holder binding, schema, and
/// revocation state are not consumed here, exactly as FMan's advertisement path
/// does not consume them
/// ([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)).
/// Complete envelope verification belongs to the relying consumer, and
/// [SPEC-flip-advertisement](../specs/SPEC-flip-advertisement.md) already
/// requires the app to run it before sending private federation details.
///
/// Two consequences follow, and are accepted as the cost of running one
/// enrollment protocol across both services. A badge revoked after enrollment
/// stays in this provider's advertisement until the retained authorization is
/// replaced, and any key willing to sign can put an envelope here. Neither
/// becomes trust: a consumer that skips its own verification was already
/// unsafe against an FMan advertisement.
///
/// # Errors
///
/// Returns an error when the provider pubkey is malformed or a retained row
/// cannot be read.
pub async fn provider_trust_envelopes(
    database: &Database,
    provider_pubkey: &Pubkey,
) -> ServiceResult<Vec<HolderAuthorizationEnvelope>> {
    Ok(load_verified(database, provider_pubkey)
        .await?
        .into_iter()
        .map(|candidate| candidate.envelope)
        .collect())
}

fn parse_provider_pubkey(provider_pubkey: &Pubkey) -> ServiceResult<PublicKey> {
    PublicKey::parse(&provider_pubkey.0)
        .map_err(|error| internal_error(format!("malformed provider pubkey: {error}")))
}

#[cfg(test)]
#[path = "../tests/holder_authorization.rs"]
mod tests;
