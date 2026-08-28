//! All Nostr behavior for a Fleet Manager.
//!
//! Decoupled from the fleet's verb machinery: each cycle takes a read-only
//! snapshot of the fleet (availability and plans), combines it with durably
//! enrolled Holder authorizations, and publishes the portable signed
//! advertisement document. Enrollment is the Holder publishing a kind-37705
//! authorization for this FMan's pubkey followed by the operator UI requesting
//! one bounded, verified relay reconciliation.
//!
//! The published document is the shared typed rendering of the
//! `SPEC-fman-nostr-events` schema (`crates/nostr/specs/`) — that record is
//! the cross-program contract. The document types live in
//! `fedi_decentralized_nostr::fman` (re-exported below) so this publisher and
//! the in-repo FI consumer depend on one definition instead of two.

pub mod backup;
pub mod format;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
pub use fedi_decentralized_domain::{AdmittedSetupPaymentFederations, HolderAuthorizationEnvelope};
use fedi_decentralized_domain::{FMAN_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS, ProtocolV1};
use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr::fman::HolderAuthorizationEventContent;
pub use fedi_decentralized_nostr::fman::{
    AdvertisementDocument, AdvertisementPayload, ApiEndpoint, Availability,
    IROH_API_ENDPOINT_TRANSPORT, IROH_API_ENDPOINT_URL_SCHEME, sign_advertisement,
    verify_advertisement_self_signature,
};
use fedi_decentralized_nostr::setup_payment_federations::{
    AdmittedSetupPaymentFederationsEvent, admit_setup_payment_federations_event,
    restore_durably_admitted_setup_payment_federations_event,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedi_decentralized_service_fleet_manager::{FEDERATION_SIZES_0_1, FEDIMINTD_VERSION_0_1};
use fman_core::directory::{AdvertisementSnapshot, DirectoryPresence, OnboardingStatus};
use fman_core::fleet::{
    FleetHolderAuthorizationStore, FleetNostrHost, FleetSetupPaymentPolicyStore,
};
use fman_core::identity::RootMnemonic;
use fman_core::onboarding::{FetchedHolderAuthorization, HolderAuthorizationFetcher};
use nostr_sdk::{
    Event, EventBuilder, Filter, JsonUtil, Keys, Kind, PublicKey, RelayUrl, Tag, Timestamp,
};
use tokio::sync::watch;

/// Advertisement republish cadence. fi-client's consumer-side
/// `FMAN_ADVERTISEMENT_MAX_AGE` is derived from this value (4x the cadence),
/// so revisit both together when changing it.
const REPUBLISH_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Retry cadence for the initial relay connection, matching the poll loops:
/// nothing runs until it succeeds, so waiting a republish interval would
/// silence Nostr for half an hour over a relay that was briefly down at start.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_secs(15);
const SETUP_PAYMENT_POLL_INTERVAL: Duration = Duration::from_secs(15 * 60);
const LOCAL_E2E_SETUP_PAYMENT_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Both the relay-side filter limit and the local cap on one fetch: at most
/// one candidate replaces the retained event per refresh, so this only needs
/// enough headroom for a relay serving stale duplicates alongside it.
const SETUP_PAYMENT_FETCH_LIMIT: u16 = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One bounded, operator-driven Holder enrollment read usable before a Fleet
/// is opened.
pub struct NostrHolderAuthorizationFetcher {
    relay_urls: Vec<RelayUrl>,
    store: Arc<FleetHolderAuthorizationStore>,
}

impl NostrHolderAuthorizationFetcher {
    pub fn new(relay_urls: Vec<RelayUrl>, store: Arc<FleetHolderAuthorizationStore>) -> Self {
        Self { relay_urls, store }
    }
}

#[async_trait::async_trait]
impl HolderAuthorizationFetcher for NostrHolderAuthorizationFetcher {
    async fn retained(
        &self,
        identity: &RootMnemonic,
    ) -> anyhow::Result<Vec<HolderAuthorizationEnvelope>> {
        load_retained_holder_authorizations(
            &self.store,
            identity.derive_service_nostr_keys().public_key(),
        )
        .await
    }

    async fn fetch(
        &self,
        identity: &RootMnemonic,
    ) -> anyhow::Result<(Vec<FetchedHolderAuthorization>, u64)> {
        let keys = identity.derive_service_nostr_keys();
        // The authorization may sit on any profile relay: the Holder
        // publishes to every relay it knows, so one reachable relay is
        // enough to find it.
        let nostr = NostrRelayClient::connect_pool(&self.relay_urls, keys.clone(), REQUEST_TIMEOUT)
            .await
            .context("connect to the configured Nostr relays")?;
        let authorizations = fetch_holder_authorizations(&keys, &nostr)
            .await?
            .into_iter()
            .map(|candidate| FetchedHolderAuthorization {
                credential_digest: candidate.credential_digest,
                authorization_issued_at: candidate.authorization_issued_at,
                event_json: candidate.event.as_json(),
                authorization: candidate.authorization,
            })
            .collect();
        Ok((
            authorizations,
            holder_authorization_max_issued_at(now_secs())?,
        ))
    }
}

/// Verify one discovered kind-37705 candidate against the FMan pubkey.
pub fn verify_candidate(
    event: &Event,
    fman_pubkey: &PublicKey,
) -> anyhow::Result<HolderAuthorizationEnvelope> {
    let max_issued_at = holder_authorization_max_issued_at(now_secs())?;
    verify_candidate_at(event, fman_pubkey, max_issued_at)
}

/// Verify one discovered kind-37705 candidate against the FMan pubkey and a
/// receiver-computed maximum authorization issue time.
pub fn verify_candidate_at(
    event: &Event,
    fman_pubkey: &PublicKey,
    max_issued_at: u64,
) -> anyhow::Result<HolderAuthorizationEnvelope> {
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
        .map_err(|_| anyhow::anyhow!("holder authorization proof invalid"))?;
    anyhow::ensure!(
        statement.holder_id_pubkey.0 == event.pubkey,
        "authorization statement holder does not match event author"
    );
    anyhow::ensure!(
        statement.subject_pubkey.0 == *fman_pubkey,
        "authorization subject is not this FMan"
    );
    anyhow::ensure!(
        statement.issued_at.0 <= max_issued_at,
        "authorization issue time exceeds the receiver limit"
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
    Ok(content.authorization)
}

fn holder_authorization_max_issued_at(now: u64) -> anyhow::Result<u64> {
    now.checked_add(FMAN_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS)
        .ok_or_else(|| anyhow::anyhow!("receiver time cannot represent authorization skew"))
}

fn holders_of(authorizations: &[HolderAuthorizationEnvelope]) -> Vec<PublicKey> {
    let mut holders = authorizations
        .iter()
        .map(|authorization| {
            authorization
                .holder_authorization
                .authorization
                .holder_id_pubkey
                .0
        })
        .collect::<Vec<_>>();
    holders.sort_by_key(ToString::to_string);
    holders.dedup();
    holders
}

/// What a completed read concludes, from the retained set it produced.
fn observed_status(
    authorizations: &[HolderAuthorizationEnvelope],
    checked_at: Option<u64>,
) -> OnboardingStatus {
    if authorizations.is_empty() {
        match checked_at {
            // A read completed and found nothing. That is a fact, and it is
            // the state a fleet waits in for a Holder to sign.
            Some(checked_at) => OnboardingStatus::NotObserved { checked_at },
            // Nothing retained and no read yet: the honest answer is that we
            // do not know.
            None => OnboardingStatus::Checking,
        }
    } else {
        OnboardingStatus::AuthorizationObserved {
            authorizations: authorizations.len(),
            holders: holders_of(authorizations),
            checked_at,
        }
    }
}

/// Stateful owner of every FMan Nostr interaction.
#[derive(Clone)]
pub struct FleetManagerNostr {
    inner: Arc<Inner>,
}

struct Inner {
    /// Owns the relay routing: the boundary publishes to and reads from
    /// every canonical relay the resolved profile lists.
    manifold_environment: ManifoldEnvironmentProfile,
    keys: Keys,
    /// `None` when the environment carries no setup-payment publisher
    /// (production until Fedi's deployment-owned key exists): the policy
    /// watch then stays `None` and no fetch runs, so paid setup is
    /// impossible rather than trusting a substitute key.
    setup_payment_publisher: Option<PublicKey>,
    presence: watch::Sender<DirectoryPresence>,
    holder_authorizations: watch::Sender<Vec<HolderAuthorizationEnvelope>>,
    setup_payment_federations: watch::Sender<Option<AdmittedSetupPaymentFederations>>,
    started: AtomicBool,
}

impl FleetManagerNostr {
    /// Construct the FMan Nostr boundary.
    ///
    /// Takes no daemon handles and does no I/O: the policy the watch starts
    /// from is loaded by the caller ([`load_retained_setup_payment_policy`])
    /// before construction, and the daemon side arrives at [`Self::start`].
    ///
    /// The resolved environment supplies the relay routing: every canonical
    /// relay it lists becomes part of the publish/read pool.
    pub fn new(
        keys: Keys,
        setup_payment_publisher: Option<PublicKey>,
        retained_holder_authorizations: Vec<HolderAuthorizationEnvelope>,
        retained_setup_payment_federations: Option<AdmittedSetupPaymentFederations>,
        manifold_environment: ManifoldEnvironmentProfile,
    ) -> Self {
        let latest_fman_version = retained_setup_payment_federations
            .as_ref()
            .map(|policy| policy.fman_version().clone());
        let (presence, _) = watch::channel(DirectoryPresence {
            service_nostr_pubkey: keys.public_key(),
            // No read has happened yet, so a retained authorization reports
            // itself with no check time rather than borrowing one.
            onboarding: observed_status(&retained_holder_authorizations, None),
            latest_fman_version,
        });
        let (holder_authorizations, _) = watch::channel(retained_holder_authorizations);
        let (setup_payment_federations, _) = watch::channel(retained_setup_payment_federations);
        Self {
            inner: Arc::new(Inner {
                manifold_environment,
                keys,
                setup_payment_publisher,
                presence,
                holder_authorizations,
                setup_payment_federations,
                started: AtomicBool::new(false),
            }),
        }
    }

    /// Start the relay loops against the running daemon.
    pub fn start(
        &self,
        host: Arc<FleetNostrHost>,
        setup_payment_store: Arc<FleetSetupPaymentPolicyStore>,
    ) -> tokio::task::JoinHandle<()> {
        assert!(
            !self.inner.started.swap(true, Ordering::AcqRel),
            "FleetManagerNostr may only be started once"
        );
        let inner = self.inner.clone();
        tokio::spawn(async move { inner.run(host, setup_payment_store).await })
    }

    /// This FMan's directory presence, as the runtime last observed it: the
    /// admin socket's read, and the way a caller watches for authorization
    /// arriving rather than sampling for it.
    pub fn presence(&self) -> watch::Receiver<DirectoryPresence> {
        self.inner.presence.subscribe()
    }

    /// The durably enrolled Holder authorizations retained by this FMan.
    ///
    /// These are the same envelopes the advertisement loop embeds. The runtime
    /// refreshes once at startup; the operator UI explicitly drives later
    /// refreshes. Relying verifiers still run fresh issuer-policy and revocation
    /// checks, so retention cannot turn a revoked badge into a passing one.
    ///
    /// Empty when no authorization is durably retained.
    pub fn holder_authorizations(&self) -> Vec<HolderAuthorizationEnvelope> {
        self.inner.holder_authorizations.borrow().clone()
    }

    pub fn subscribe_setup_payment_federations(
        &self,
    ) -> watch::Receiver<Option<AdmittedSetupPaymentFederations>> {
        self.inner.setup_payment_federations.subscribe()
    }
}

impl Inner {
    async fn run(
        self: Arc<Self>,
        host: Arc<FleetNostrHost>,
        setup_payment_store: Arc<FleetSetupPaymentPolicyStore>,
    ) {
        let nostr = loop {
            // The environment profile owns this public routing value. Staging
            // and Production pin it; only Development permits the profile
            // override. One reachable relay is enough to start: the pool
            // keeps reconnecting the rest in the background.
            match NostrRelayClient::connect_pool(
                self.manifold_environment.nostr_relays().as_urls(),
                self.keys.clone(),
                REQUEST_TIMEOUT,
            )
            .await
            {
                Ok(nostr) => break nostr,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "connect to the configured nostr relay failed; retrying"
                    );
                    tracing::warn!(
                        safe_to_share = true,
                        stage = "nostr_relay_connection",
                        failure_kind = "connect_failed",
                        "connect to the configured nostr relay failed; retrying"
                    );
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
            }
        };
        tokio::join!(
            run_advertisements(self.clone(), host, nostr.clone()),
            run_setup_payment_refreshes(self, setup_payment_store, nostr),
        );
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the epoch")
        .as_secs()
}

/// Render one daemon snapshot as an unsigned payload.
fn build_payload(snapshot: AdvertisementSnapshot, keys: &Keys) -> AdvertisementPayload {
    let now = now_secs();
    AdvertisementPayload {
        version: ProtocolV1,
        fman_id_pubkey: keys.public_key().to_string(),
        service_pubkey: snapshot.service_pubkey.to_string(),
        issued_at: now,
        expires_at: now + 2 * REPUBLISH_INTERVAL.as_secs(),
        api_endpoints: vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!(
                "{IROH_API_ENDPOINT_URL_SCHEME}{}",
                snapshot.iroh_endpoint_id
            ),
        }],
        availability: Availability {
            fedimintd_versions: vec![FEDIMINTD_VERSION_0_1.to_owned()],
            federation_sizes: FEDERATION_SIZES_0_1.to_vec(),
        },
        plans: snapshot.plans,
        holder_authorizations: Vec::new(),
    }
}

