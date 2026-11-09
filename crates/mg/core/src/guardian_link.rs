//! Guardian link: the operator's Fedi app, linked to this MG so the daemon
//! can tell it when something needs attention
//! ([`SPEC-guardian-link`](../../specs/SPEC-guardian-link.md)).
//!
//! Three pieces, all over one durable row (`db::guardian_link`):
//!
//! - **Linking.** The dashboard asks for a link offer: a one-time secret the
//!   fleet keeps in memory for ten minutes and shows as a QR code. The app
//!   dials the [`GUARDIAN_LINK_ALPN`] service with that secret and its
//!   push-gateway hook; [`GuardianLinkRpc`] consumes the secret and stores
//!   the link. That is the phone's only call: the hook lives a year and is
//!   not renewed, so when it expires the host links the phone again.
//! - **Attention.** [`Fleet::attention_reasons`] derives the coarse set of
//!   reasons the operator should look from what the daemon already holds:
//!   the latest readiness report (the Health page's checks), the
//!   setup-payment policy's recommended release, and unread support
//!   messages.
//! - **Notifying.** [`spawn_notifier`] wakes when one of those sources
//!   changes, or the link does, and runs the edge detector: when a reason
//!   appears that the device has not been told about, it invokes the hook
//!   once, through the same origin-pinned capability the FI completion
//!   callbacks use. Nothing runs on a timer except the retry of a failed
//!   send. The invocation carries no data: title and body are fixed by the
//!   app when it creates the hook, and the dashboard shows why.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fedi_decentralized_domain::{AdmittedSetupPaymentFederations, ProtocolV1};
use fedi_decentralized_service_manifold_guardian::{
    DEVICE_LABEL_MAX_CHARS, DkgCompletionCallback, DkgCompletionCallbackInput, GuardianLinkError,
    GuardianLinkInvite, GuardianLinkResult, GuardianLinkService, LinkDeviceRequest,
    LinkDeviceResponse, LinkSecret,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::{GuardianLinkDelivery, GuardianLinkRecord};
use crate::facts::CompletionCallbackReason;
use crate::fleet::Fleet;
use crate::push_callback::{CallbackAttemptOutcome, retry_delay};
use crate::seat_readiness::{ReadinessOutcome, this_mg_version};

/// Why the MG wants the host to look. Coarse on purpose; the dashboard has the
/// detail. Persisted (as snake_case codes) so a restart does not repeat a
/// notification for a reason already told.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// The Health page's relay check fails.
    RelayCheckFailed,
    /// The Health page's discovery check fails.
    DiscoveryCheckFailed,
    /// The Health page's bitcoin check fails.
    BitcoinCheckFailed,
    /// The Health page's version check fails: this release is below the
    /// policy's minimum, so the host sells no new seats.
    VersionCheckFailed,
    /// The Health page's uptime check fails.
    UptimeCheckFailed,
    /// This release is below the one the setup-payment policy recommends.
    UpdateRecommended,
    /// Support wrote to the host.
    SupportMessage,
}

/// How long a link offer stays scannable.
pub const LINK_OFFER_VALIDITY: Duration = Duration::from_secs(10 * 60);

/// A link offer the dashboard handed out and nobody has used yet.
pub(crate) struct LinkOffer {
    secret: LinkSecret,
    expires_at_ms: i64,
}

/// Per-fleet guardian-link runtime state: the open offer and the notifier's
/// wake.
#[derive(Default)]
pub(crate) struct GuardianLinkState {
    offer: Mutex<Option<LinkOffer>>,
    /// Wakes the notifier after a link, a revoke, or a new readiness report.
    pub(crate) changed: tokio::sync::Notify,
}

/// The operator-facing projection of the link (admin `GuardianLink`).
#[derive(Clone, Debug)]
pub struct GuardianLinkStatus {
    /// Whether this deployment can invoke hooks at all.
    pub available: bool,
    pub link: Option<GuardianLinkRecord>,
    pub offer: Option<LinkOfferStatus>,
}

