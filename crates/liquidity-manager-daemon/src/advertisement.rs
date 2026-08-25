//! Provider advertisement build, publication, withdrawal, and readiness.
//!
//! The advertisement is FLIP's only public ready signal: it is published to the
//! configured Nostr relays while setup and dependency validation pass, and
//! withdrawn when they stop passing. A durable `withdrawn_at` keeps an operator
//! withdrawal from being undone by an automatic republication. See
//! [SPEC-flip-advertisement](../specs/SPEC-flip-advertisement.md).

use std::time::Duration;

use fedi_decentralized_service_liquidity_manager::{
    AdvertisementPublicationStatus, ComponentHealth, GetAdvertisementStateResponse, HealthStatus,
    LiquidityProviderAdvertisement, PUBLIC_LIQUIDITY_API_ALPN,
    PUBLIC_LIQUIDITY_PROTOCOL_VERSION as PROTOCOL_VERSION, ProviderPolicy, RelayPublicationState,
    RelayStatus, RpcTransport, ServiceResult, SetupStatus, Signed, Timestamp, Url,
    canonical_json_payload,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::warn;

use crate::DaemonContext;
use crate::daemon::Worker;
use crate::identity;
use crate::nostr::{RelayPublishRequest, RelayWithdrawRequest};
use crate::setup_store;
use crate::{failed_precondition, internal_error, now_timestamp};

const ADVERTISEMENT_ROW_ID: i64 = 1;
const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// Current publication state stored in SQLite.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AdvertisementRecord {
    pub status: AdvertisementPublicationStatus,
    pub advertisement: Option<Signed<LiquidityProviderAdvertisement>>,
    pub last_published_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,

    /// When the operator last took the advertisement off the relays.
    ///
    /// Set by `withdraw`, cleared by a publication. While it is set the
    /// publisher leaves the provider off the market; only an explicit operator
    /// republish (`force`) puts it back. This is the whole of the withdrawal's
    /// durability: the `Withdrawn` status is a report of what happened, and the
    /// publisher overwrites it on its next pass.
    pub withdrawn_at: Option<Timestamp>,
    pub relay_states: Vec<RelayPublicationState>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PublicReadiness {
    pub ready: bool,
    pub reason: Option<String>,
    pub setup_status: SetupStatus,
    pub recovery_complete: bool,
    pub advertisement_enabled: bool,
}

pub(crate) async fn get_state(
    context: &DaemonContext,
) -> ServiceResult<GetAdvertisementStateResponse> {
    let readiness = public_readiness(context).await?;
    let setup = setup_store::load_setup_state(&context.database).await?;
    let record = load_advertisement_record(context).await?;
    let unverified_holder_authorization_count =
        unverified_published_envelopes(context, record.advertisement.as_ref()).await;
    Ok(GetAdvertisementStateResponse {
        advertisement: if readiness.ready {
            record.advertisement
        } else {
            None
        },
        publication_status: if readiness.ready {
            record.status
        } else {
            AdvertisementPublicationStatus::NotReady
        },
        last_published_at: record.last_published_at,
        expires_at: record.expires_at,
        withdrawn_at: record.withdrawn_at,
        relay_states: record.relay_states,
        ready: readiness.ready,
        readiness: setup.validation,
        unverified_holder_authorization_count,
    })
}

/// Counts published holder authorizations that no longer verify.
///
/// `republish` builds its envelopes from `provider_trust_envelopes`, which
/// re-verifies every retained event on read. Nothing did that on the way back
/// out: `get_state` returned whatever the `provider_advertisements` row held,
/// so without this the Admin API would present an envelope as published without
/// any statement that it still verifies.
///
/// The published payload is not edited. It is signed over exactly the envelopes
/// it carries, so removing one would leave a response whose proof does not
/// check. Reporting the count says the same thing without breaking the
/// signature.
///
/// Every failure path counts as unverified. If the provider identity cannot be
/// read, or the enrolled set cannot be loaded, then nothing here has verified
/// anything, and saying "0 unverified" would be the one answer that is wrong.
async fn unverified_published_envelopes(
    context: &DaemonContext,
    advertisement: Option<&Signed<LiquidityProviderAdvertisement>>,
) -> u32 {
    let Some(advertisement) = advertisement else {
        return 0;
    };
    let published = &advertisement.payload.holder_authorizations;
    if published.is_empty() {
        return 0;
    }
    let count = u32::try_from(published.len()).unwrap_or(u32::MAX);

    let Ok(provider_pubkey) = identity::load_provider_identity(&context.database).await else {
        return count;
    };
    let Ok(verified) =
        crate::holder_authorization::provider_trust_envelopes(&context.database, &provider_pubkey)
            .await
    else {
        return count;
    };
    u32::try_from(
        published
            .iter()
            .filter(|envelope| !verified.contains(envelope))
            .count(),
    )
    .unwrap_or(u32::MAX)
}

pub(crate) async fn republish(
    context: &DaemonContext,
    force: bool,
) -> ServiceResult<AdvertisementRecord> {
    let readiness = public_readiness(context).await?;
    if !readiness.ready {
        persist_not_ready(context, &readiness).await?;
        return load_advertisement_record(context).await;
    }

    // An operator who withdrew stays withdrawn until they say otherwise.
    //
    // Withdrawal only ever moved local status and expired the relay events; it
    // changed no configuration, and readiness is derived from configuration, so
    // the publisher's next pass found the deployment ready and republished
    // under a fresh signature. Every trigger did it: the reconcile tick, the
    // five config verbs, holder-authorization refresh, and "refresh relays".
    // The operator saw `Withdrawn`, believed they were off the market, and were
    // back on it — with the status flipping back by itself and nothing
    // recording that they had asked to leave.
    //
    // `force` is the operator saying otherwise. It is what the Republish verb
    // sends and what the dashboard's Republish control sets; the automatic
    // paths all pass `false`. Gating here rather than inside `public_readiness`
    // is deliberate: readiness answers "is this deployment fit to advertise",
    // which withdrawal does not change, and a reason threaded through it would
    // be overwritten by the same loop one tick later.
    let record = load_advertisement_record(context).await?;
    if !force && record.withdrawn_at.is_some() {
        return Ok(record);
    }

    let setup = setup_store::load_setup_state(&context.database).await?;
    let config = setup
        .config
        .ok_or_else(|| failed_precondition("setup config is not configured"))?;
    // One handle for the whole publication: the advertisement signature and
    // the relay event must come from the same key.
    let auth_provider = context.auth_provider().await;
    let provider_pubkey = identity::load_provider_identity(&context.database).await?;
    let issued_at = now_timestamp();
    let holder_authorizations =
        crate::holder_authorization::provider_trust_envelopes(&context.database, &provider_pubkey)
            .await?;
    if let Some(display) = &config.provider_display {
        display.validate().map_err(|error| {
            failed_precondition(format!("provider display metadata is invalid: {error}"))
        })?;
    }
    let expires_at = Timestamp(
        issued_at
            .0
            .saturating_add(config.advertisement.republish_interval.0.saturating_mul(2)),
    );
    let advertisement = LiquidityProviderAdvertisement {
        version: PROTOCOL_VERSION,
        provider_pubkey: provider_pubkey.clone(),
        issued_at,
        expires_at,
        supported_sources: config.capacity.supported_sources.clone(),
        holder_authorizations,
        policy: config.policy.clone(),
        display: config.provider_display.clone(),
        api_endpoints: vec![advertised_endpoint_url(
            &config.advertised_endpoint.address.0,
        )],
        api_versions: vec![PROTOCOL_VERSION],
        relay_hints: config.relays.clone(),
    };
    // Sign only when the payload actually changed: Schnorr signing is
    // non-deterministic, so re-signing identical content (e.g. two republish
    // triggers within the same second) would publish a different proof than
    // the persisted advertisement and break byte-identical replaceable-event
    // updates on relays.
    let stored = load_advertisement_record(context).await?;
    let previous_status = stored.status;
    let payload_unchanged = stored
        .advertisement
        .as_ref()
        .is_some_and(|existing| existing.payload == advertisement);
    let signed = match stored.advertisement {
        Some(existing) if payload_unchanged => existing,
        _ => auth_provider.sign_advertisement(advertisement)?,
    };
    let hash = fedi_decentralized_service_liquidity_manager::advertisement_hash(&signed.payload)
        .map_err(internal_error)?;
    let signed_json = String::from_utf8(canonical_json_payload(&signed).map_err(internal_error)?.0)
        .map_err(internal_error)?;
    let readiness_json = serde_json::to_string(&readiness).map_err(internal_error)?;
    let signed_storage_json = serde_json::to_string(&signed).map_err(internal_error)?;

    // Readiness is rechecked here, under the writer, and not only at the top of
    // this function.
    //
    // Checking once at the top is not enough: between that check and the write,
    // the publisher loads config, loads envelopes, validates display metadata,
    // builds and signs — all of it awaiting. `apply_setup_config` and
    // `update_provider_config` commit and then trigger publication, so either
    // can turn readiness false inside that window while this row is still
    // written and published as a readiness assertion.
    //
    // Taking the write transaction *before* the recheck is what makes it a fence
    // rather than a second racy read: `begin_write` is `BEGIN IMMEDIATE`, so no
    // other writer can commit between the recheck's reads and the insert below.
    // The recheck reads on a separate pooled connection, which sees everything
    // committed before the writer was taken.
    //
    // That fences the database-backed readiness inputs. The in-memory ones —
    // daemon phase, recovery, endpoint identity, signing readiness, verification
    // inputs — are not covered by a SQLite lock, so they are fenced by value
    // instead: the snapshot the recheck judged is compared again immediately
    // before the commit, and again before every relay publish below.
    let mut tx = context
        .database
        .begin_write()
        .await
        .map_err(internal_error)?;
    let recheck_inputs = in_memory_readiness_inputs(context).await;
    let recheck = judge_readiness(context, &recheck_inputs).await?;
    if !recheck.ready {
        tx.rollback().await.map_err(internal_error)?;
        persist_not_ready(context, &recheck).await?;
        return load_advertisement_record(context).await;
    }

    sqlx::query(
        "INSERT INTO provider_advertisements \
         (id, status, advertisement_hash, signed_advertisement_json, readiness_json, \
          issued_at, expires_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           status = excluded.status, \
           advertisement_hash = excluded.advertisement_hash, \
           signed_advertisement_json = excluded.signed_advertisement_json, \
           readiness_json = excluded.readiness_json, \
           issued_at = excluded.issued_at, \
           expires_at = excluded.expires_at, \
           withdrawn_at = NULL, \
           last_error = NULL, \
           updated_at = unixepoch()",
    )
    .bind(ADVERTISEMENT_ROW_ID)
    .bind(AdvertisementPublicationStatus::Stale.to_string())
    .bind(hash.0.to_vec())
    .bind(&signed_storage_json)
    .bind(&readiness_json)
    .bind(issued_at.0 as i64)
    .bind(expires_at.0 as i64)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    // The last thing before the commit. Nothing between the recheck above and
    // here awaits on anything outside SQLite, but `daemon_state` takes no
    // SQLite lock, so a phase change or an identity install can still land in
    // that window. Comparing the snapshot closes it down to the commit call
    // itself.
    //
    // *The residual, stated rather than implied:* a change that lands during
    // `tx.commit()` is not caught here. Closing that would need the in-memory
    // inputs to move under the same lock as the row, which they do not — they
    // are process state. What it cannot do is publish: the publish loop below
    // re-checks, and publication is what asserts readiness.
    if in_memory_readiness_inputs(context).await != recheck_inputs {
        tx.rollback().await.map_err(internal_error)?;
        let moved = public_readiness(context).await?;
        persist_not_ready(context, &moved).await?;
        return load_advertisement_record(context).await;
    }
    tx.commit().await.map_err(internal_error)?;

    if force {
        clear_relay_states(context).await?;
    }

    let mut had_failure = false;
    let relay_secret_key = auth_provider.relay_signing_secret_hex();
    let Some(secret_key) = relay_secret_key else {
        mark_all_relays_failed(
            context,
            &config.relays,
            &hash.0,
            "provider identity has no Nostr signing key",
        )
        .await?;
        persist_publication_status(
            context,
            AdvertisementPublicationStatus::Failed,
            Some("provider identity has no Nostr signing key"),
        )
        .await?;
        return load_advertisement_record(context).await;
    };

    for relay_url in &config.relays {
        // Re-checked before every relay, not once before the loop.
        //
        // The published event *is* the readiness assertion, and each relay is a
        // separate one reached after a separate network round trip. A single
        // check before the loop would leave every relay after the first
        // asserting readiness that may have failed while an earlier one was
        // being written to.
        //
        // A daemon that goes not-ready mid-loop stops here rather than
        // continuing. The relays already published to keep their events until
        // expiry, which stopping neither causes nor worsens.
        let publish_inputs = in_memory_readiness_inputs(context).await;
        let still_ready = judge_readiness(context, &publish_inputs).await?;
        if !still_ready.ready {
            persist_not_ready(context, &still_ready).await?;
            return load_advertisement_record(context).await;
        }

        match context
            .relay_publisher
            .publish(RelayPublishRequest {
                relay_url: relay_url.clone(),
                content: signed_json.clone(),
                created_at: signed.payload.issued_at,
                nostr_secret_key_hex: secret_key.clone(),
            })
            .await
        {
            Ok(result) => {
                upsert_relay_state(
                    context,
                    relay_url,
                    RelayStatus::Published,
                    Some(&hash.0),
                    Some(&result.event_id),
                    None,
                    Some(now_timestamp()),
                )
                .await?;
            }
            Err(error) => {
                had_failure = true;
                upsert_relay_state(
                    context,
                    relay_url,
                    RelayStatus::Failed,
                    Some(&hash.0),
                    None,
                    Some(&error),
                    Some(now_timestamp()),
                )
                .await?;
            }
        }
    }

    let outcome = if had_failure {
        AdvertisementPublicationStatus::Failed
    } else {
        AdvertisementPublicationStatus::Published
    };
    persist_publication_status(context, outcome, None).await?;
    // Only a change is news. This runs on a reconcile timer, so an unchanged
    // advertisement re-sent to the same relays would otherwise write the same
    // line every minute for the life of the process.
    if previous_status != outcome || !payload_unchanged {
        match outcome {
            AdvertisementPublicationStatus::Published => tracing::info!(
                relays = config.relays.len(),
                expires_at = signed.payload.expires_at.0,
                resigned = !payload_unchanged,
                "published the provider advertisement"
            ),
            _ => warn!(
                relays = config.relays.len(),
                "advertisement publication failed on at least one relay"
            ),
        }
    }

    load_advertisement_record(context).await
}

pub(crate) async fn withdraw(
    context: &DaemonContext,
    reason: Option<String>,
) -> ServiceResult<AdvertisementRecord> {
    let relay_states = load_relay_states(context).await?;
    // Both are needed to reach a relay at all: the key that authored the
    // publication, and the advertisement to expire. Missing either means
    // nothing was published under this identity for a relay to still be
    // serving, so only local state moves.
    let auth_provider = context.auth_provider().await;
    let relay_secret_key = auth_provider.relay_signing_secret_hex();
    let stored = load_advertisement_record(context).await?;
    // `withdrawn_at`, not the status. Every readiness-driven withdrawal is
    // followed by `persist_not_ready`, which overwrites the status to
    // `NotReady`; reading the status would find no withdrawal on the next pass
    // and announce the same departure every minute. The timestamp is the
    // durable half and survives it.
    let was_withdrawn = stored.withdrawn_at.is_some();
    let expired = match (&relay_secret_key, stored.advertisement) {
        (Some(_), Some(published)) => {
            Some(expire_advertisement(auth_provider.as_ref(), published)?)
        }
        _ => None,
    };

    for relay in relay_states {
        let mut withdraw_error = None;
        if let (Some(secret_key), Some((expired_content, expired_created_at))) =
            (&relay_secret_key, &expired)
            && let Err(error) = context
                .relay_publisher
                .withdraw(RelayWithdrawRequest {
                    relay_url: relay.relay_url.clone(),
                    reason: reason.clone(),
                    nostr_secret_key_hex: secret_key.clone(),
                    expired_content: expired_content.clone(),
                    expired_created_at: *expired_created_at,
                })
                .await
        {
            warn!(relay_url = %relay.relay_url.0, %error, "relay withdraw failed");
            withdraw_error = Some(format!("withdraw failed: {error}"));
        }
        // A withdrawal that failed leaves the event on the relay, so recording
        // `Disconnected` would assert the advertisement is gone when it may not
        // be. The durable row is the only place an operator can see that a
        // non-ready FLIP may still be advertised; a log line is not that.
        let (status, detail) = match &withdraw_error {
            Some(error) => (RelayStatus::Failed, Some(error.as_str())),
            None => (RelayStatus::Disconnected, reason.as_deref()),
        };
        upsert_relay_state(
            context,
            &relay.relay_url,
            status,
            None,
            None,
            detail,
            Some(now_timestamp()),
        )
        .await?;
    }

    persist_publication_status(
        context,
        AdvertisementPublicationStatus::Withdrawn,
        reason.as_deref(),
    )
    .await?;
    // The advertisement is the only public ready signal, so leaving the market
    // is worth a warning — and worth exactly one, however many passes keep the
    // deployment off it.
    if !was_withdrawn {
        warn!(
            reason = reason.as_deref().unwrap_or(""),
            "withdrew the provider advertisement from every relay"
        );
    }
    load_advertisement_record(context).await
}

/// Re-signs the last published advertisement so that it is already expired.
///
/// The withdrawal is the same document the provider was advertising, restated
/// with an `expires_at` that has passed, so a client applying the freshness rule
/// in [SPEC-flip-advertisement](../specs/SPEC-flip-advertisement.md) rejects it
/// without needing to understand a new kind of message. Returns the canonical
/// JSON and the `created_at` the superseding event must carry.
fn expire_advertisement(
    auth_provider: &dyn crate::auth::PublicAuthProvider,
    published: Signed<LiquidityProviderAdvertisement>,
) -> ServiceResult<(String, Timestamp)> {
    // Strictly after the advertisement being replaced, so replaceable-event
    // ordering keeps this one — equal timestamps are resolved by lowest event
    // id, which could otherwise keep the live one.
    //
    // Bounded above at one second from now, because `published.issued_at` is
    // **unverified input**: it is reloaded from `signed_advertisement_json` and
    // nothing re-checks it. A tampered far-future value would put `created_at`
    // past what relays accept, they would reject the expiry, and the live
    // advertisement would keep being served — a withdrawal that silently does
    // not withdraw.
    let now = now_timestamp().0;
    let issued_at = Timestamp(
        now.max(published.payload.issued_at.0.saturating_add(1))
            .min(now.saturating_add(1)),
    );

    // Built from constants and the one field signing actually checks, rather
    // than from the stored payload.
    //
    // Spreading `..published.payload` would supply every remaining field, and
    // each would be re-signed under the live provider key and pushed to every
    // relay while `sign_advertisement` checks only `provider_pubkey`. Two of
    // those fields are live hazards: `display` would bypass the
    // `display.validate()` that `republish` enforces, so an expiry could publish
    // provider-signed metadata the live path refuses; and `persist_not_ready`
    // never clears the stored payload while the publisher task re-runs
    // `withdraw` each interval, so anything left in that column would be
    // re-signed and re-published indefinitely.
    //
    // An expiry needs none of it. It exists to supersede the live advertisement
    // by `issued_at`, and consumers reject it on `expires_at <= now` before
    // reading endpoints or badges.
    let expired = LiquidityProviderAdvertisement {
        version: PROTOCOL_VERSION,
        provider_pubkey: published.payload.provider_pubkey,
        issued_at,
        // Expired at the instant it is issued.
        expires_at: issued_at,
        supported_sources: Vec::new(),
        holder_authorizations: Vec::new(),
        policy: ProviderPolicy {
            accepted_attester_policies: Vec::new(),
            supported_networks: Vec::new(),
        },
        display: None,
        api_endpoints: Vec::new(),
        api_versions: vec![PROTOCOL_VERSION],
        relay_hints: Vec::new(),
    };
    let signed = auth_provider.sign_advertisement(expired)?;
    let content = String::from_utf8(canonical_json_payload(&signed).map_err(internal_error)?.0)
        .map_err(internal_error)?;
    Ok((content, issued_at))
}

pub(crate) async fn refresh_relays(
    context: &DaemonContext,
) -> ServiceResult<Vec<RelayPublicationState>> {
    let readiness = public_readiness(context).await?;
    if readiness.ready {
        let record = republish(context, false).await?;
        Ok(record.relay_states)
    } else {
        persist_not_ready(context, &readiness).await?;
        Ok(load_relay_states(context).await?)
    }
}

pub(crate) async fn reconcile_after_config_change(context: &DaemonContext) -> ServiceResult<()> {
    // Admin calls can arrive after recovery but before the public transport has
    // settled its node id. Treat that as startup still in progress: the
    // publisher is started by the transport and immediately reconciles once
    // this readiness input exists. Withdrawing here can otherwise race that
    // first publish and leave the persisted advertisement withdrawn.
    if context.local_iroh_node_id().await.is_none() {
        return Ok(());
    }
    let readiness = public_readiness(context).await?;
    if readiness.ready {
        republish(context, false).await?;
    } else {
        withdraw(context, readiness.reason.clone()).await?;
        persist_not_ready(context, &readiness).await?;
    }
    Ok(())
}

pub(crate) async fn run_publisher_task(context: DaemonContext) -> anyhow::Result<()> {
    loop {
        match reconcile_after_config_change(&context).await {
            Ok(()) => {
                context
                    .record_worker_success(Worker::AdvertisementPublisher)
                    .await
            }
            Err(error) => {
                warn!(?error, "advertisement reconciliation failed");
                context
                    .record_worker_failure(Worker::AdvertisementPublisher, error.to_string())
                    .await;
            }
        }

        let delay = reconcile_interval(&context).await;
        tokio::select! {
            _ = context.shutdown.cancelled() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// The readiness inputs this daemon holds in process memory rather than in
/// SQLite.
///
/// The database-backed inputs are fenced by taking the writer before the
/// recheck: `begin_write` is `BEGIN IMMEDIATE`, so nothing can commit between
/// the recheck's reads and the insert. A SQLite lock says nothing about process
/// state, so these are fenced by re-reading and comparing instead.
///
/// **Compared by value, not by a generation counter.** A counter is correct only
/// while every writer remembers to bump it, and `daemon_state` has four writers
/// plus the auth-provider swap. Comparing the values cannot drift from the
/// writers. It can only drift from [`judge_readiness`], and the two are built
/// together here.
///
/// `verification_inputs_available` is in the snapshot even though
/// `TrustVerificationProvider::mode` derives it from a field fixed at
/// construction and no runtime path changes it. Carrying it costs a bool and
/// means a later provider that *can* change it is fenced without anyone
/// remembering this.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InMemoryReadinessInputs {
    phase: crate::daemon::DaemonPhase,
    recovery_complete: bool,
    public_iroh_node_id: Option<String>,
    signing_ready: bool,
    verification_inputs_available: bool,
}

/// Reads the in-memory readiness inputs and releases every lock before
/// returning.
///
/// No guard is held across an await on purpose. Tokio's `RwLock` is
/// write-preferring, so holding a `daemon_state` read guard over a call that
/// reads it again deadlocks the moment a writer queues between the two.
pub(crate) async fn in_memory_readiness_inputs(context: &DaemonContext) -> InMemoryReadinessInputs {
    let state = context.daemon_state.read().await.clone();
    InMemoryReadinessInputs {
        phase: state.phase,
        recovery_complete: state.recovery_complete,
        public_iroh_node_id: state.public_iroh_node_id,
        signing_ready: context.auth_provider().await.mode().signing_ready,
        verification_inputs_available: context.verification_provider.mode().inputs_available,
    }
}

pub(crate) async fn public_readiness(context: &DaemonContext) -> ServiceResult<PublicReadiness> {
    let inputs = in_memory_readiness_inputs(context).await;
    judge_readiness(context, &inputs).await
}

/// Judges readiness against a snapshot of the in-memory inputs the caller
/// already took, so a fence can hold that snapshot and compare it later.
pub(crate) async fn judge_readiness(
    context: &DaemonContext,
    state: &InMemoryReadinessInputs,
) -> ServiceResult<PublicReadiness> {
    let setup = setup_store::load_setup_state(&context.database).await?;
    let advertisement_enabled = setup
        .config
        .as_ref()
        .map(|config| config.advertisement.ready_advertisement_enabled)
        .unwrap_or(false);
    let mut reason = None;
    if !state.recovery_complete {
        reason = Some("startup recovery is not complete".to_owned());
    } else if state.phase != crate::daemon::DaemonPhase::Ready {
        reason = Some(format!("daemon phase is {:?}", state.phase));
    } else if setup.status != SetupStatus::Ready {
        reason = Some("setup is not ready".to_owned());
    } else if !advertisement_enabled {
        reason = Some("ready advertisement publication is disabled".to_owned());
    } else if !state.signing_ready {
        reason = Some("provider signing key is not installed".to_owned());
    } else if !state.verification_inputs_available {
        reason = Some(
            "trust verification inputs are unavailable: the invite-code federation \
             preview cannot produce results \
             (test deployments can substitute it with --trust-fixtures)"
                .to_owned(),
        );
    } else if let Some(config) = &setup.config {
        // Derived here rather than trusted from the stored `SetupStatus`.
        // Admin validation refuses an empty policy on the way in, but a config
        // accepted before that check existed — or restored from a backup taken
        // before it — carries a stored `Ready` that no validation pass ever
        // re-examined. Readiness is the last gate before publication, so the
        // invariant has to hold here to hold at all.
        if config.policy.accepted_attester_policies.is_empty() {
            reason = Some("no accepted attester policy is configured".to_owned());
        } else if config.advertisement.republish_interval.0 == 0 {
            // Same reasoning, for the same reason. `expires_at` below is
            // `issued_at + republish_interval * 2`, so publishing at zero
            // emits an advertisement that expired in the instant it was
            // issued. Admin validation now refuses it on the way in, but a
            // config stored before that check existed carries a `Ready` that
            // no validation pass has re-examined, and publishing under it
            // reports success while reaching nobody.
            reason = Some("advertisement republish interval is zero".to_owned());
        } else if config.relays.is_empty() {
            reason = Some("no relays are configured".to_owned());
        } else if !matches!(config.advertised_endpoint.transport, RpcTransport::Iroh) {
            reason = Some("advertised endpoint transport is not Iroh".to_owned());
        } else if config.advertised_endpoint.address.0.trim().is_empty() {
            reason = Some("advertised endpoint address is empty".to_owned());
        } else if state.public_iroh_node_id.as_deref()
            != Some(config.advertised_endpoint.address.0.as_str())
        {
            reason = Some(
                "advertised Iroh endpoint does not match local public endpoint identity".to_owned(),
            );
        }
    } else {
        reason = Some("setup config is not configured".to_owned());
    }

    if reason.is_none() {
        // The spec requires the published advertisement to carry at least one
        // inline holder-authorization envelope. Enrolled, not verified: like
        // FMan, this provider does not judge the badge before publishing it.
        let provider_pubkey = identity::load_provider_identity(&context.database).await?;
        let envelopes = crate::holder_authorization::provider_trust_envelopes(
            &context.database,
            &provider_pubkey,
        )
        .await?;
        if envelopes.is_empty() {
            reason = Some("no Holder authorization is enrolled for this provider".to_owned());
        }
    }

    Ok(PublicReadiness {
        ready: reason.is_none(),
        reason,
        setup_status: setup.status,
        recovery_complete: state.recovery_complete,
        advertisement_enabled,
    })
}

async fn reconcile_interval(context: &DaemonContext) -> Duration {
    setup_store::load_setup_state(&context.database)
        .await
        .ok()
        .and_then(|setup| setup.config)
        .map(|config| config.advertisement.republish_interval.0)
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RECONCILE_INTERVAL)
}

pub(crate) async fn relay_health_component(
    context: &DaemonContext,
    observed_at: Timestamp,
) -> ComponentHealth {
    // "No relay state is configured" and "relay state could not be read" are
    // different conditions, and collapsing them hides a storage fault during
    // the incident this component exists to diagnose. Reported the same way the
    // database component reports its own failure.
    let relay_states = match load_relay_states(context).await {
        Ok(relay_states) => relay_states,
        Err(error) => {
            return ComponentHealth {
                component: fedi_decentralized_service_liquidity_manager::HealthComponent::Relays,
                status: HealthStatus::Unhealthy,
                detail: Some(format!("relay state could not be read: {error}")),
                observed_at,
            };
        }
    };
    let failed_count = relay_states
        .iter()
        .filter(|relay| relay.status == RelayStatus::Failed)
        .count();
    let status = if relay_states.is_empty() {
        HealthStatus::Unknown
    } else if failed_count == 0 {
        HealthStatus::Healthy
    } else if failed_count == relay_states.len() {
        HealthStatus::Unhealthy
    } else {
        HealthStatus::Warning
    };
    ComponentHealth {
        component: fedi_decentralized_service_liquidity_manager::HealthComponent::Relays,
        status,
        detail: Some(format!(
            "relays={}, failed={}",
            relay_states.len(),
            failed_count
        )),
        observed_at,
    }
}

async fn load_advertisement_record(context: &DaemonContext) -> ServiceResult<AdvertisementRecord> {
    let row = sqlx::query(
        "SELECT status, signed_advertisement_json, readiness_json, last_published_at, expires_at, \
         withdrawn_at FROM provider_advertisements WHERE id = ?",
    )
    .bind(ADVERTISEMENT_ROW_ID)
    .fetch_optional(context.database.pool())
    .await
    .map_err(internal_error)?;

    let relay_states = load_relay_states(context).await?;
    let Some(row) = row else {
        return Ok(AdvertisementRecord {
            status: AdvertisementPublicationStatus::NotReady,
            advertisement: None,
            last_published_at: None,
            expires_at: None,
            withdrawn_at: None,
            relay_states,
        });
    };

    Ok(AdvertisementRecord {
        status: parse_publication_status(row.get::<String, _>("status").as_str())?,
        advertisement: row
            .get::<Option<String>, _>("signed_advertisement_json")
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(internal_error)?,
        last_published_at: optional_timestamp(row.get("last_published_at")),
        expires_at: optional_timestamp(row.get("expires_at")),
        withdrawn_at: optional_timestamp(row.get("withdrawn_at")),
        relay_states,
    })
}

async fn load_relay_states(context: &DaemonContext) -> ServiceResult<Vec<RelayPublicationState>> {
    let rows = sqlx::query(
        "SELECT relay_url, status, last_error, last_seen_at \
         FROM relay_publications ORDER BY relay_url",
    )
    .fetch_all(context.database.pool())
    .await
    .map_err(internal_error)?;

    rows.into_iter()
        .map(|row| {
            Ok(RelayPublicationState {
                relay_url: Url(row.get("relay_url")),
                status: parse_relay_status(row.get::<String, _>("status").as_str())?,
                last_error: row.get("last_error"),
                last_seen_at: optional_timestamp(row.get("last_seen_at")),
            })
        })
        .collect()
}

async fn persist_not_ready(
    context: &DaemonContext,
    readiness: &PublicReadiness,
) -> ServiceResult<()> {
    let readiness_json = serde_json::to_string(readiness).map_err(internal_error)?;
    sqlx::query(
        "INSERT INTO provider_advertisements \
         (id, status, readiness_json, last_error, updated_at) \
         VALUES (?, ?, ?, ?, unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           status = excluded.status, \
           readiness_json = excluded.readiness_json, \
           last_error = excluded.last_error, \
           updated_at = unixepoch()",
    )
    .bind(ADVERTISEMENT_ROW_ID)
    .bind(AdvertisementPublicationStatus::NotReady.to_string())
    .bind(readiness_json)
    .bind(readiness.reason.as_deref())
    .execute(context.database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

async fn persist_publication_status(
    context: &DaemonContext,
    status: AdvertisementPublicationStatus,
    error: Option<&str>,
) -> ServiceResult<()> {
    let now = now_timestamp();
    let last_published_at = if status == AdvertisementPublicationStatus::Published {
        Some(now.0 as i64)
    } else {
        None
    };
    let withdrawn_at = if status == AdvertisementPublicationStatus::Withdrawn {
        Some(now.0 as i64)
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO provider_advertisements \
         (id, status, last_published_at, withdrawn_at, last_error, updated_at) \
         VALUES (?, ?, ?, ?, ?, unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           status = excluded.status, \
           last_published_at = COALESCE(excluded.last_published_at, provider_advertisements.last_published_at), \
           withdrawn_at = COALESCE(excluded.withdrawn_at, provider_advertisements.withdrawn_at), \
           last_error = excluded.last_error, \
           updated_at = unixepoch()",
    )
    .bind(ADVERTISEMENT_ROW_ID)
    .bind(status.to_string())
    .bind(last_published_at)
    .bind(withdrawn_at)
    .bind(error)
    .execute(context.database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

async fn upsert_relay_state(
    context: &DaemonContext,
    relay_url: &Url,
    status: RelayStatus,
    advertisement_hash: Option<&[u8]>,
    event_id: Option<&str>,
    last_error: Option<&str>,
    last_seen_at: Option<Timestamp>,
) -> ServiceResult<()> {
    sqlx::query(
        "INSERT INTO relay_publications \
         (relay_url, status, advertisement_hash, event_id, last_error, last_seen_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, unixepoch()) \
         ON CONFLICT(relay_url) DO UPDATE SET \
           status = excluded.status, \
           advertisement_hash = COALESCE(excluded.advertisement_hash, relay_publications.advertisement_hash), \
           event_id = COALESCE(excluded.event_id, relay_publications.event_id), \
           last_error = excluded.last_error, \
           last_seen_at = excluded.last_seen_at, \
           updated_at = unixepoch()",
    )
    .bind(&relay_url.0)
    .bind(status.to_string())
    .bind(advertisement_hash)
    .bind(event_id)
    .bind(last_error)
    .bind(last_seen_at.map(|timestamp| timestamp.0 as i64))
    .execute(context.database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

async fn clear_relay_states(context: &DaemonContext) -> ServiceResult<()> {
    sqlx::query("DELETE FROM relay_publications")
        .execute(context.database.pool())
        .await
        .map_err(internal_error)?;
    Ok(())
}

async fn mark_all_relays_failed(
    context: &DaemonContext,
    relays: &[Url],
    advertisement_hash: &[u8],
    error: &str,
) -> ServiceResult<()> {
    for relay_url in relays {
        upsert_relay_state(
            context,
            relay_url,
            RelayStatus::Failed,
            Some(advertisement_hash),
            None,
            Some(error),
            Some(now_timestamp()),
        )
        .await?;
    }
    Ok(())
}

fn advertised_endpoint_url(node_id: &str) -> Url {
    let alpn = String::from_utf8_lossy(PUBLIC_LIQUIDITY_API_ALPN).replace('/', "%2F");
    Url(format!("iroh://{node_id}?alpn={alpn}"))
}

fn optional_timestamp(value: Option<i64>) -> Option<Timestamp> {
    value
        .and_then(|value| u64::try_from(value).ok())
        .map(Timestamp)
}

fn parse_publication_status(value: &str) -> ServiceResult<AdvertisementPublicationStatus> {
    value
        .parse()
        .map_err(|_| internal_error(format!("unknown advertisement publication status {value}")))
}

fn parse_relay_status(value: &str) -> ServiceResult<RelayStatus> {
    value
        .parse()
        .map_err(|_| internal_error(format!("unknown relay status {value}")))
}

#[cfg(test)]
#[path = "../tests/advertisement.rs"]
mod tests;
