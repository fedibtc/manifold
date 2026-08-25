//! Shared Nostr relay client wrapper.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::time::Duration;

use core::future::Future;

use fedimint_core::runtime::{Instant, spawn, timeout as runtime_timeout};
use futures_util::{Stream, StreamExt};
use nostr_sdk::pool::{Output, RelayLimits};
use nostr_sdk::{
    Client, ClientOptions, Event, EventBuilder, EventId, Filter, JsonUtil, Keys, RelayMessage,
    RelayPoolNotification, RelayStatus, RelayUrl, SubscriptionId,
};
use tokio_stream::wrappers::BroadcastStream;

use crate::relay_candidate_database::RelayCandidateDatabase;
use crate::{NostrClientError, NostrClientResult, ROLE_FETCHED_EVENT_MAX_BYTES};

/// Thin wrapper around [`nostr_sdk::Client`] with common relay operations.
///
/// A client holds one or more relays. Single-relay construction
/// ([`Self::connect`], [`Self::connect_without_signer`]) keeps the strict
/// one-relay semantics the backup, restore, and per-relay verifier paths
/// rely on. Pooled construction ([`Self::connect_pool`]) serves the liveness
/// paths (advertisements, discovery): publishes succeed on the first relay
/// ack, reads succeed once one relay delivers its complete answer, and
/// slower relays merge best-effort. Cross-relay duplicates of one signed
/// event share an event id and are counted once; nothing here applies NIP-01
/// replacement ordering — semantic layers own that.
#[derive(Clone, Debug)]
pub struct NostrRelayClient {
    /// Inner client from the Nostr SDK.
    client: Client,
}

impl NostrRelayClient {
    /// Create a client without a signer, add one relay, and connect to it.
    ///
    /// This supports publishing an already-signed event without importing its
    /// author key, including retrying an operational publication receipt.
    ///
    /// # Errors
    ///
    /// Returns an error if adding the validated relay fails or no relay
    /// connection succeeds before the timeout.
    pub async fn connect_without_signer(
        relay_url: &RelayUrl,
        timeout: Duration,
    ) -> NostrClientResult<Self> {
        Self::connect_inner(std::slice::from_ref(relay_url), None, timeout).await
    }

    /// Create a client with keys, add one relay, and connect to it.
    ///
    /// The configured keys are the author keys for all subsequent publish operations
    /// performed by role-specific clients built from this relay client.
    ///
    /// # Errors
    ///
    /// Returns an error if the relay URL is invalid, adding the relay fails, or no
    /// relay connection succeeds before the timeout.
    pub async fn connect(
        relay_url: &str,
        keys: Keys,
        timeout: Duration,
    ) -> NostrClientResult<Self> {
        let relay_url = parse_relay_url(relay_url)?;
        Self::connect_inner(&[relay_url], Some(keys), timeout).await
    }

    /// Create a client over every given relay and connect the pool.
    ///
    /// One reachable relay is enough: relays that fail to connect here keep
    /// reconnecting in the background and join later. Use this for the
    /// liveness paths only; backup, restore, and per-relay verifier reads
    /// keep their single-relay clients.
    ///
    /// # Errors
    ///
    /// Returns an error if no relay is given, adding a relay fails, or no
    /// relay connection succeeds before the timeout.
    pub async fn connect_pool(
        relay_urls: &[RelayUrl],
        keys: Keys,
        timeout: Duration,
    ) -> NostrClientResult<Self> {
        if relay_urls.is_empty() {
            return Err(NostrClientError::Connect);
        }
        Self::connect_inner(relay_urls, Some(keys), timeout).await
    }

    async fn connect_inner(
        relay_urls: &[RelayUrl],
        keys: Option<Keys>,
        timeout: Duration,
    ) -> NostrClientResult<Self> {
        let mut builder = Client::builder()
            .database(RelayCandidateDatabase)
            .opts(role_client_options());
        if let Some(keys) = keys {
            builder = builder.signer(keys);
        }
        let client = builder.build();
        for relay_url in relay_urls {
            client
                .add_relay(relay_url.clone())
                .await
                .map_err(|source| NostrClientError::AddRelay {
                    url: relay_url.clone(),
                    source,
                })?;
        }
        let output = client.try_connect(timeout).await;
        if output.success.is_empty() {
            return Err(NostrClientError::Connect);
        }
        // `try_connect` schedules no retries for relays that failed just now.
        // Spawn their persistent connection tasks so a relay that was down at
        // startup joins the pool when it comes back. Already-connected relays
        // (every relay of a single-relay client that got here) are skipped.
        client.connect().await;
        Ok(Self { client })
    }

