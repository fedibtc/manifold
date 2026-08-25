use super::*;

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[allow(clippy::type_complexity)]
fn pending_open(
    seq: u64,
) -> (
    tokio::sync::watch::Sender<Option<Arc<Result<ClientHandleArc, String>>>>,
    PendingOpen,
) {
    pending_open_aged(seq, std::time::Duration::ZERO)
}

/// A pending open that started `age` ago.
#[allow(clippy::type_complexity)]
fn pending_open_aged(
    seq: u64,
    age: std::time::Duration,
) -> (
    tokio::sync::watch::Sender<Option<Arc<Result<ClientHandleArc, String>>>>,
    PendingOpen,
) {
    let (tx, rx) = tokio::sync::watch::channel(None);
    (
        tx,
        PendingOpen {
            done: rx,
            seq,
            started_at: std::time::Instant::now()
                .checked_sub(age)
                .expect("test clock is past the epoch"),
            stuck_reported: false,
        },
    )
}

/// A stuck open is reported once, and only after the threshold.
///
/// Reporting is the entire remedy: nothing cancels the open, so the pending
/// slot is held until the process restarts. A report that repeated on every
/// later open would bury the log for as long as the fault lasts, and one
/// that fired early would train an operator to ignore it.
#[test]
fn a_stuck_open_is_reported_once() {
    let mut inner = TargetFedimintClientsInner::default();
    let now = std::time::Instant::now();

    let (_young, open) = pending_open_aged(1, STUCK_OPEN_REPORT_AFTER / 2);
    inner.opens.insert("young".to_owned(), open);
    assert!(
        inner.take_newly_stuck_opens(now).is_empty(),
        "an open below the threshold is not stuck"
    );

    let (_stuck, open) = pending_open_aged(2, STUCK_OPEN_REPORT_AFTER * 2);
    inner.opens.insert("stuck".to_owned(), open);
    let reported = inner.take_newly_stuck_opens(now);
    assert_eq!(
        reported
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["stuck"],
        "only the open past the threshold is reported"
    );
    assert!(
        reported[0].1 >= STUCK_OPEN_REPORT_AFTER,
        "the report carries the age the operator needs"
    );

    assert!(
        inner.take_newly_stuck_opens(now).is_empty(),
        "the same stuck open is not reported twice"
    );
}

/// The refusal names its occupants, oldest first.
///
/// "At capacity" alone tells an operator the budget is full and not which
/// target filled it, and choosing which federation to stop endorsing is the
/// only action available to them before a restart.
#[test]
fn the_capacity_report_names_occupants_oldest_first() {
    let mut inner = TargetFedimintClientsInner::default();
    let now = std::time::Instant::now();

    for (federation_id, age) in [
        ("newest", std::time::Duration::from_secs(1)),
        ("oldest", std::time::Duration::from_secs(900)),
        ("middle", std::time::Duration::from_secs(60)),
    ] {
        let (_tx, open) = pending_open_aged(1, age);
        inner.opens.insert(federation_id.to_owned(), open);
    }

    let ages = inner.pending_open_ages(now);
    assert_eq!(
        ages.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["oldest", "middle", "newest"]
    );
}

/// Pending opens are bounded, and bounded separately.
///
/// The concern is unchanged from when opens shared the client ceiling: if
/// nothing bounded them, targets that accept a connection and never answer
/// would pin a RocksDB handle each, whatever the ceiling said. What changed
/// is where the bound lives. Sharing the ceiling bounded them only in
/// arithmetic — `least_recently_used_idle` can evict an installed client and
/// never a pending open, so once opens filled the ceiling `make_room` found
/// no victim and opened anyway, and the set grew without bound after all.
#[test]
fn pending_opens_have_their_own_bound() {
    let mut inner = TargetFedimintClientsInner::default();
    assert!(inner.may_start_open(), "an empty pool may start an open");

    for index in 0..MAX_PENDING_OPENS {
        let (_tx, open) = pending_open(index as u64);
        inner.opens.insert(format!("federation-{index}"), open);
    }
    assert!(
        !inner.may_start_open(),
        "a full pending set refuses another open"
    );

    inner.opens.remove("federation-0");
    assert!(
        inner.may_start_open(),
        "a resolved open gives its pending slot back"
    );
}

/// A pending open does not consume a client slot.
///
/// This is the crowding that made the shared ceiling worse than no ceiling:
/// an FI driving federations whose config download never completes filled
/// the ceiling with opens that could not be evicted, so healthy federations
/// that were already installed lost their slots to targets that had never
/// answered.
#[test]
fn pending_opens_do_not_crowd_out_installed_clients() {
    let max = NonZeroUsize::new(1).unwrap();
    let mut inner = TargetFedimintClientsInner::default();

    let (_tx, open) = pending_open(1);
    inner.opens.insert("stuck".to_owned(), open);
    assert!(
        inner.has_room(max),
        "a pending open must leave the client ceiling alone"
    );
}

#[test]
fn the_victim_is_the_oldest_idle_federation() {
    let usage = ids(&["a", "b", "c"]);
    assert_eq!(
        least_recently_used_idle(&usage, |_| true),
        Some("a".to_owned())
    );
    // `a` is mid-deposit, so the next oldest goes instead. Stopping at the
    // first busy entry would leave the pool unable to reclaim anything for
    // as long as one worker held one client.
    assert_eq!(
        least_recently_used_idle(&usage, |id| id != "a"),
        Some("b".to_owned())
    );
    assert_eq!(least_recently_used_idle(&usage, |_| false), None);
}

