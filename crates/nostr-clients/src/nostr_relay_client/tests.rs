//! Tests for shared Nostr relay client helpers.

use std::{
    borrow::Cow,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{Stream, future::poll_fn};
use nostr_sdk::{
    Event, EventBuilder, Keys, RelayMessage, RelayPoolNotification, RelayUrl, SubscriptionId, Tag,
};

use super::*;

struct PanicAfterLimitNotifications {
    notifications: VecDeque<Result<RelayPoolNotification, ()>>,
    max_polls: usize,
    polls: Arc<AtomicUsize>,
}

struct PendingNotifications {
    polls: Arc<AtomicUsize>,
}

impl Stream for PendingNotifications {
    type Item = Result<RelayPoolNotification, ()>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Pending
    }
}

impl Stream for PanicAfterLimitNotifications {
    type Item = Result<RelayPoolNotification, ()>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polls = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            polls <= self.max_polls,
            "bounded subscription stream was polled after the local cap"
        );

        Poll::Ready(self.notifications.pop_front())
    }
}

fn signed_text_event(content: &str) -> Event {
    EventBuilder::text_note(content)
        .sign_with_keys(&Keys::generate())
        .expect("test event signs")
}

fn relay_a() -> RelayUrl {
    RelayUrl::parse("wss://relay.example").expect("test relay URL parses")
}

fn relay_b() -> RelayUrl {
    RelayUrl::parse("wss://second.example").expect("test relay URL parses")
}

fn one_relay() -> HashSet<RelayUrl> {
    HashSet::from([relay_a()])
}

fn two_relays() -> HashSet<RelayUrl> {
    HashSet::from([relay_a(), relay_b()])
}

#[test]
fn role_relay_limits_bound_all_event_kinds() {
    let limits = role_relay_limits();
    assert_eq!(
        limits.events.get_max_size(&nostr_sdk::Kind::TextNote),
        Some(u32::try_from(ROLE_FETCHED_EVENT_MAX_BYTES).expect("role event bound fits u32"))
    );
}

fn event_notification_from(
    relay_url: RelayUrl,
    subscription_id: &SubscriptionId,
    event: Event,
) -> Result<RelayPoolNotification, ()> {
    Ok(RelayPoolNotification::Message {
        relay_url,
        message: RelayMessage::Event {
            subscription_id: Cow::Owned(subscription_id.clone()),
            event: Cow::Owned(event),
        },
    })
}

fn event_notification(
    subscription_id: &SubscriptionId,
    event: Event,
) -> Result<RelayPoolNotification, ()> {
    event_notification_from(relay_a(), subscription_id, event)
}

fn eose_notification_from(
    relay_url: RelayUrl,
    subscription_id: &SubscriptionId,
) -> Result<RelayPoolNotification, ()> {
    Ok(RelayPoolNotification::Message {
        relay_url,
        message: RelayMessage::EndOfStoredEvents(Cow::Owned(subscription_id.clone())),
    })
}

fn eose_notification(subscription_id: &SubscriptionId) -> Result<RelayPoolNotification, ()> {
    eose_notification_from(relay_a(), subscription_id)
}

fn closed_notification_from(
    relay_url: RelayUrl,
    subscription_id: &SubscriptionId,
) -> Result<RelayPoolNotification, ()> {
    Ok(RelayPoolNotification::Message {
        relay_url,
        message: RelayMessage::Closed {
            subscription_id: Cow::Owned(subscription_id.clone()),
            message: Cow::Borrowed("test close"),
        },
    })
}

fn closed_notification(subscription_id: &SubscriptionId) -> Result<RelayPoolNotification, ()> {
    closed_notification_from(relay_a(), subscription_id)
}