async fn run_advertisements(inner: Arc<Inner>, host: Arc<FleetNostrHost>, nostr: NostrRelayClient) {
    let mut authorizations = inner.holder_authorizations.subscribe();
    loop {
        let current = authorizations.borrow().clone();
        if let Err(err) = advertise_once(&inner, &host, &nostr, current).await {
            tracing::warn!(?err, "FMan advertisement cycle failed");
        }
        tokio::select! {
            () = tokio::time::sleep(REPUBLISH_INTERVAL) => {}
            () = host.advertisement_changed() => {}
            changed = authorizations.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

async fn run_setup_payment_refreshes(
    inner: Arc<Inner>,
    store: Arc<FleetSetupPaymentPolicyStore>,
    nostr: NostrRelayClient,
) {
    let Some(publisher) = inner.setup_payment_publisher else {
        return;
    };
    let poll_interval = if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
        LOCAL_E2E_SETUP_PAYMENT_POLL_INTERVAL
    } else {
        SETUP_PAYMENT_POLL_INTERVAL
    };
    loop {
        match refresh_setup_payment_federations(&nostr, publisher, &store).await {
            Ok(Some(policy)) => {
                let admitted = policy.set();
                let latest_fman_version = admitted.fman_version().clone();
                inner.presence.send_if_modified(|presence| {
                    if presence.latest_fman_version.as_ref() == Some(&latest_fman_version) {
                        false
                    } else {
                        presence.latest_fman_version = Some(latest_fman_version.clone());
                        true
                    }
                });
                inner.setup_payment_federations.send_if_modified(|current| {
                    if current.as_ref() == Some(admitted) {
                        false
                    } else {
                        *current = Some(admitted.clone());
                        true
                    }
                });
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(?err, "setup-payment policy refresh failed"),
        }
        tokio::time::sleep(poll_interval).await;
    }
}

async fn advertise_once(
    inner: &Inner,
    host: &FleetNostrHost,
    nostr: &NostrRelayClient,
    holder_authorizations: Vec<HolderAuthorizationEnvelope>,
) -> anyhow::Result<()> {
    let Some(snapshot) = host.advertisement().await else {
        return Ok(());
    };
    let mut payload = build_payload(snapshot, &inner.keys);
    payload.holder_authorizations = holder_authorizations;

    let document = sign_advertisement(payload, &inner.keys)?;
    let content = serde_json::to_string(&document).context("serialize advertisement")?;
    nostr
        .publish_event(
            EventBuilder::new(
                Kind::Custom(fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_EVENT_KIND),
                content,
            )
            .tags([
                Tag::parse([
                    "d",
                    fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_D_TAG,
                ])
                .expect("valid FMan advertisement d tag"),
                Tag::parse([
                    "t",
                    fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_HASHTAG,
                ])
                .expect("valid FMan advertisement hashtag"),
            ]),
        )
        .await
        .map_err(|err| anyhow::anyhow!("publish advertisement: {err}"))?;
    tracing::info!(
        safe_to_share = true,
        holder_authorizations = document.payload.holder_authorizations.len(),
        "published FMan advertisement"
    );
    Ok(())
}

struct VerifiedHolderAuthorizationEvent {
    event: Event,
    authorization: HolderAuthorizationEnvelope,
    credential_digest: Vec<u8>,
    authorization_issued_at: u64,
}

fn verified_holder_authorization_event(
    event: Event,
    fman_pubkey: &PublicKey,
    max_issued_at: u64,
) -> anyhow::Result<VerifiedHolderAuthorizationEvent> {
    let authorization = verify_candidate_at(&event, fman_pubkey, max_issued_at)?;
    let statement = &authorization.holder_authorization.authorization;
    Ok(VerifiedHolderAuthorizationEvent {
        credential_digest: statement.credential_digest.0.to_vec(),
        authorization_issued_at: statement.issued_at.0,
        event,
        authorization,
    })
}

async fn fetch_holder_authorizations(
    keys: &Keys,
    nostr: &NostrRelayClient,
) -> anyhow::Result<Vec<VerifiedHolderAuthorizationEvent>> {
    let max_issued_at = holder_authorization_max_issued_at(now_secs())?;
    let filter = Filter::new()
        .kind(Kind::Custom(
            fedi_decentralized_nostr::fman::HOLDER_AUTHORIZATION_EVENT_KIND,
        ))
        .pubkey(keys.public_key())
        .hashtag(fedi_decentralized_nostr::fman::FMAN_AUTHORIZATION_HASHTAG)
        .limit(64);
    let candidates = nostr
        .fetch_events_capped(filter, REQUEST_TIMEOUT, 64)
        .await
        .map_err(|err| anyhow::anyhow!("fetch holder authorizations: {err}"))?;
    let mut authorizations = std::collections::BTreeMap::new();
    for event in candidates {
        match verified_holder_authorization_event(event, &keys.public_key(), max_issued_at) {
            Ok(candidate) => {
                let replace = authorizations.get(&candidate.credential_digest).is_none_or(
                    |current: &VerifiedHolderAuthorizationEvent| {
                        candidate.authorization_issued_at > current.authorization_issued_at
                    },
                );
                if replace {
                    authorizations.insert(candidate.credential_digest.clone(), candidate);
                }
            }
            Err(_) => tracing::debug!("skipping invalid Holder authorization candidate"),
        }
    }
    Ok(authorizations.into_values().collect())
}

/// Revalidate every event retained by the fleet before seeding the runtime.
pub async fn load_retained_holder_authorizations(
    store: &FleetHolderAuthorizationStore,
    fman_pubkey: PublicKey,
) -> anyhow::Result<Vec<HolderAuthorizationEnvelope>> {
    let max_issued_at = holder_authorization_max_issued_at(now_secs())?;
    decode_retained_holder_authorizations(
        store.load(max_issued_at).await?,
        fman_pubkey,
        max_issued_at,
    )
}

fn decode_retained_holder_authorizations(
    event_jsons: Vec<String>,
    fman_pubkey: PublicKey,
    max_issued_at: u64,
) -> anyhow::Result<Vec<HolderAuthorizationEnvelope>> {
    event_jsons
        .into_iter()
        .map(|json| {
            let event = Event::from_json(json).context("parse retained Holder authorization")?;
            verified_holder_authorization_event(event, &fman_pubkey, max_issued_at)
                .map(|candidate| candidate.authorization)
                .context("revalidate retained Holder authorization")
        })
        .collect()
}

async fn refresh_setup_payment_federations(
    nostr: &NostrRelayClient,
    publisher: PublicKey,
    store: &FleetSetupPaymentPolicyStore,
) -> anyhow::Result<Option<AdmittedSetupPaymentFederationsEvent>> {
    let filter = Filter::new()
        .kind(Kind::Custom(
            fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
        ))
        .author(publisher)
        .identifier(
            fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
        )
        .limit(usize::from(SETUP_PAYMENT_FETCH_LIMIT));
    let candidates = match nostr
        .fetch_events_capped(filter, REQUEST_TIMEOUT, SETUP_PAYMENT_FETCH_LIMIT)
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            tracing::warn!(
                ?err,
                "fetching setup-payment policy failed; using stored policy"
            );
            Vec::new()
        }
    };
    admit_setup_payment_federations(store, publisher, candidates, Timestamp::now()).await
}

/// Load the retained setup-payment policy, admitting it under the current
/// publisher. Startup's read, before the boundary exists: the result is what
/// [`FleetManagerNostr::new`] starts its watch from.
///
/// # Errors
///
/// Fails when the retained event cannot be decoded or no longer authenticates
/// under `publisher` — a rotated publisher must stop the daemon, not silently
/// downgrade it to no policy.
pub async fn load_retained_setup_payment_policy(
    store: &FleetSetupPaymentPolicyStore,
    publisher: PublicKey,
) -> anyhow::Result<Option<AdmittedSetupPaymentFederations>> {
    Ok(
        admit_setup_payment_federations(store, publisher, Vec::new(), Timestamp::now())
            .await?
            .map(|policy| policy.set().clone()),
    )
}

async fn admit_setup_payment_federations(
    store: &FleetSetupPaymentPolicyStore,
    publisher: PublicKey,
    candidates: Vec<Event>,
    now: Timestamp,
) -> anyhow::Result<Option<AdmittedSetupPaymentFederationsEvent>> {
    let stored_json = store.load().await.context("load setup-payment policy")?;
    let admission = admit(stored_json, publisher, candidates, now)?;
    if let Some((event_json, admitted)) = admission.retain {
        store
            .replace(&event_json, &admitted)
            .await
            .context("retain setup-payment policy")?;
    }
    Ok(admission.admitted)
}

/// The retained event, the candidates, and what the store must be told —
/// decided without touching the store, so the decision is testable on its own.
#[derive(Debug)]
struct Admission {
    admitted: Option<AdmittedSetupPaymentFederationsEvent>,
    /// `Some` only when the winner differs from what was already retained.
    retain: Option<(String, AdmittedSetupPaymentFederations)>,
}

fn admit(
    stored_json: Option<String>,
    publisher: PublicKey,
    candidates: Vec<Event>,
    now: Timestamp,
) -> anyhow::Result<Admission> {
    let stored = stored_json
        .map(|json| serde_json::from_str::<Event>(&json))
        .transpose()
        .context("decode stored setup-payment policy")?;
    let mut admitted = stored
        .as_ref()
        .map(|event| restore_durably_admitted_setup_payment_federations_event(event, publisher))
        .transpose()
        .context("restore setup-payment policy")?;
    for candidate in candidates {
        if let Ok(candidate) =
            admit_setup_payment_federations_event(&candidate, publisher, now, admitted.as_ref())
        {
            admitted = Some(candidate);
        }
    }
    let retain = match admitted {
        Some(ref admitted)
            if stored
                .as_ref()
                .is_none_or(|event| event.id != admitted.event().id) =>
        {
            Some((
                serde_json::to_string(admitted.event())?,
                admitted.set().clone(),
            ))
        }
        _ => None,
    };
    Ok(Admission { admitted, retain })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