    pub(crate) async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// Publish an event builder and return the accepted event id.
    ///
    /// The verdict is at-least-one-ack, but the call itself waits for every
    /// connected relay to answer or hit the SDK's per-relay acknowledgement
    /// timeout — a connected-but-silent relay bounds publish latency, it
    /// never changes the outcome. Unreachable relays are skipped fast.
    pub async fn publish_event(&self, builder: EventBuilder) -> NostrClientResult<EventId> {
        let output = self
            .client
            .send_event_builder(builder)
            .await
            .map_err(|source| NostrClientError::Publish { source })?;
        validate_publish_output(output)
    }

    /// Publish a complete event that was signed before this client was built.
    ///
    /// # Errors
    ///
    /// Returns an error if sending fails or no relay acknowledges the event.
    pub async fn publish_signed_event(&self, event: &Event) -> NostrClientResult<EventId> {
        let output = self
            .client
            .send_event(event)
            .await
            .map_err(|source| NostrClientError::Publish { source })?;
        validate_publish_output(output)
    }

    /// Fetch all events matching a filter within the timeout.
    ///
    /// Runs on the same bounded collector as [`Self::fetch_events_capped`]
    /// (with the widest count bound) so a dead pool relay delays the result
    /// only until every reachable relay has answered, never for the full
    /// timeout on its own.
    pub(crate) async fn fetch_events(
        &self,
        filter: Filter,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        let mut events = self.fetch_events_capped(filter, timeout, u16::MAX).await?;
        // The SDK helper this replaced returned newest-first; callers take a
        // prefix of the result, so keep that contract.
        events.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(events)
    }

    /// Fetch at most `limit` events matching a filter within the timeout.
    pub async fn fetch_events_capped(
        &self,
        filter: Filter,
        timeout: Duration,
        limit: u16,
    ) -> NostrClientResult<Vec<Event>> {
        // `Filter::limit` is only a relay hint. Do not use SDK fetch/stream helpers for
        // this role-specific hard cap: some helpers deduplicate by retaining every seen
        // event id before the caller receives a stream. Subscribe manually, read raw
        // relay messages for this subscription, and unsubscribe as soon as the local cap
        // or timeout is reached.
        let mut notifications = BroadcastStream::new(self.client.notifications());
        let subscription_id = SubscriptionId::generate();
        let client = self.client.clone();
        let cancel_subscription_id = subscription_id.clone();
        let bounds = CandidateBounds::for_limit(limit);
        // Boxed: this future is embedded (via role clients) inside already
        // enormous command futures, and inlining the collector state pushed
        // fi-cli's test-thread stack over its limit.
        Box::pin(subscribe_and_collect_capped(
            subscribed_relays(
                &self.client,
                self.client
                    .subscribe_with_id(subscription_id.clone(), filter, None),
            ),
            &mut notifications,
            &subscription_id,
            timeout,
            bounds,
            move || {
                drop(spawn("unsubscribe bounded Nostr query", async move {
                    client.unsubscribe(&cancel_subscription_id).await;
                }));
            },
        ))
        .await
        .map_err(|source| NostrClientError::Fetch { source })
    }

    /// Fetch a complete, bounded stored-event result before an absolute deadline.
    ///
    /// Unlike [`Self::fetch_events_capped`], this security-sensitive variant
    /// accepts a negative result only after EOSE. On a pooled client one
    /// relay's EOSE-complete answer is enough; slower relays merge
    /// best-effort until every subscribed relay answered or the deadline.
    /// Timeout, stream termination, relay `CLOSED`, notification failure, or
    /// a candidate/byte bound fails closed as an incomplete query while no
    /// relay has delivered a complete answer.
    pub async fn fetch_events_complete_capped(
        &self,
        filter: Filter,
        deadline: Instant,
        limit: u16,
    ) -> NostrClientResult<Vec<Event>> {
        self.fetch_events_complete_capped_with_policy(
            filter,
            deadline,
            CandidateBounds::for_limit(limit),
            ResourceCapPolicy::FailClosed,
        )
        .await
    }