#[test]
fn using_a_federation_moves_it_off_the_eviction_front() {
    let mut inner = TargetFedimintClientsInner::default();
    for federation_id in ["a", "b", "c"] {
        inner.touch(federation_id);
    }
    assert_eq!(inner.usage, ids(&["a", "b", "c"]));

    inner.touch("a");
    assert_eq!(inner.usage, ids(&["b", "c", "a"]));

    // One entry per federation however often it is used: the order is a
    // ranking, and a repeat that appended would let a hot federation push
    // the whole history off the front.
    inner.touch("a");
    assert_eq!(inner.usage, ids(&["b", "c", "a"]));
}

#[test]
fn taking_a_federation_removes_it_from_the_order() {
    let mut inner = TargetFedimintClientsInner::default();
    for federation_id in ["a", "b"] {
        inner.touch(federation_id);
    }

    assert!(inner.take("a").is_none(), "no client was ever inserted");
    assert_eq!(
        inner.usage,
        ids(&["b"]),
        "a closed federation must leave the order, or it stays the \
         perpetual victim and every later pass wastes its pick on it"
    );
}

#[test]
fn only_unheld_lock_entries_are_pruned() {
    let mut inner = TargetFedimintClientsInner::default();
    inner.locks.insert("a".to_owned(), Arc::default());
    inner.locks.insert("b".to_owned(), Arc::default());
    let held = inner.locks.get("b").expect("just inserted").clone();

    inner.prune_idle_locks();

    assert_eq!(inner.locks.keys().collect::<Vec<_>>(), vec!["b"]);
    drop(held);
    inner.prune_idle_locks();
    assert!(inner.locks.is_empty());
}

fn test_federations_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join("fedi-flip-tests")
        .join(format!("{name}-{}-{nanos}", std::process::id()))
}

/// The stability worker's per-item budget drops the item's future at
/// whatever await it is suspended on, and one of those is the target client
/// open. The open must therefore survive its caller.
///
/// Before the pool owned it, the drop landed inside
/// `ClientBuilder::build_stopped`, whose locally created `TaskGroup` has no
/// `Drop`: a config-refresh task holding a clone of the client database was
/// left detached, holding the RocksDB file lock for the life of the
/// process, with the pool holding no handle to it. The next pass then
/// called `open_rocksdb` again and blocked forever on `flock` inside a
/// `block_in_place` section, which `tokio::time::timeout` cannot interrupt.
///
/// This test does not reproduce that leak, and should not be read as
/// doing so: the unroutable endpoint parks the open in `preview`'s
/// retry backoff, well before `build_stopped` exists to spawn anything.
/// What it pins is the two properties that make the leak unreachable —
/// the pool owns the open its cancelled caller started, and a second
/// caller attaches to that same open rather than starting another
/// against a database the first one is holding.
///
/// Multi-thread flavour on purpose: a `block_in_place` on the caller's own
/// future would stop it polling its own `timeout`, and a current-thread
/// runtime would hide that.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_open_stays_owned_by_the_pool() -> anyhow::Result<()> {
    let federations_dir = test_federations_dir("cancelled-open");
    std::fs::create_dir_all(&federations_dir)?;

    // Unroutable on purpose: the join has to still be in flight when the
    // caller gives up, because that is the state the wedge needed.
    let invite = InviteCode::new(
        SafeUrl::parse("ws://10.255.255.1:7000")?,
        fedimint_core::PeerId::from(0),
        fedimint_core::config::FederationId::dummy(),
        None,
    )
    .to_string();

    let pool = TargetFedimintClients::new(
        NonZeroUsize::new(2).unwrap(),
        // This test is about who owns an open, not about which endpoints
        // are dialable, and its endpoint is deliberately unroutable — so
        // the permissive policy is what keeps the address check from being
        // the thing that stops it.
        EndpointPolicy::AllowPrivate,
    );
    let budget = std::time::Duration::from_millis(250);

    let first = tokio::time::timeout(
        budget,
        pool.create_or_load(&federations_dir, "federation-stuck", &invite, None),
    )
    .await;
    assert!(
        first.is_err(),
        "the unroutable open should still be in flight when the budget expires"
    );
    assert_eq!(
        pool.pending_open_count().await,
        1,
        "the pool must own the open its cancelled caller started"
    );
    let first_seq = pool
        .pending_open_seq("federation-stuck")
        .await
        .expect("the open is still in flight");

    // That this returns at all is half the point: a caller that entered an
    // uncancellable blocking section could not poll its own timeout.
    let second = tokio::time::timeout(
        budget,
        pool.create_or_load(&federations_dir, "federation-stuck", &invite, None),
    )
    .await;
    assert!(
        second.is_err(),
        "a second caller must wait on the in-flight open, not block on its file lock"
    );
    // The count alone cannot tell attaching from restarting, because a
    // second open would overwrite at the same key. The sequence can.
    assert_eq!(
        pool.pending_open_seq("federation-stuck").await,
        Some(first_seq),
        "the second caller must attach to the first open rather than start another"
    );

    Ok(())
}