#[derive(Clone, Debug)]
pub struct LinkOfferStatus {
    pub uri: String,
    pub expires_at: u64,
}

impl Fleet {
    /// Hand out a fresh link offer, replacing any open one. Needs the Iroh
    /// endpoint id the daemon binds once its router is up.
    pub fn create_guardian_link_offer(&self) -> anyhow::Result<GuardianLinkInvite> {
        anyhow::ensure!(
            self.config().push_gateway_origin.is_some(),
            "this host has no push gateway configured, so it cannot notify a phone"
        );
        let iroh_endpoint_id = self
            .iroh_endpoint_id()
            .ok_or_else(|| anyhow::anyhow!("the daemon is still starting"))?;
        let secret = LinkSecret::from_bytes(rand::random());
        let expires_at_ms = crate::db::now_ms()
            .saturating_add(i64::try_from(LINK_OFFER_VALIDITY.as_millis()).unwrap_or(i64::MAX));
        *self.guardian_link.offer.lock().expect("offer lock") = Some(LinkOffer {
            secret: secret.clone(),
            expires_at_ms,
        });
        Ok(self.guardian_link_invite(iroh_endpoint_id, secret))
    }

    fn guardian_link_invite(
        &self,
        iroh_endpoint_id: String,
        secret: LinkSecret,
    ) -> GuardianLinkInvite {
        let pubkey = self.identity().derive_service_nostr_pubkey();
        GuardianLinkInvite {
            fman_nostr_pubkey: nostr_sdk::PublicKey::from_slice(&pubkey.serialize())
                .expect("an x-only key is a Nostr public key"),
            iroh_endpoint_id,
            environment: self.config().manifold_environment.to_string(),
            secret,
        }
    }

    /// Consume the open offer if `supplied` is it and it has not expired.
    fn consume_guardian_link_offer(&self, supplied: &LinkSecret) -> bool {
        let mut offer = self.guardian_link.offer.lock().expect("offer lock");
        let Some(open) = offer.as_ref() else {
            return false;
        };
        if crate::db::now_ms() > open.expires_at_ms {
            *offer = None;
            return false;
        }
        if !constant_time_eq::constant_time_eq(open.secret.as_bytes(), supplied.as_bytes()) {
            return false;
        }
        *offer = None;
        true
    }

    /// The link, the open offer, and whether notifying is possible here.
    pub async fn guardian_link_status(&self) -> anyhow::Result<GuardianLinkStatus> {
        let link = self.db().guardian_link().await?;
        let offer = {
            let offer = self.guardian_link.offer.lock().expect("offer lock");
            offer
                .as_ref()
                .filter(|open| crate::db::now_ms() <= open.expires_at_ms)
                .and_then(|open| {
                    let id = self.iroh_endpoint_id()?;
                    Some(LinkOfferStatus {
                        uri: self.guardian_link_invite(id, open.secret.clone()).to_uri(),
                        expires_at: u64::try_from(open.expires_at_ms / 1000).unwrap_or(0),
                    })
                })
        };
        Ok(GuardianLinkStatus {
            available: self.config().push_gateway_origin.is_some(),
            link,
            offer,
        })
    }

    /// The operator's revoke: forget the device and close any open offer.
    pub async fn revoke_guardian_link(&self) -> anyhow::Result<bool> {
        *self.guardian_link.offer.lock().expect("offer lock") = None;
        let removed = self.db().delete_guardian_link().await?;
        self.guardian_link.changed.notify_one();
        Ok(removed)
    }

    /// Validate a device's hook against the deployment's gateway origin.
    fn validate_guardian_link_callback(
        &self,
        callback: &DkgCompletionCallback,
    ) -> GuardianLinkResult<()> {
        let origin = self.config().push_gateway_origin.as_ref().ok_or_else(|| {
            GuardianLinkError::PushGatewayUnusable(
                "no push gateway origin is configured".to_owned(),
            )
        })?;
        origin.validate(callback).map(drop).map_err(|_| {
            GuardianLinkError::PushGatewayUnusable(
                "the hook is not under this host's push gateway origin".to_owned(),
            )
        })
    }