    /// Fetch an EOSE-complete or bound-complete stored-event result before an
    /// absolute deadline.
    ///
    /// Same fail-closed semantics as [`Self::fetch_events_complete_capped`]
    /// with exactly one difference: reaching a local resource bound —
    /// collecting `limit` matching events, or retaining
    /// `retained_max_bytes` across the batch — completes the query
    /// successfully with the retained prefix instead of failing it. A result
    /// is therefore accepted only at EOSE, at the local candidate cap, or at
    /// the aggregate byte bound; timeout, stream termination, relay `CLOSED`,
    /// or notification failure before any of those points still fails closed
    /// as an incomplete query. Use this for enumerations whose caps are
    /// resource backstops rather than completeness requirements, so an
    /// attacker publishing one event more than a cap cannot turn the whole
    /// query into an error.
    pub(crate) async fn fetch_events_complete_or_capped(
        &self,
        filter: Filter,
        deadline: Instant,
        limit: u16,
        retained_max_bytes: usize,
    ) -> NostrClientResult<Vec<Event>> {
        self.fetch_events_complete_capped_with_policy(
            filter,
            deadline,
            CandidateBounds::for_limit_and_aggregate(limit, retained_max_bytes),
            ResourceCapPolicy::CompleteAtCap,
        )
        .await
    }

    async fn fetch_events_complete_capped_with_policy(
        &self,
        filter: Filter,
        deadline: Instant,
        bounds: CandidateBounds,
        cap_policy: ResourceCapPolicy,
    ) -> NostrClientResult<Vec<Event>> {
        let mut notifications = BroadcastStream::new(self.client.notifications());
        let subscription_id = SubscriptionId::generate();
        let client = self.client.clone();
        let cancel_subscription_id = subscription_id.clone();
        // Boxed for the same stack-size reason as the bounded fetch above.
        Box::pin(subscribe_and_collect_complete_capped(
            subscribed_relays(
                &self.client,
                self.client
                    .subscribe_with_id(subscription_id.clone(), filter, None),
            ),
            &mut notifications,
            &subscription_id,
            deadline,
            bounds,
            cap_policy,
            move || {
                drop(spawn("unsubscribe complete Nostr query", async move {
                    client.unsubscribe(&cancel_subscription_id).await;
                }));
            },
        ))
        .await
        .map_err(|error| match error {
            CompleteQueryError::Subscribe(source) => NostrClientError::Fetch { source },
            CompleteQueryError::Incomplete(reason) => NostrClientError::IncompleteQuery { reason },
        })
    }

    /// Fetch the first event matching a filter within the timeout.
    pub(crate) async fn fetch_one_event(
        &self,
        filter: Filter,
        timeout: Duration,
        context: &'static str,
    ) -> NostrClientResult<Event> {
        self.fetch_events(filter, timeout)
            .await?
            .into_iter()
            .next()
            .ok_or(NostrClientError::MissingEvent { context })
    }
}

/// Map a subscribe future's output to the set of relays a collector waits
/// on.
///
/// For a pool that is the relays that accepted the subscription *and* are
/// currently connected: the SDK reports success for a disconnected relay
/// too — it buffers the subscription until reconnect — and waiting on one
/// of those would hold every read at its full deadline during an outage.
///
/// A single-relay client keeps its exact pre-pool behavior instead: wait on
/// the one relay for the whole timeout, so a briefly disconnected relay may
/// reconnect, receive the buffered subscription, and still answer in time.
async fn subscribed_relays<E>(
    client: &Client,
    subscribe: impl Future<Output = Result<Output<()>, E>>,
) -> Result<HashSet<RelayUrl>, E> {
    let mut subscribed = subscribe.await?.success;
    let relays = client.relays().await;
    if relays.len() <= 1 {
        return Ok(relays.into_keys().collect());
    }
    subscribed.retain(|url| {
        relays
            .get(url)
            .is_some_and(|relay| relay.status() == RelayStatus::Connected)
    });
    Ok(subscribed)
}

