//! Guardian link: the operator's Fedi app, linked to this FMan so the daemon
//! can tell it when something needs attention
//! ([`SPEC-guardian-link`](../../specs/SPEC-guardian-link.md)).
//!
//! Three pieces, all over one durable row (`db::guardian_link`):
//!
//! - **Linking.** The dashboard asks for a link offer: a one-time secret the
//!   fleet keeps in memory for ten minutes and shows as a QR code. The app
//!   dials the [`GUARDIAN_LINK_ALPN`] service with that secret and its
//!   push-gateway hook; [`GuardianLinkRpc`] consumes the secret and stores
//!   the link. Later verbs are signed by the device key the link recorded.
//! - **Attention.** [`Fleet::attention_reasons`] derives the coarse set of
//!   reasons the operator should look, from what the daemon already knows:
//!   seat health, readiness, payment-federation receivability, directory
//!   presence, and unread support messages.
//! - **Notifying.** [`spawn_notifier`] runs the edge detector: when a reason
//!   appears that the device has not been told about, it invokes the hook
//!   once with every current reason, through the same origin-pinned
//!   capability the FI completion callbacks use. The push payload carries only
//!   reason codes; title and body are fixed by the app when it creates the
//!   hook.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fedi_decentralized_domain::ProtocolV1;
use fedi_decentralized_service_fleet_manager::{
    AttentionReason, DEVICE_LABEL_MAX_CHARS, DkgCompletionCallback, DkgCompletionCallbackInput,
    FiSignedRequest, FmanName, GetAttentionRequest, GetAttentionResponse, GuardianLinkError,
    GuardianLinkInvite, GuardianLinkResult, GuardianLinkService, LinkDeviceRequest,
    LinkDeviceResponse, LinkSecret, RenewCallbackRequest, RenewCallbackResponse, SeatHealth,
    SignedRequest, Timestamp, UnlinkDeviceRequest, UnlinkDeviceResponse,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::{GuardianLinkDelivery, GuardianLinkRecord};
use crate::directory::{DirectoryPresence, OnboardingStatus};
use crate::facts::CompletionCallbackReason;
use crate::fleet::Fleet;
use crate::push_callback::{CallbackAttemptOutcome, retry_delay};
use crate::seat::{SeatPhase, SeatReport};

/// How long a link offer stays scannable.
pub const LINK_OFFER_VALIDITY: Duration = Duration::from_secs(10 * 60);

/// How long a seat may be `Unavailable` before that counts as attention: a
/// crash-and-respawn is routine, a seat that stays down is not.
pub const SEAT_UNAVAILABLE_GRACE: Duration = Duration::from_secs(10 * 60);

/// Production cadence of the notifier's scan.
pub const DEFAULT_NOTIFY_SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// A link offer the dashboard handed out and nobody has used yet.
pub(crate) struct LinkOffer {
    secret: LinkSecret,
    expires_at_ms: i64,
}

/// Per-fleet guardian-link runtime state: the open offer and the evaluator's
/// memory of when each seat became unavailable.
#[derive(Default)]
pub(crate) struct GuardianLinkState {
    offer: Mutex<Option<LinkOffer>>,
    unavailable_since: Mutex<HashMap<fedi_decentralized_service_fleet_manager::SeatId, Instant>>,
    /// Edge trigger for the notifier after a link, renewal, or revoke.
    pub(crate) changed: tokio::sync::Notify,
}

#[cfg(test)]
impl GuardianLinkState {
    /// Pretend every currently unavailable seat has been so for `age`.
    pub(crate) fn backdate_unavailable_for_test(&self, age: Duration) {
        for first in self
            .unavailable_since
            .lock()
            .expect("unavailable lock")
            .values_mut()
        {
            *first = Instant::now()
                .checked_sub(age)
                .expect("age fits in Instant");
        }
    }
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_secs()
}

impl Fleet {
    /// This FMan's display name, as the app shows it.
    pub fn fman_name(&self) -> FmanName {
        let pubkey = self.identity().derive_service_nostr_pubkey();
        FmanName::from_fman_id(
            nostr_sdk::PublicKey::from_slice(&pubkey.serialize())
                .expect("an x-only key is a Nostr public key"),
        )
    }

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
        let removed = self.db().delete_guardian_link(None).await?;
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
    /// daemon already knows. Coarse by design: see [`AttentionReason`].
    pub async fn attention_reasons(
        &self,
        presence: &DirectoryPresence,
    ) -> anyhow::Result<BTreeSet<AttentionReason>> {
        self.attention_reasons_with(self.seat_report_snapshot(), presence)
            .await
    }

    async fn attention_reasons_with(
        &self,
        seats: Vec<(fedi_decentralized_service_fleet_manager::SeatId, SeatReport)>,
        presence: &DirectoryPresence,
    ) -> anyhow::Result<BTreeSet<AttentionReason>> {
        let mut reasons = BTreeSet::new();
        {
            let now = Instant::now();
            let mut since = self
                .guardian_link
                .unavailable_since
                .lock()
                .expect("unavailable lock");
            let mut live = HashMap::new();
            for (seat_id, report) in seats {
                // Only a formed seat is the operator's to keep up: one still
                // waiting on its FI's DKG is idle by design.
                let health = match report {
                    SeatReport::Active {
                        phase: SeatPhase::DataLoss { .. },
                        ..
                    } => SeatHealth::Failed,
                    SeatReport::Active {
                        phase: SeatPhase::Running { .. },
                        health,
                    } => health,
                    SeatReport::Active { .. } | SeatReport::Decommissioned { .. } => continue,
                };
                match health {
                    SeatHealth::Failed => {
                        reasons.insert(AttentionReason::SeatFailed);
                    }
                    SeatHealth::Unavailable => {
                        let first = since.remove(&seat_id).unwrap_or(now);
                        if SEAT_UNAVAILABLE_GRACE <= now.duration_since(first) {
                            reasons.insert(AttentionReason::SeatUnavailable);
                        }
                        live.insert(seat_id, first);
                    }
                    SeatHealth::Healthy => {}
                }
            }
            *since = live;
        }
        if self.seat_readiness().is_some() && !self.ready_for_new_seats().await {
            reasons.insert(AttentionReason::NotReadyForNewSeats);
        }
        // Nothing an FMan advertises is free (`Plan` docs), so an offered plan
        // means the operator expects to be paid through these federations.
        let priced = !self.offered_plans().await.is_empty();
        if priced
            && self
                .payment_federation_statuses()
                .await
                .iter()
                .any(|federation| federation.accepted && !federation.receivable)
        {
            reasons.insert(AttentionReason::PaymentFederationNotReceivable);
        }
        if matches!(presence.onboarding, OnboardingStatus::NotObserved { .. }) {
            reasons.insert(AttentionReason::NotApproved);
        }
        if 0 < self.db().support_unread().await? {
            reasons.insert(AttentionReason::SupportMessage);
        }
        Ok(reasons)
    }

    /// Invoke the linked device's hook once with these reasons. The
    /// idempotency key is minted from the durable notification sequence, so a
    /// retry after a transient failure deduplicates at the gateway.
    async fn send_guardian_link_notification(
        &self,
        link: &GuardianLinkRecord,
        reasons: &BTreeSet<AttentionReason>,
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
        let mut data = serde_json::Map::new();
        data.insert(
            "reasons".to_owned(),
            serde_json::Value::String(
                reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
        let invoker = &self.config().completion_callback_invoker;
        if !invoker.is_available() {
            return CallbackAttemptOutcome::Retryable(
                CompletionCallbackReason::HttpClientUnavailable,
            );
        }
        invoker.invoke(&validated.with_data(data)).await
    }

    /// The operator's test: notify the linked device now with the `test`
    /// reason. Consumes one of the gateway's hourly invocations.
    pub async fn test_guardian_link_notification(&self) -> anyhow::Result<CallbackAttemptOutcome> {
        let link = self
            .db()
            .guardian_link()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no phone is linked"))?;
        anyhow::ensure!(
            link.delivery == GuardianLinkDelivery::Active,
            "the linked phone's hook was rejected by the gateway; relink the phone"
        );
        let outcome = self
            .send_guardian_link_notification(&link, &BTreeSet::from([AttentionReason::Test]))
            .await;
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
                    .record_guardian_link_notified(link.notification_seq, &told)
                    .await?;
            }
            CallbackAttemptOutcome::Terminal(reason) => {
                self.db().record_guardian_link_terminal(reason).await?;
                tracing::warn!(
                    reason = reason.as_str(),
                    "guardian link hook rejected for good; the phone must relink"
                );
            }
            CallbackAttemptOutcome::Retryable(reason) => {
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

/// Start the notifier for this fleet. `presence` is the directory runtime's
/// latest observation, which is what the admin socket reads too.
pub fn spawn_notifier(
    fleet: Arc<Fleet>,
    presence: watch::Receiver<DirectoryPresence>,
    scan_interval: Duration,
) -> GuardianLinkNotifier {
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        let mut scan = tokio::time::interval(scan_interval);
        scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut backoff: Option<(u32, Instant)> = None;
        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = scan.tick() => {}
                () = fleet.guardian_link.changed.notified() => {}
            }
            let snapshot = presence.borrow().clone();
            if let Err(error) = tick(&fleet, &snapshot, &mut backoff).await {
                tracing::warn!(
                    error = format_args!("{error:#}"),
                    "guardian link tick failed"
                );
            }
        }
    });
    GuardianLinkNotifier {
        shutdown,
        handle: Some(handle),
    }
}

/// One edge-detecting pass. Returns whether a notification was sent.
async fn tick(
    fleet: &Fleet,
    presence: &DirectoryPresence,
    backoff: &mut Option<(u32, Instant)>,
) -> anyhow::Result<bool> {
    let Some(link) = fleet.db().guardian_link().await? else {
        *backoff = None;
        return Ok(false);
    };
    if link.delivery != GuardianLinkDelivery::Active {
        return Ok(false);
    }
    let current = fleet.attention_reasons(presence).await?;
    let told: BTreeSet<_> = link.notified_reasons.iter().copied().collect();
    let still_told: BTreeSet<_> = told.intersection(&current).copied().collect();
    if still_told != told {
        let narrowed: Vec<_> = still_told.iter().copied().collect();
        fleet
            .db()
            .set_guardian_link_notified_reasons(&narrowed)
            .await?;
    }
    if current.difference(&still_told).next().is_none() {
        return Ok(false);
    }
    if let Some((_, not_before)) = backoff
        && Instant::now() < *not_before
    {
        return Ok(false);
    }
    let outcome = fleet.send_guardian_link_notification(&link, &current).await;
    fleet
        .record_guardian_link_outcome(&link, outcome, &current)
        .await?;
    *backoff = match outcome {
        CallbackAttemptOutcome::Retryable(_) => {
            let attempt = backoff.map_or(1, |(attempt, _)| attempt.saturating_add(1));
            Some((
                attempt,
                Instant::now()
                    + retry_delay(
                        fleet.config().push_callback_retry_interval,
                        attempt,
                        &link.linked_at_ms.to_be_bytes(),
                    ),
            ))
        }
        _ => None,
    };
    Ok(outcome == CallbackAttemptOutcome::Delivered)
}

/// The Iroh service the operator's app talks to.
#[derive(Clone)]
pub struct GuardianLinkRpc {
    fleet: Arc<Fleet>,
    presence: watch::Receiver<DirectoryPresence>,
}

impl GuardianLinkRpc {
    pub fn new(fleet: Arc<Fleet>, presence: watch::Receiver<DirectoryPresence>) -> Self {
        Self { fleet, presence }
    }

    /// Verify a device-signed verb and require the signer to be the linked
    /// device. Every failure is the same coarse `Unauthorized`.
    async fn authorize<T: FiSignedRequest>(
        &self,
        request: &SignedRequest<T>,
    ) -> GuardianLinkResult<(T, GuardianLinkRecord)> {
        let verified = request
            .verify(Timestamp(now_secs()))
            .map_err(|_| GuardianLinkError::Unauthorized)?;
        let link = self
            .fleet
            .db()
            .guardian_link()
            .await
            .map_err(|error| internal("guardian_link", &error))?
            .ok_or(GuardianLinkError::NotLinked)?;
        if link.device_id != *verified.signer() {
            return Err(GuardianLinkError::Unauthorized);
        }
        Ok((verified.into_inner(), link))
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
        let linked_at_ms = self
            .fleet
            .db()
            .replace_guardian_link(
                &request.device_id,
                request.device_label.trim(),
                &request.callback,
                request.callback_expires_at.0,
            )
            .await
            .map_err(|error| internal("link_device", &error))?;
        self.fleet.guardian_link.changed.notify_one();
        tracing::info!("guardian link established");
        Ok(LinkDeviceResponse {
            version: ProtocolV1,
            fman_name: self.fleet.fman_name().to_string(),
            linked_at: Timestamp(u64::try_from(linked_at_ms / 1000).unwrap_or(0)),
        })
    }

    async fn get_attention(
        &self,
        request: SignedRequest<GetAttentionRequest>,
    ) -> GuardianLinkResult<GetAttentionResponse> {
        let (_, link) = self.authorize(&request).await?;
        let presence = self.presence.borrow().clone();
        let reasons = self
            .fleet
            .attention_reasons(&presence)
            .await
            .map_err(|error| internal("get_attention", &error))?;
        Ok(GetAttentionResponse {
            version: ProtocolV1,
            fman_name: self.fleet.fman_name().to_string(),
            reasons: reasons.into_iter().collect(),
            checked_at: Timestamp(now_secs()),
            linked_at: Timestamp(u64::try_from(link.linked_at_ms / 1000).unwrap_or(0)),
            last_notified_at: link
                .last_notified_at_ms
                .map(|at_ms| Timestamp(u64::try_from(at_ms / 1000).unwrap_or(0))),
            callback_expires_at: Timestamp(link.callback_expires_at),
        })
    }

    async fn renew_callback(
        &self,
        request: SignedRequest<RenewCallbackRequest>,
    ) -> GuardianLinkResult<RenewCallbackResponse> {
        let (request, _) = self.authorize(&request).await?;
        self.fleet
            .validate_guardian_link_callback(&request.callback)?;
        let renewed = self
            .fleet
            .db()
            .renew_guardian_link_callback(
                &request.device_id,
                &request.callback,
                request.callback_expires_at.0,
            )
            .await
            .map_err(|error| internal("renew_callback", &error))?;
        if !renewed {
            return Err(GuardianLinkError::NotLinked);
        }
        self.fleet.guardian_link.changed.notify_one();
        Ok(RenewCallbackResponse {
            version: ProtocolV1,
        })
    }

    async fn unlink_device(
        &self,
        request: SignedRequest<UnlinkDeviceRequest>,
    ) -> GuardianLinkResult<UnlinkDeviceResponse> {
        let (request, _) = self.authorize(&request).await?;
        self.fleet
            .db()
            .delete_guardian_link(Some(&request.device_id))
            .await
            .map_err(|error| internal("unlink_device", &error))?;
        self.fleet.guardian_link.changed.notify_one();
        Ok(UnlinkDeviceResponse {
            version: ProtocolV1,
        })
    }
}

#[cfg(test)]
#[path = "../tests/guardian_link.rs"]
mod tests;