    /// What the operator should look at right now, derived from what the
    /// daemon already holds. Coarse by design: see [`AttentionReason`].
    pub async fn attention_reasons(
        &self,
        policy: Option<&AdmittedSetupPaymentFederations>,
    ) -> anyhow::Result<BTreeSet<AttentionReason>> {
        let mut reasons = BTreeSet::new();
        // Before the first readiness run nothing is known, so nothing fails.
        if let Some(report) = self.seat_readiness() {
            let checks: [(ReadinessOutcome, AttentionReason); 5] = [
                (report.relay, AttentionReason::RelayCheckFailed),
                (report.discovery, AttentionReason::DiscoveryCheckFailed),
                (report.bitcoin, AttentionReason::BitcoinCheckFailed),
                (report.version, AttentionReason::VersionCheckFailed),
                (report.uptime, AttentionReason::UptimeCheckFailed),
            ];
            reasons.extend(
                checks
                    .into_iter()
                    .filter(|(outcome, _)| !outcome.passed())
                    .map(|(_, reason)| reason),
            );
        }
        if policy
            .and_then(AdmittedSetupPaymentFederations::recommended_min_mg_version)
            .is_some_and(|recommended| this_mg_version().is_older_than(recommended))
        {
            reasons.insert(AttentionReason::UpdateRecommended);
        }
        if 0 < self.db().support_unread().await? {
            reasons.insert(AttentionReason::SupportMessage);
        }
        Ok(reasons)
    }

    /// Invoke the linked device's hook once. The idempotency key is minted
    /// from the durable notification sequence, so a retry after a transient
    /// failure deduplicates at the gateway.
    async fn send_guardian_link_notification(
        &self,
        link: &GuardianLinkRecord,
    ) -> CallbackAttemptOutcome {
        let Some(callback) = link.callback.as_ref() else {
            return CallbackAttemptOutcome::Terminal(
                CompletionCallbackReason::HookExpiredOrRevoked,
            );
        };
        let Some(origin) = self.config().push_gateway_origin.as_ref() else {
            return CallbackAttemptOutcome::Retryable(
                CompletionCallbackReason::GatewayOriginMissing,
            );
        };
        let keyed = DkgCompletionCallback::new(DkgCompletionCallbackInput {
            callback_url: callback.callback_url().to_owned(),
            idempotency_key: format!(
                "guardian-link:{}:{}",
                link.linked_at_ms, link.notification_seq
            ),
        })
        .expect("a stored callback URL is bounded and the key is short");
        let Ok(validated) = origin.validate(&keyed) else {
            return CallbackAttemptOutcome::Retryable(
                CompletionCallbackReason::GatewayOriginMismatch,
            );
        };
        let invoker = &self.config().completion_callback_invoker;
        if !invoker.is_available() {
            return CallbackAttemptOutcome::Retryable(
                CompletionCallbackReason::HttpClientUnavailable,
            );
        }
        invoker.invoke(&validated).await
    }

    /// The host's test: notify the linked device now. Consumes one of the
    /// gateway's hourly invocations and tells it nothing new.
    pub async fn test_guardian_link_notification(&self) -> anyhow::Result<CallbackAttemptOutcome> {
        let link = self
            .db()
            .guardian_link()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no phone is linked"))?;
        anyhow::ensure!(
            !matches!(link.delivery, GuardianLinkDelivery::Terminal(_)),
            "the linked phone's hook was rejected by the gateway; link the phone again"
        );
        let outcome = self.send_guardian_link_notification(&link).await;
        self.record_guardian_link_outcome(
            &link,
            outcome,
            &link.notified_reasons.iter().copied().collect(),
        )
        .await?;
        Ok(outcome)
    }