/// A publish succeeds when at least one relay accepted the event; relays
/// that rejected it are logged and left to the caller's retry cadence. A
/// single-relay client behaves exactly as before: its only relay failing
/// means the success set is empty.
fn validate_publish_output(output: Output<EventId>) -> NostrClientResult<EventId> {
    if output.success.is_empty() {
        return Err(NostrClientError::PublishRejected {
            failed: if output.failed.is_empty() {
                "no relay accepted the event".to_owned()
            } else {
                format!("{:?}", output.failed)
            },
        });
    }
    if !output.failed.is_empty() {
        tracing::warn!(
            accepted = output.success.len(),
            failed = ?output.failed,
            "some relays did not accept a published event"
        );
    }
    Ok(*output.id())
}

fn parse_relay_url(relay_url: &str) -> NostrClientResult<RelayUrl> {
    RelayUrl::parse(relay_url).map_err(|source| NostrClientError::InvalidRelayUrl {
        url: relay_url.to_owned(),
        reason: source.to_string(),
    })
}

fn role_client_options() -> ClientOptions {
    ClientOptions::new()
        .relay_limits(role_relay_limits())
        .verify_subscriptions(true)
}

fn role_relay_limits() -> RelayLimits {
    let mut limits = RelayLimits::default();
    limits.events.max_size =
        Some(u32::try_from(ROLE_FETCHED_EVENT_MAX_BYTES).expect("role event bound fits u32"));
    limits
}

#[derive(Clone, Copy)]
struct CandidateBounds {
    /// Maximum number of matching relay events observed.
    count: u16,
    /// Maximum normalized size retained for one event.
    per_event_bytes: usize,
    /// Maximum normalized size retained across the returned batch.
    aggregate_bytes: usize,
}

impl CandidateBounds {
    fn for_limit(count: u16) -> Self {
        Self::for_limit_and_aggregate(
            count,
            ROLE_FETCHED_EVENT_MAX_BYTES.saturating_mul(usize::from(count)),
        )
    }

    /// Bounds with an aggregate byte cap decoupled from count x per-event:
    /// large enumerations use an explicit memory ceiling well below that
    /// product.
    fn for_limit_and_aggregate(count: u16, aggregate_bytes: usize) -> Self {
        Self {
            count,
            per_event_bytes: ROLE_FETCHED_EVENT_MAX_BYTES,
            aggregate_bytes,
        }
    }
}

async fn subscribe_and_collect_capped<S, E, C, F, FE>(
    subscribe: F,
    notifications: S,
    subscription_id: &SubscriptionId,
    timeout: Duration,
    bounds: CandidateBounds,
    cancel_subscription: C,
) -> Result<Vec<Event>, FE>
where
    S: Stream<Item = Result<RelayPoolNotification, E>> + Unpin,
    C: FnOnce(),
    F: Future<Output = Result<HashSet<RelayUrl>, FE>>,
{
    let mut cleanup = CleanupOnDrop::new(cancel_subscription);
    let subscribed = subscribe.await?;
    let events = collect_subscription_notifications_capped(
        notifications,
        subscription_id,
        subscribed,
        timeout,
        bounds,
    )
    .await;
    cleanup.run();
    Ok(events)
}

async fn collect_subscription_notifications_capped<S, E>(
    mut notifications: S,
    subscription_id: &SubscriptionId,
    mut pending_relays: HashSet<RelayUrl>,
    timeout: Duration,
    bounds: CandidateBounds,
) -> Vec<Event>
where
    S: Stream<Item = Result<RelayPoolNotification, E>> + Unpin,
{
    let mut events = Vec::new();
    let mut seen_event_ids = HashSet::new();
    let mut seen_events = 0_usize;
    let mut retained_bytes = 0_usize;
    let deadline = Instant::now() + timeout;

    // No relay accepted the subscription: nothing can arrive.
    if pending_relays.is_empty() {
        return events;
    }

    while seen_events < usize::from(bounds.count) {
        let next_notification = match runtime_timeout(
            deadline.saturating_duration_since(Instant::now()),
            notifications.next(),
        )
        .await
        {
            Ok(next_notification) => next_notification,
            Err(_) => break,
        };
        let Some(notification) = next_notification else {
            break;
        };
        let Ok(notification) = notification else {
            continue;
        };
        match subscription_notification(notification, subscription_id) {
            SubscriptionNotification::Event(event) => {
                // A pool repeats one signed event once per relay; only the
                // first copy counts against the bounds.
                if !seen_event_ids.insert(event.id) {
                    continue;
                }
                seen_events += 1;
                let event_bytes = event.as_json().len();
                if event_bytes > bounds.per_event_bytes
                    || retained_bytes.saturating_add(event_bytes) > bounds.aggregate_bytes
                {
                    continue;
                }
                retained_bytes += event_bytes;
                events.push(*event);
            }
            SubscriptionNotification::EndOfStoredEvents(relay_url)
            | SubscriptionNotification::Closed(relay_url) => {
                pending_relays.remove(&relay_url);
                if pending_relays.is_empty() {
                    break;
                }
            }
            SubscriptionNotification::Ignore => continue,
        }
    }

    events
}