#[tokio::test]
async fn collect_subscription_notifications_stops_and_cancels_at_limit() {
    let subscription_id = SubscriptionId::generate();
    let events = [
        signed_text_event("first"),
        signed_text_event("second"),
        signed_text_event("third"),
    ];
    let polls = Arc::new(AtomicUsize::new(0));
    let canceled = Arc::new(AtomicBool::new(false));
    let notifications = PanicAfterLimitNotifications {
        notifications: events
            .iter()
            .cloned()
            .map(|event| event_notification(&subscription_id, event))
            .collect(),
        max_polls: 2,
        polls: Arc::clone(&polls),
    };
    let cancel_flag = Arc::clone(&canceled);

    let collected = subscribe_and_collect_capped(
        async { Ok::<_, ()>(one_relay()) },
        notifications,
        &subscription_id,
        Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    )
    .await
    .expect("test subscription succeeds");

    assert_eq!(collected, events[..2]);
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn collect_subscription_notifications_accepts_short_stream() {
    let subscription_id = SubscriptionId::generate();
    let events = [signed_text_event("only")];
    let canceled = Arc::new(AtomicBool::new(false));
    let notifications = PanicAfterLimitNotifications {
        notifications: events
            .iter()
            .cloned()
            .map(|event| event_notification(&subscription_id, event))
            .collect(),
        max_polls: 2,
        polls: Arc::new(AtomicUsize::new(0)),
    };
    let cancel_flag = Arc::clone(&canceled);

    let collected = subscribe_and_collect_capped(
        async { Ok::<_, ()>(one_relay()) },
        notifications,
        &subscription_id,
        Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    )
    .await
    .expect("test subscription succeeds");

    assert_eq!(collected, events);
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn collect_subscription_notifications_stops_and_cancels_at_eose() {
    let subscription_id = SubscriptionId::generate();
    let event = signed_text_event("only");
    let canceled = Arc::new(AtomicBool::new(false));
    let notifications = PanicAfterLimitNotifications {
        notifications: VecDeque::from([
            event_notification(&subscription_id, event.clone()),
            eose_notification(&subscription_id),
        ]),
        max_polls: 2,
        polls: Arc::new(AtomicUsize::new(0)),
    };
    let cancel_flag = Arc::clone(&canceled);

    let collected = subscribe_and_collect_capped(
        async { Ok::<_, ()>(one_relay()) },
        notifications,
        &subscription_id,
        Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    )
    .await
    .expect("test subscription succeeds");

    assert_eq!(collected, [event]);
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn capped_collection_waits_for_every_subscribed_relay() {
    // One relay's EOSE must not end a pooled query while another subscribed
    // relay still owes its answer.
    let subscription_id = SubscriptionId::generate();
    let second = signed_text_event("from the slow relay");
    let notifications = VecDeque::from([
        eose_notification_from(relay_a(), &subscription_id),
        event_notification_from(relay_b(), &subscription_id, second.clone()),
        eose_notification_from(relay_b(), &subscription_id),
    ]);

    let collected = collect_subscription_notifications_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Duration::from_secs(1),
        CandidateBounds::for_limit(4),
    )
    .await;

    assert_eq!(collected, [second]);
}

#[tokio::test]
async fn capped_collection_counts_cross_relay_copies_once() {
    // The pool repeats one signed event once per relay. The copy must not
    // consume a second cap slot or hide a later distinct event.
    let subscription_id = SubscriptionId::generate();
    let duplicated = signed_text_event("published everywhere");
    let distinct = signed_text_event("second distinct event");
    let notifications = VecDeque::from([
        event_notification_from(relay_a(), &subscription_id, duplicated.clone()),
        event_notification_from(relay_b(), &subscription_id, duplicated.clone()),
        event_notification_from(relay_b(), &subscription_id, distinct.clone()),
        eose_notification_from(relay_a(), &subscription_id),
        eose_notification_from(relay_b(), &subscription_id),
    ]);

    let collected = collect_subscription_notifications_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Duration::from_secs(1),
        CandidateBounds::for_limit(2),
    )
    .await;

    assert_eq!(collected, [duplicated, distinct]);
}

#[tokio::test]
async fn capped_collection_with_no_subscribed_relay_returns_nothing() {
    let subscription_id = SubscriptionId::generate();
    let polls = Arc::new(AtomicUsize::new(0));

    let collected = collect_subscription_notifications_capped(
        PendingNotifications {
            polls: Arc::clone(&polls),
        },
        &subscription_id,
        HashSet::new(),
        Duration::from_secs(60),
        CandidateBounds::for_limit(2),
    )
    .await;

    assert!(collected.is_empty());
    assert_eq!(polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn complete_collection_accepts_empty_only_after_eose() {
    let subscription_id = SubscriptionId::generate();
    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter([eose_notification(&subscription_id)]),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("EOSE completes the negative query");

    assert!(collected.is_empty());
}

#[tokio::test]
async fn complete_collection_rejects_timeout_before_eose() {
    let subscription_id = SubscriptionId::generate();
    let error = collect_subscription_notifications_complete_capped(
        PendingNotifications {
            polls: Arc::new(AtomicUsize::new(0)),
        },
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_millis(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("timeout is incomplete");

    assert_eq!(error, "deadline elapsed before EOSE");
}

#[tokio::test]
async fn complete_collection_rejects_subscription_timeout() {
    let subscription_id = SubscriptionId::generate();
    let canceled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&canceled);
    let result = subscribe_and_collect_complete_capped(
        std::future::pending::<Result<HashSet<RelayUrl>, ()>>(),
        tokio_stream::empty::<Result<RelayPoolNotification, ()>>(),
        &subscription_id,
        Instant::now() + Duration::from_millis(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(CompleteQueryError::Incomplete(
            "deadline elapsed before subscription started"
        ))
    ));
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_a_pending_complete_query_runs_subscription_cleanup() {
    let subscription_id = SubscriptionId::generate();
    let polls = Arc::new(AtomicUsize::new(0));
    let canceled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&canceled);
    let pending_polls = Arc::clone(&polls);
    let task = tokio::spawn(async move {
        subscribe_and_collect_complete_capped(
            async { Ok::<_, ()>(one_relay()) },
            PendingNotifications {
                polls: pending_polls,
            },
            &subscription_id,
            Instant::now() + Duration::from_secs(60),
            CandidateBounds::for_limit(2),
            ResourceCapPolicy::FailClosed,
            || {
                cancel_flag.store(true, Ordering::SeqCst);
            },
        )
        .await
    });
    poll_fn(|context| {
        if 0 < polls.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            context.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;

    task.abort();
    let join_error = match task.await {
        Ok(_) => panic!("query task was not cancelled"),
        Err(error) => error,
    };
    assert!(join_error.is_cancelled());
    assert!(
        canceled.load(Ordering::SeqCst),
        "dropping a pending complete query runs its unsubscribe cleanup"
    );
}

#[tokio::test]
async fn complete_collection_rejects_stream_end_and_close_before_eose() {
    let subscription_id = SubscriptionId::generate();
    let ended = collect_subscription_notifications_complete_capped(
        tokio_stream::empty::<Result<RelayPoolNotification, ()>>(),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("stream end is incomplete");
    assert_eq!(ended, "notification stream ended before EOSE");

    let closed = collect_subscription_notifications_complete_capped(
        tokio_stream::iter([closed_notification(&subscription_id)]),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("CLOSED is incomplete");
    assert_eq!(closed, "relay closed the subscription before EOSE");
}

#[tokio::test]
async fn complete_collection_rejects_notification_failure_before_eose() {
    let subscription_id = SubscriptionId::generate();
    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::iter([Err::<RelayPoolNotification, ()>(())]),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("notification failure is incomplete");

    assert_eq!(error, "notification stream failed before EOSE");
}

#[tokio::test]
async fn complete_collection_rejects_a_subscription_no_relay_accepted() {
    let subscription_id = SubscriptionId::generate();
    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::empty::<Result<RelayPoolNotification, ()>>(),
        &subscription_id,
        HashSet::new(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("a subscription no relay accepted is incomplete");

    assert_eq!(error, "no relay accepted the subscription");
}

#[tokio::test]
async fn complete_collection_finishes_when_every_relay_answers() {
    // Fast path: both relays complete before the deadline; the merged,
    // deduplicated result returns without waiting out the deadline.
    let subscription_id = SubscriptionId::generate();
    let duplicated = signed_text_event("published everywhere");
    let only_on_b = signed_text_event("retained by one relay");
    let notifications = [
        event_notification_from(relay_a(), &subscription_id, duplicated.clone()),
        eose_notification_from(relay_a(), &subscription_id),
        event_notification_from(relay_b(), &subscription_id, duplicated.clone()),
        event_notification_from(relay_b(), &subscription_id, only_on_b.clone()),
        eose_notification_from(relay_b(), &subscription_id),
    ];

    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(4),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("every relay answered");

    assert_eq!(collected, [duplicated, only_on_b]);
}

#[tokio::test]
async fn complete_collection_succeeds_at_deadline_with_one_complete_answer() {
    // One relay answered completely; the other stalls. The stalled relay
    // costs the deadline, not the query.
    let subscription_id = SubscriptionId::generate();
    let event = signed_text_event("from the live relay");
    let live_relay_answers = VecDeque::from([
        event_notification_from(relay_a(), &subscription_id, event.clone()),
        eose_notification_from(relay_a(), &subscription_id),
    ]);
    struct AnswersThenPending {
        answers: VecDeque<Result<RelayPoolNotification, ()>>,
    }
    impl Stream for AnswersThenPending {
        type Item = Result<RelayPoolNotification, ()>;
        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            match self.answers.pop_front() {
                Some(answer) => Poll::Ready(Some(answer)),
                None => Poll::Pending,
            }
        }
    }

    let collected = collect_subscription_notifications_complete_capped(
        AnswersThenPending {
            answers: live_relay_answers,
        },
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_millis(50),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("one complete answer carries the query past the stalled relay");

    assert_eq!(collected, [event]);
}

#[tokio::test]
async fn complete_collection_survives_a_close_after_one_complete_answer() {
    let subscription_id = SubscriptionId::generate();
    let event = signed_text_event("from the live relay");
    let notifications = [
        event_notification_from(relay_a(), &subscription_id, event.clone()),
        eose_notification_from(relay_a(), &subscription_id),
        closed_notification_from(relay_b(), &subscription_id),
    ];

    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("a close after one complete answer does not fail the query");

    assert_eq!(collected, [event]);
}

#[tokio::test]
async fn complete_collection_fails_when_every_relay_closes_without_eose() {
    let subscription_id = SubscriptionId::generate();
    let notifications = [
        closed_notification_from(relay_a(), &subscription_id),
        closed_notification_from(relay_b(), &subscription_id),
    ];

    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("no relay delivered a complete answer");

    assert_eq!(error, "relay closed the subscription before EOSE");
}

#[tokio::test]
async fn complete_collection_counts_cross_relay_copies_once() {
    // Under the fail-closed cap a duplicated copy must not count as the
    // "one event beyond the bound" that fails the query.
    let subscription_id = SubscriptionId::generate();
    let duplicated = signed_text_event("published everywhere");
    let notifications = [
        event_notification_from(relay_a(), &subscription_id, duplicated.clone()),
        eose_notification_from(relay_a(), &subscription_id),
        event_notification_from(relay_b(), &subscription_id, duplicated.clone()),
        eose_notification_from(relay_b(), &subscription_id),
    ];

    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(1),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("the cross-relay copy is not a cap violation");

    assert_eq!(collected, [duplicated]);
}

#[tokio::test]
async fn complete_collection_settles_when_a_slower_relay_overflows_a_bound() {
    // Relay A already delivered its complete answer; a distinct overflow
    // event from slower relay B must not turn that answer into an error.
    let subscription_id = SubscriptionId::generate();
    let from_a = signed_text_event("relay A's complete answer");
    let notifications = [
        event_notification_from(relay_a(), &subscription_id, from_a.clone()),
        eose_notification_from(relay_a(), &subscription_id),
        event_notification_from(relay_b(), &subscription_id, signed_text_event("overflow")),
    ];

    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        two_relays(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(1),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect("a slower relay's overflow settles to the complete answer");

    assert_eq!(collected, [from_a]);
}

#[tokio::test]
async fn complete_collection_rejects_candidate_cap_exhaustion() {
    let subscription_id = SubscriptionId::generate();
    let notifications = [
        event_notification(&subscription_id, signed_text_event("first")),
        event_notification(&subscription_id, signed_text_event("overflow")),
        eose_notification(&subscription_id),
    ];
    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(1),
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("a candidate beyond the hard cap is incomplete");

    assert_eq!(error, "candidate count exceeded the local bound");
}

#[tokio::test]
async fn cap_tolerant_complete_collection_accepts_a_reached_cap_without_eose() {
    // One spam event beyond the cap must not brick the enumeration: reaching
    // the local candidate cap is a successful, deliberately truncated result.
    let subscription_id = SubscriptionId::generate();
    let first = signed_text_event("first");
    let notifications = [
        event_notification(&subscription_id, first.clone()),
        event_notification(&subscription_id, signed_text_event("beyond the cap")),
    ];
    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(1),
        ResourceCapPolicy::CompleteAtCap,
    )
    .await
    .expect("reaching the candidate cap completes the query");

    assert_eq!(collected, [first]);
}

#[tokio::test]
async fn cap_tolerant_complete_collection_accepts_an_aggregate_byte_bound_hit() {
    // The aggregate byte bound is memory insurance, not a completeness
    // requirement: an event that would push the batch past it completes the
    // enumeration with the retained prefix instead of failing it, so a
    // publisher flooding the relay with large events cannot brick discovery.
    let subscription_id = SubscriptionId::generate();
    let first = signed_text_event("first");
    let second = signed_text_event("second overflows the aggregate bound");
    let first_bytes = first.as_json().len();
    let second_bytes = second.as_json().len();
    let notifications = [
        event_notification(&subscription_id, first.clone()),
        event_notification(&subscription_id, second),
    ];
    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds {
            count: 4,
            per_event_bytes: first_bytes.max(second_bytes),
            aggregate_bytes: first_bytes + second_bytes - 1,
        },
        ResourceCapPolicy::CompleteAtCap,
    )
    .await
    .expect("hitting the aggregate byte bound completes the query");

    assert_eq!(collected, [first]);
}

#[tokio::test]
async fn cap_tolerant_complete_collection_rejects_a_stall_below_the_cap() {
    // Fewer events than the cap without EOSE is still an incomplete answer.
    let subscription_id = SubscriptionId::generate();
    let notifications = tokio_stream::iter([event_notification(
        &subscription_id,
        signed_text_event("only"),
    )]);
    let error = collect_subscription_notifications_complete_capped(
        notifications,
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::CompleteAtCap,
    )
    .await
    .expect_err("a stalled answer below the cap is incomplete");

    assert_eq!(error, "notification stream ended before EOSE");
}

#[tokio::test]
async fn cap_tolerant_complete_collection_still_requires_eose_below_the_cap() {
    let subscription_id = SubscriptionId::generate();
    let event = signed_text_event("only");
    let notifications = [
        event_notification(&subscription_id, event.clone()),
        eose_notification(&subscription_id),
    ];
    let collected = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds::for_limit(2),
        ResourceCapPolicy::CompleteAtCap,
    )
    .await
    .expect("EOSE completes the below-cap query");

    assert_eq!(collected, [event]);
}

#[tokio::test]
async fn complete_collection_rejects_per_event_byte_exhaustion() {
    let subscription_id = SubscriptionId::generate();
    let event = signed_text_event("oversized");
    let event_bytes = event.as_json().len();
    let notifications = [
        event_notification(&subscription_id, event),
        eose_notification(&subscription_id),
    ];
    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds {
            count: 2,
            per_event_bytes: event_bytes - 1,
            aggregate_bytes: event_bytes,
        },
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("per-event byte exhaustion is incomplete");

    assert_eq!(error, "candidate exceeded the per-event byte bound");
}

#[tokio::test]
async fn complete_collection_rejects_aggregate_byte_exhaustion() {
    let subscription_id = SubscriptionId::generate();
    let first = signed_text_event("first");
    let second = signed_text_event("second");
    let first_bytes = first.as_json().len();
    let second_bytes = second.as_json().len();
    let notifications = [
        event_notification(&subscription_id, first),
        event_notification(&subscription_id, second),
        eose_notification(&subscription_id),
    ];
    let error = collect_subscription_notifications_complete_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Instant::now() + Duration::from_secs(1),
        CandidateBounds {
            count: 2,
            per_event_bytes: first_bytes.max(second_bytes),
            aggregate_bytes: first_bytes + second_bytes - 1,
        },
        ResourceCapPolicy::FailClosed,
    )
    .await
    .expect_err("aggregate byte exhaustion is incomplete");

    assert_eq!(error, "candidates exceeded the aggregate byte bound");
}

#[tokio::test]
async fn collect_subscription_notifications_cleans_up_when_dropped_while_pending() {
    let subscription_id = SubscriptionId::generate();
    let polls = Arc::new(AtomicUsize::new(0));
    let canceled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&canceled);
    let mut future = Box::pin(subscribe_and_collect_capped(
        async { Ok::<_, ()>(one_relay()) },
        PendingNotifications {
            polls: Arc::clone(&polls),
        },
        &subscription_id,
        Duration::from_secs(60),
        CandidateBounds::for_limit(1),
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    ));

    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("bounded collection unexpectedly completed"),
    })
    .await;

    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert!(!canceled.load(Ordering::SeqCst));
    drop(future);
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn subscribe_failure_cleans_up_before_notifications() {
    let subscription_id = SubscriptionId::generate();
    let polls = Arc::new(AtomicUsize::new(0));
    let canceled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&canceled);

    let result = subscribe_and_collect_capped(
        async { Err::<HashSet<RelayUrl>, _>("subscribe failed") },
        PendingNotifications {
            polls: Arc::clone(&polls),
        },
        &subscription_id,
        Duration::from_secs(60),
        CandidateBounds::for_limit(1),
        || {
            cancel_flag.store(true, Ordering::SeqCst);
        },
    )
    .await;

    assert_eq!(result.expect_err("subscription fails"), "subscribe failed");
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert!(canceled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn per_event_bytes_and_seen_event_count_are_hard_bounded() {
    let subscription_id = SubscriptionId::generate();
    let oversized = EventBuilder::text_note("small")
        .tag(Tag::custom(
            nostr_sdk::TagKind::custom("oversized"),
            ["x".repeat(2_000)],
        ))
        .sign_with_keys(&Keys::generate())
        .expect("test event signs");
    let accepted = signed_text_event("accepted");
    let hidden_after_count_bound = signed_text_event("hidden");
    let polls = Arc::new(AtomicUsize::new(0));
    let notifications = PanicAfterLimitNotifications {
        notifications: VecDeque::from([
            event_notification(&subscription_id, oversized),
            event_notification(&subscription_id, accepted.clone()),
            event_notification(&subscription_id, hidden_after_count_bound),
        ]),
        max_polls: 2,
        polls: Arc::clone(&polls),
    };

    let collected = collect_subscription_notifications_capped(
        notifications,
        &subscription_id,
        one_relay(),
        Duration::from_secs(1),
        CandidateBounds {
            count: 2,
            per_event_bytes: 1_000,
            aggregate_bytes: 1_000,
        },
    )
    .await;

    assert_eq!(collected, [accepted]);
    assert_eq!(polls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retained_aggregate_bytes_are_hard_bounded() {
    let subscription_id = SubscriptionId::generate();
    let first = signed_text_event("first");
    let second = signed_text_event("second");
    let first_bytes = first.as_json().len();
    let second_bytes = second.as_json().len();
    let notifications = VecDeque::from([
        event_notification(&subscription_id, first.clone()),
        event_notification(&subscription_id, second),
    ]);

    let collected = collect_subscription_notifications_capped(
        tokio_stream::iter(notifications),
        &subscription_id,
        one_relay(),
        Duration::from_secs(1),
        CandidateBounds {
            count: 2,
            per_event_bytes: first_bytes.max(second_bytes),
            aggregate_bytes: first_bytes + second_bytes - 1,
        },
    )
    .await;

    assert_eq!(collected, [first]);
}

#[test]
fn publish_succeeds_on_one_ack_and_fails_on_none() {
    let event_id = signed_text_event("published").id;

    let all_accepted = Output {
        val: event_id,
        success: HashSet::from([relay_a(), relay_b()]),
        failed: std::collections::HashMap::new(),
    };
    assert_eq!(
        validate_publish_output(all_accepted).expect("every relay accepted"),
        event_id
    );

    let one_accepted = Output {
        val: event_id,
        success: HashSet::from([relay_a()]),
        failed: std::collections::HashMap::from([(relay_b(), "rejected".to_owned())]),
    };
    assert_eq!(
        validate_publish_output(one_accepted).expect("one ack is a successful publish"),
        event_id
    );

    let none_accepted = Output {
        val: event_id,
        success: HashSet::new(),
        failed: std::collections::HashMap::from([(relay_a(), "rejected".to_owned())]),
    };
    validate_publish_output(none_accepted).expect_err("no ack fails the publish");
}