    async fn record_guardian_link_outcome(
        &self,
        link: &GuardianLinkRecord,
        outcome: CallbackAttemptOutcome,
        told: &BTreeSet<AttentionReason>,
    ) -> anyhow::Result<()> {
        match outcome {
            CallbackAttemptOutcome::Delivered => {
                let told: Vec<_> = told.iter().copied().collect();
                self.db()
                    .record_guardian_link_notified(link.linked_at_ms, link.notification_seq, &told)
                    .await?;
            }
            CallbackAttemptOutcome::Terminal(reason) => {
                self.db()
                    .record_guardian_link_terminal(link.linked_at_ms, reason)
                    .await?;
                tracing::warn!(
                    reason = reason.as_str(),
                    "guardian link hook rejected for good; the phone must relink"
                );
            }
            CallbackAttemptOutcome::Retryable(reason) => {
                if reason == CompletionCallbackReason::TargetInactive {
                    self.db()
                        .record_guardian_link_inactive(link.linked_at_ms)
                        .await?;
                }
                tracing::info!(
                    reason = reason.as_str(),
                    "guardian link notification deferred"
                );
            }
        }
        Ok(())
    }
}

/// The notifier: a periodic scan plus wakes, each running one edge-detecting
/// tick. Drop or `shutdown` stops it.
pub struct GuardianLinkNotifier {
    shutdown: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

impl GuardianLinkNotifier {
    pub async fn shutdown(mut self) {
        self.shutdown.send_replace(true);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for GuardianLinkNotifier {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

/// When the notifier tries again after a send or a tick failed: its attempt
/// count and the earliest time.
type Backoff = Option<(u32, tokio::time::Instant)>;

/// Start the notifier for this fleet. It wakes on a link change or a new
/// readiness report (`changed`), a new setup-payment `policy`, or a support
/// thread write, and on its own only to retry a failed send.
pub fn spawn_notifier(
    fleet: Arc<Fleet>,
    mut policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) -> GuardianLinkNotifier {
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let mut support = fleet.db().subscribe_support();
    let handle = tokio::spawn(async move {
        let mut backoff: Backoff = None;
        let mut policy_open = true;
        loop {
            let snapshot = policy.borrow_and_update().clone();
            support.borrow_and_update();
            if let Err(error) = tick(&fleet, snapshot.as_ref(), &mut backoff).await {
                tracing::warn!(
                    error = format_args!("{error:#}"),
                    "guardian link tick failed"
                );
                backoff = next_backoff(&fleet, backoff, b"guardian-link-tick");
            }
            let retry_at = backoff.map(|(_, at)| at);
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                () = fleet.guardian_link.changed.notified() => {}
                changed = policy.changed(), if policy_open => policy_open = changed.is_ok(),
                // The database holds the sender for as long as the fleet.
                Ok(()) = support.changed() => {}
                () = tokio::time::sleep_until(retry_at.unwrap_or_else(tokio::time::Instant::now)),
                    if retry_at.is_some() => {}
            }
        }
    });
    GuardianLinkNotifier {
        shutdown,
        handle: Some(handle),
    }
}

fn next_backoff(fleet: &Fleet, backoff: Backoff, entropy: &[u8]) -> Backoff {
    let attempt = backoff.map_or(1, |(attempt, _)| attempt.saturating_add(1));
    Some((
        attempt,
        tokio::time::Instant::now()
            + retry_delay(
                fleet.config().push_callback_retry_interval,
                attempt,
                entropy,
            ),
    ))
}

/// One edge-detecting pass. Returns whether a notification was sent.
async fn tick(
    fleet: &Fleet,
    policy: Option<&AdmittedSetupPaymentFederations>,
    backoff: &mut Backoff,
) -> anyhow::Result<bool> {
    let Some(link) = fleet.db().guardian_link().await? else {
        *backoff = None;
        return Ok(false);
    };
    if matches!(link.delivery, GuardianLinkDelivery::Terminal(_)) {
        *backoff = None;
        return Ok(false);
    }
    let current = fleet.attention_reasons(policy).await?;
    // A source not read yet since start (no readiness report, no policy) is
    // unknown, not cleared: its told reasons stay told, so a restart does not
    // tell them again.
    let readiness_known = fleet.seat_readiness().is_some();
    let unread = |reason: &AttentionReason| match reason {
        AttentionReason::RelayCheckFailed
        | AttentionReason::DiscoveryCheckFailed
        | AttentionReason::BitcoinCheckFailed
        | AttentionReason::VersionCheckFailed
        | AttentionReason::UptimeCheckFailed => !readiness_known,
        AttentionReason::UpdateRecommended => policy.is_none(),
        AttentionReason::SupportMessage => false,
    };
    let told: BTreeSet<_> = link.notified_reasons.iter().copied().collect();
    let still_told: BTreeSet<_> = told
        .iter()
        .copied()
        .filter(|reason| current.contains(reason) || unread(reason))
        .collect();
    if still_told != told {
        let narrowed: Vec<_> = still_told.iter().copied().collect();
        fleet
            .db()
            .set_guardian_link_notified_reasons(link.linked_at_ms, &narrowed)
            .await?;
    }
    if current.difference(&still_told).next().is_none() {
        *backoff = None;
        return Ok(false);
    }
    if let Some((_, not_before)) = backoff
        && tokio::time::Instant::now() < *not_before
    {
        return Ok(false);
    }
    let outcome = fleet.send_guardian_link_notification(&link).await;
    let now_told: BTreeSet<_> = current.union(&still_told).copied().collect();
    fleet
        .record_guardian_link_outcome(&link, outcome, &now_told)
        .await?;
    *backoff = match outcome {
        CallbackAttemptOutcome::Retryable(_) => {
            next_backoff(fleet, *backoff, &link.linked_at_ms.to_be_bytes())
        }
        _ => None,
    };
    Ok(outcome == CallbackAttemptOutcome::Delivered)
}

/// The Iroh service the operator's app talks to.
#[derive(Clone)]
pub struct GuardianLinkRpc {
    fleet: Arc<Fleet>,
}

impl GuardianLinkRpc {
    pub fn new(fleet: Arc<Fleet>) -> Self {
        Self { fleet }
    }
}

fn internal(verb: &'static str, error: &dyn std::fmt::Display) -> GuardianLinkError {
    tracing::error!(verb, error = %error, "guardian link internal error");
    GuardianLinkError::Internal
}

fn validate_device_label(label: &str) -> GuardianLinkResult<()> {
    let trimmed = label.trim();
    if trimmed.is_empty()
        || DEVICE_LABEL_MAX_CHARS < trimmed.chars().count()
        || trimmed.chars().any(char::is_control)
    {
        return Err(GuardianLinkError::InvalidArgument(format!(
            "device label must be 1 to {DEVICE_LABEL_MAX_CHARS} printable characters"
        )));
    }
    Ok(())
}

impl GuardianLinkService for GuardianLinkRpc {
    async fn link_device(
        &self,
        request: LinkDeviceRequest,
    ) -> GuardianLinkResult<LinkDeviceResponse> {
        validate_device_label(&request.device_label)?;
        self.fleet
            .validate_guardian_link_callback(&request.callback)?;
        if !self.fleet.consume_guardian_link_offer(&request.secret) {
            return Err(GuardianLinkError::InvalidInvite);
        }
        self.fleet
            .db()
            .replace_guardian_link(request.device_label.trim(), &request.callback)
            .await
            .map_err(|error| internal("link_device", &error))?;
        self.fleet.guardian_link.changed.notify_one();
        tracing::info!("guardian link established");
        Ok(LinkDeviceResponse {
            version: ProtocolV1,
        })
    }
}

#[cfg(test)]
#[path = "../tests/guardian_link.rs"]
mod tests;