/// How a complete query treats reaching a local resource bound.
#[derive(Clone, Copy, Eq, PartialEq)]
enum ResourceCapPolicy {
    /// One matching event beyond the count bound, or an event that would
    /// exceed the aggregate byte bound, fails the query closed.
    FailClosed,
    /// Reaching the count bound or the aggregate byte bound completes the
    /// query successfully with the retained prefix, without waiting for
    /// EOSE.
    CompleteAtCap,
}

#[allow(clippy::too_many_arguments)]
async fn subscribe_and_collect_complete_capped<S, E, C, F, FE>(
    subscribe: F,
    notifications: S,
    subscription_id: &SubscriptionId,
    deadline: Instant,
    bounds: CandidateBounds,
    cap_policy: ResourceCapPolicy,
    cancel_subscription: C,
) -> Result<Vec<Event>, CompleteQueryError<FE>>
where
    S: Stream<Item = Result<RelayPoolNotification, E>> + Unpin,
    C: FnOnce(),
    F: Future<Output = Result<HashSet<RelayUrl>, FE>>,
{
    let mut cleanup = CleanupOnDrop::new(cancel_subscription);
    let subscribe_result = runtime_timeout(
        deadline.saturating_duration_since(Instant::now()),
        subscribe,
    )
    .await
    .map_err(|_| CompleteQueryError::Incomplete("deadline elapsed before subscription started"))?;
    let subscribed = subscribe_result.map_err(CompleteQueryError::Subscribe)?;
    let events = collect_subscription_notifications_complete_capped(
        notifications,
        subscription_id,
        subscribed,
        deadline,
        bounds,
        cap_policy,
    )
    .await;
    cleanup.run();
    events.map_err(CompleteQueryError::Incomplete)
}

async fn collect_subscription_notifications_complete_capped<S, E>(
    mut notifications: S,
    subscription_id: &SubscriptionId,
    mut pending_relays: HashSet<RelayUrl>,
    deadline: Instant,
    bounds: CandidateBounds,
    cap_policy: ResourceCapPolicy,
) -> Result<Vec<Event>, &'static str>
where
    S: Stream<Item = Result<RelayPoolNotification, E>> + Unpin,
{
    let mut events = Vec::new();
    let mut seen_event_ids = HashSet::new();
    let mut retained_bytes = 0_usize;
    // Relays that finished their stored answer with EOSE. One is enough for
    // the query to succeed; the rest merge best-effort until the deadline.
    let mut complete_relays = 0_usize;

    if pending_relays.is_empty() {
        return Err("no relay accepted the subscription");
    }

    // Once one relay has answered completely, the merged events are a real
    // result — everything before the stopping point arrived in order, so
    // that relay's answer is intact. With no complete answer the query is
    // incomplete and reports why it stopped.
    fn settle(
        complete_relays: usize,
        events: Vec<Event>,
        stopped_because: &'static str,
    ) -> Result<Vec<Event>, &'static str> {
        if complete_relays > 0 {
            Ok(events)
        } else {
            Err(stopped_because)
        }
    }

    loop {
        let next_notification = match runtime_timeout(
            deadline.saturating_duration_since(Instant::now()),
            notifications.next(),
        )
        .await
        {
            Ok(next_notification) => next_notification,
            Err(_) => return settle(complete_relays, events, "deadline elapsed before EOSE"),
        };
        let Some(notification) = next_notification else {
            return settle(
                complete_relays,
                events,
                "notification stream ended before EOSE",
            );
        };
        let Ok(notification) = notification else {
            return settle(
                complete_relays,
                events,
                "notification stream failed before EOSE",
            );
        };
        match subscription_notification(notification, subscription_id) {
            SubscriptionNotification::Event(event) => {
                // A pool repeats one signed event once per relay; only the
                // first copy counts against the bounds.
                if !seen_event_ids.insert(event.id) {
                    continue;
                }
                // Resource-bound overflows also settle: once one relay has
                // delivered its complete answer, nothing a slower relay
                // sends afterwards may turn that answer into an error.
                if events.len() == usize::from(bounds.count) {
                    return settle(
                        complete_relays,
                        events,
                        "candidate count exceeded the local bound",
                    );
                }
                let event_bytes = event.as_json().len();
                if event_bytes > bounds.per_event_bytes {
                    return settle(
                        complete_relays,
                        events,
                        "candidate exceeded the per-event byte bound",
                    );
                }
                if retained_bytes.saturating_add(event_bytes) > bounds.aggregate_bytes {
                    // The aggregate byte bound is memory insurance. Under
                    // `CompleteAtCap` reaching it degrades to a successful,
                    // deliberately truncated result rather than an error the
                    // relay's other publishers could trigger at will.
                    return match cap_policy {
                        ResourceCapPolicy::CompleteAtCap => Ok(events),
                        ResourceCapPolicy::FailClosed => settle(
                            complete_relays,
                            events,
                            "candidates exceeded the aggregate byte bound",
                        ),
                    };
                }
                retained_bytes += event_bytes;
                events.push(*event);
                if cap_policy == ResourceCapPolicy::CompleteAtCap
                    && events.len() == usize::from(bounds.count)
                {
                    return Ok(events);
                }
            }
            SubscriptionNotification::EndOfStoredEvents(relay_url) => {
                if pending_relays.remove(&relay_url) {
                    complete_relays += 1;
                }
                if pending_relays.is_empty() {
                    return Ok(events);
                }
            }
            SubscriptionNotification::Closed(relay_url) => {
                pending_relays.remove(&relay_url);
                if pending_relays.is_empty() {
                    return settle(
                        complete_relays,
                        events,
                        "relay closed the subscription before EOSE",
                    );
                }
            }
            SubscriptionNotification::Ignore => {}
        }
    }
}

enum CompleteQueryError<E> {
    Subscribe(E),
    Incomplete(&'static str),
}

struct CleanupOnDrop<C>
where
    C: FnOnce(),
{
    cleanup: Option<C>,
}

impl<C> CleanupOnDrop<C>
where
    C: FnOnce(),
{
    fn new(cleanup: C) -> Self {
        Self {
            cleanup: Some(cleanup),
        }
    }

    fn run(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

impl<C> Drop for CleanupOnDrop<C>
where
    C: FnOnce(),
{
    fn drop(&mut self) {
        self.run();
    }
}

enum SubscriptionNotification {
    Event(Box<Event>),
    EndOfStoredEvents(RelayUrl),
    Closed(RelayUrl),
    Ignore,
}

fn subscription_notification(
    notification: RelayPoolNotification,
    expected_subscription_id: &SubscriptionId,
) -> SubscriptionNotification {
    match notification {
        RelayPoolNotification::Message {
            message:
                RelayMessage::Event {
                    subscription_id,
                    event,
                },
            ..
        } if subscription_id.as_ref() == expected_subscription_id => {
            SubscriptionNotification::Event(Box::new(event.into_owned()))
        }
        RelayPoolNotification::Message {
            message: RelayMessage::EndOfStoredEvents(subscription_id),
            relay_url,
        } if subscription_id.as_ref() == expected_subscription_id => {
            SubscriptionNotification::EndOfStoredEvents(relay_url)
        }
        RelayPoolNotification::Message {
            message: RelayMessage::Closed {
                subscription_id, ..
            },
            relay_url,
        } if subscription_id.as_ref() == expected_subscription_id => {
            SubscriptionNotification::Closed(relay_url)
        }
        _ => SubscriptionNotification::Ignore,
    }
}
