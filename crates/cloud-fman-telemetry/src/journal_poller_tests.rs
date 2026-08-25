//! Journal poller orchestration tests.

use std::{
    collections::VecDeque,
    num::{NonZeroU16, NonZeroUsize},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalResponse, ListSafeEventJournalsResponse, SafeEventCursor,
    SafeEventJournal, SafeEventJournalIncarnation, SafeEventJournalInfo, SeatId,
};

use super::*;
use crate::{
    auth::VerifiedHttpAuth,
    cipher::SecretCipher,
    iroh_journal_source::{JournalSession, JournalSource},
    journal_target::WorkTarget,
    journal_types::{JournalStreamId, ReceptionDay, ValidatedJournalBatch},
    store::{JournalStreamState, TargetMaterial},
};

struct FakeSource {
    incarnation: SafeEventJournalIncarnation,
    responses: Arc<tokio::sync::Mutex<VecDeque<FetchSafeEventJournalResponse>>>,
    connections: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
    stale_store: Option<Store>,
    connect_clock: Option<(Arc<TestClock>, i64)>,
    fetch_clock: Option<(Arc<TestClock>, i64)>,
}

struct FakeSession {
    incarnation: SafeEventJournalIncarnation,
    responses: Arc<tokio::sync::Mutex<VecDeque<FetchSafeEventJournalResponse>>>,
    fetches: Arc<AtomicUsize>,
    stale_store: Option<Store>,
    fetch_clock: Option<(Arc<TestClock>, i64)>,
}

struct TestClock(AtomicI64);

struct BudgetClock {
    wall: i64,
    expired: Arc<std::sync::atomic::AtomicBool>,
}

struct HotTwoSource {
    incarnation: SafeEventJournalIncarnation,
    fman_fetches: Arc<AtomicUsize>,
    seat_fetches: Arc<AtomicUsize>,
    budget_expired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

struct HotTwoSession {
    incarnation: SafeEventJournalIncarnation,
    fman_fetches: Arc<AtomicUsize>,
    seat_fetches: Arc<AtomicUsize>,
    budget_expired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

struct SlowSource {
    incarnation: SafeEventJournalIncarnation,
    fetches: Arc<AtomicUsize>,
    budget_expired: Arc<std::sync::atomic::AtomicBool>,
}

struct SlowSession {
    incarnation: SafeEventJournalIncarnation,
    fetches: Arc<AtomicUsize>,
    budget_expired: Arc<std::sync::atomic::AtomicBool>,
}

struct BlockingConnectSource {
    incarnation: SafeEventJournalIncarnation,
    connections: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

struct CountingSession {
    incarnation: SafeEventJournalIncarnation,
    lists: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
}

struct CapacityIsolationSource {
    incarnation: SafeEventJournalIncarnation,
    fetches: Arc<AtomicUsize>,
}

struct CapacityIsolationSession {
    incarnation: SafeEventJournalIncarnation,
    fman_id: String,
    fetches: Arc<AtomicUsize>,
    fetched: bool,
}

impl Clock for TestClock {
    fn now(&self) -> Result<i64, PollError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

impl Clock for BudgetClock {
    fn now(&self) -> Result<i64, PollError> {
        Ok(self.wall)
    }

    fn target_budget_expired(&self, _started: tokio::time::Instant) -> bool {
        self.expired.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JournalSource for HotTwoSource {
    async fn connect(&self, _target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        Ok(Box::new(HotTwoSession {
            incarnation: self.incarnation.clone(),
            fman_fetches: self.fman_fetches.clone(),
            seat_fetches: self.seat_fetches.clone(),
            budget_expired: self.budget_expired.clone(),
        }))
    }
}

#[async_trait]
impl JournalSession for HotTwoSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        Ok(ListSafeEventJournalsResponse {
            journals: vec![
                SafeEventJournalInfo {
                    journal: SafeEventJournal::Fman,
                    incarnation: self.incarnation.clone(),
                },
                SafeEventJournalInfo {
                    journal: SafeEventJournal::Seat {
                        seat_id: SeatId::new("22".repeat(32)).unwrap(),
                    },
                    incarnation: self.incarnation.clone(),
                },
            ],
        })
    }

    async fn fetch(
        &mut self,
        stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        let counter = match &stream.journal {
            SafeEventJournal::Fman => &self.fman_fetches,
            SafeEventJournal::Seat { .. } => &self.seat_fetches,
        };
        counter.fetch_add(1, Ordering::SeqCst);
        if let Some(expired) = &self.budget_expired {
            expired.store(true, Ordering::SeqCst);
        }
        let offset = stream.cursor.as_ref().map_or(1, |cursor| cursor.offset + 1);
        Ok(FetchSafeEventJournalResponse::Current {
            incarnation: self.incarnation.clone(),
            jsonl: b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
            next_cursor: Some(SafeEventCursor {
                incarnation: self.incarnation.clone(),
                segment: 1,
                offset,
            }),
            continuity_gap: false,
        })
    }
}

#[async_trait]
impl JournalSource for SlowSource {
    async fn connect(&self, _target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        Ok(Box::new(SlowSession {
            incarnation: self.incarnation.clone(),
            fetches: self.fetches.clone(),
            budget_expired: self.budget_expired.clone(),
        }))
    }
}

#[async_trait]
impl JournalSource for BlockingConnectSource {
    async fn connect(&self, _target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        if self.connections.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.wait().await;
            self.release.wait().await;
        }
        Ok(Box::new(CountingSession {
            incarnation: self.incarnation.clone(),
            lists: self.lists.clone(),
            fetches: self.fetches.clone(),
        }))
    }
}

#[async_trait]
impl JournalSession for CountingSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        Ok(ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: self.incarnation.clone(),
            }],
        })
    }

    async fn fetch(
        &mut self,
        _stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Err(PollError::Transient)
    }
}

#[async_trait]
impl JournalSource for CapacityIsolationSource {
    async fn connect(&self, target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        Ok(Box::new(CapacityIsolationSession {
            incarnation: self.incarnation.clone(),
            fman_id: target.fman_id().to_owned(),
            fetches: self.fetches.clone(),
            fetched: false,
        }))
    }
}

#[async_trait]
impl JournalSession for CapacityIsolationSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        Ok(ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: self.incarnation.clone(),
            }],
        })
    }

    async fn fetch(
        &mut self,
        stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if self.fman_id == "11" && !self.fetched {
            self.fetched = true;
            let offset = stream.cursor.as_ref().map_or(1, |cursor| cursor.offset + 1);
            return Ok(FetchSafeEventJournalResponse::Current {
                incarnation: self.incarnation.clone(),
                jsonl: format!("{{\"fields\":{{\"safe_to_share\":true}},\"offset\":{offset}}}\n")
                    .into_bytes(),
                next_cursor: Some(SafeEventCursor {
                    incarnation: self.incarnation.clone(),
                    segment: 1,
                    offset,
                }),
                continuity_gap: false,
            });
        }
        Ok(FetchSafeEventJournalResponse::Current {
            incarnation: self.incarnation.clone(),
            jsonl: Vec::new(),
            next_cursor: stream.cursor.clone(),
            continuity_gap: false,
        })
    }
}

#[async_trait]
impl JournalSession for SlowSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        Ok(ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: self.incarnation.clone(),
            }],
        })
    }

    async fn fetch(
        &mut self,
        _stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        self.budget_expired.store(true, Ordering::SeqCst);
        Ok(FetchSafeEventJournalResponse::Current {
            incarnation: self.incarnation.clone(),
            jsonl: b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
            next_cursor: Some(SafeEventCursor {
                incarnation: self.incarnation.clone(),
                segment: 1,
                offset: 1,
            }),
            continuity_gap: false,
        })
    }
}

#[async_trait]
impl JournalSource for FakeSource {
    async fn connect(&self, _target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        self.connections.fetch_add(1, Ordering::SeqCst);
        if let Some((clock, value)) = &self.connect_clock {
            clock.0.store(*value, Ordering::SeqCst);
        }
        Ok(Box::new(FakeSession {
            incarnation: self.incarnation.clone(),
            responses: self.responses.clone(),
            fetches: self.fetches.clone(),
            stale_store: self.stale_store.clone(),
            fetch_clock: self.fetch_clock.clone(),
        }))
    }
}

#[async_trait]
impl JournalSession for FakeSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        Ok(ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: self.incarnation.clone(),
            }],
        })
    }

    async fn fetch(
        &mut self,
        _stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if let Some(store) = self.stale_store.take() {
            store.quarantine("11").await.unwrap();
        }
        if let Some((clock, value)) = &self.fetch_clock {
            clock.0.store(*value, Ordering::SeqCst);
        }
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or(PollError::Transient)
    }
}

async fn store(directory: &tempfile::TempDir) -> Store {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(
        &directory.path().join("state.sqlite"),
        "development",
        SecretCipher::new(&[7; 32]),
        "test".into(),
        200_000,
    )
    .await
    .unwrap();
    let now = now().unwrap();
    let auth = VerifiedHttpAuth {
        signer: "11".repeat(32),
        event_id: "poll".into(),
        created_at: now,
    };
    store.reserve_auth(&auth, now).await.unwrap();
    store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: "11",
                fman_name: "calm-tern",
                endpoint_id: "unused-by-fake",
                capability: &[9; 32],
                generation: 1,
            },
            now,
        )
        .await
        .unwrap();
    store
}

fn incarnation() -> SafeEventJournalIncarnation {
    "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap()
}

fn responses(count: usize) -> VecDeque<FetchSafeEventJournalResponse> {
    let incarnation = incarnation();
    (1..=count)
        .map(|offset| FetchSafeEventJournalResponse::Current {
            incarnation: incarnation.clone(),
            jsonl: format!("{{\"fields\":{{\"safe_to_share\":true}},\"offset\":{offset}}}\n")
                .into_bytes(),
            next_cursor: Some(SafeEventCursor {
                incarnation: incarnation.clone(),
                segment: 1,
                offset: offset as u64,
            }),
            continuity_gap: false,
        })
        .chain(std::iter::once(FetchSafeEventJournalResponse::Current {
            incarnation: incarnation.clone(),
            jsonl: Vec::new(),
            next_cursor: Some(SafeEventCursor {
                incarnation: incarnation.clone(),
                segment: 1,
                offset: count as u64,
            }),
            continuity_gap: false,
        }))
        .collect()
}

fn poller(
    store: Store,
    archive: JournalArchive,
    responses: VecDeque<FetchSafeEventJournalResponse>,
    stale_store: Option<Store>,
) -> (JournalPoller, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let connections = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let poller = JournalPoller::with_source(
        store,
        archive,
        Arc::new(FakeSource {
            incarnation: incarnation(),
            responses: Arc::new(tokio::sync::Mutex::new(responses)),
            connections: connections.clone(),
            fetches: fetches.clone(),
            stale_store,
            connect_clock: None,
            fetch_clock: None,
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
    );
    (poller, connections, fetches)
}

fn clocked_poller(
    store: Store,
    archive: JournalArchive,
    responses: VecDeque<FetchSafeEventJournalResponse>,
    clock: Arc<TestClock>,
    connect_time: Option<i64>,
    fetch_time: Option<i64>,
) -> (JournalPoller, Arc<AtomicUsize>) {
    let fetches = Arc::new(AtomicUsize::new(0));
    let source = FakeSource {
        incarnation: incarnation(),
        responses: Arc::new(tokio::sync::Mutex::new(responses)),
        connections: Arc::new(AtomicUsize::new(0)),
        fetches: fetches.clone(),
        stale_store: None,
        connect_clock: connect_time.map(|time| (clock.clone(), time)),
        fetch_clock: fetch_time.map(|time| (clock.clone(), time)),
    };
    (
        JournalPoller::with_source_and_clock(
            store,
            archive,
            Arc::new(source),
            NonZeroUsize::new(1).unwrap(),
            NonZeroU16::new(30).unwrap(),
            clock,
        ),
        fetches,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn drains_multiple_batches_with_one_iroh_session_and_resumes_promptly() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, connections, fetches) = poller(store.clone(), archive, responses(10), None);
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);

    assert!(!poller.poll_once(receiver).await.unwrap());
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 11);
    assert_eq!(store.final_frame_boundaries().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_max_cursor_is_archived_and_committed() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let source_incarnation = incarnation();
    let cursor = SafeEventCursor {
        incarnation: source_incarnation.clone(),
        segment: i64::MAX as u64,
        offset: i64::MAX as u64,
    };
    let responses = VecDeque::from([
        FetchSafeEventJournalResponse::Current {
            incarnation: source_incarnation.clone(),
            jsonl: b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
            next_cursor: Some(cursor.clone()),
            continuity_gap: false,
        },
        FetchSafeEventJournalResponse::Current {
            incarnation: source_incarnation.clone(),
            jsonl: Vec::new(),
            next_cursor: Some(cursor.clone()),
            continuity_gap: false,
        },
    ]);
    let (poller, _, _) = poller(store.clone(), archive, responses, None);
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);

    assert!(!poller.poll_once(receiver).await.unwrap());
    assert_eq!(store.final_frame_boundaries().await.unwrap().len(), 1);
    let target = store
        .active_collection_targets(now().unwrap())
        .await
        .unwrap()
        .pop()
        .unwrap();
    let work = store
        .begin_collection_work(&target, now().unwrap())
        .await
        .unwrap()
        .unwrap();
    let state = store
        .open_journal_stream(
            &work,
            &SafeEventJournal::Fman,
            &source_incarnation,
            now().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.cursor, Some(cursor));
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_overflow_is_contained_before_archive_or_cursor_commit() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let source_incarnation = incarnation();
    let responses = VecDeque::from([FetchSafeEventJournalResponse::Current {
        incarnation: source_incarnation.clone(),
        jsonl: b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
        next_cursor: Some(SafeEventCursor {
            incarnation: source_incarnation.clone(),
            segment: i64::MAX as u64 + 1,
            offset: 1,
        }),
        continuity_gap: false,
    }]);
    let (poller, _, _) = poller(store.clone(), archive, responses, None);
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);

    // A hostile target is a typed transient target failure, not a daemon failure.
    assert!(!poller.poll_once(receiver).await.unwrap());
    assert!(store.final_frame_boundaries().await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(directory.path().join("logs"))
            .unwrap()
            .count(),
        0
    );
    let target = store
        .active_collection_targets(now().unwrap())
        .await
        .unwrap()
        .pop()
        .unwrap();
    let work = store
        .begin_collection_work(&target, now().unwrap())
        .await
        .unwrap()
        .unwrap();
    let state = store
        .open_journal_stream(
            &work,
            &SafeEventJournal::Fman,
            &source_incarnation,
            now().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(state.cursor.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn same_day_archive_saturation_preserves_cursor_and_other_target_work() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let source_incarnation = incarnation();
    let first_jsonl = b"{\"fields\":{\"safe_to_share\":true},\"offset\":1}\n";
    let quota = zstd::stream::encode_all(&first_jsonl[..], 3).unwrap().len() as u64;
    let archive = JournalArchive::open(directory.path(), quota).unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let poller = JournalPoller::with_source(
        store.clone(),
        archive,
        Arc::new(CapacityIsolationSource {
            incarnation: source_incarnation.clone(),
            fetches: fetches.clone(),
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);

    assert!(!poller.poll_once(receiver.clone()).await.unwrap());
    assert_eq!(store.final_frame_boundaries().await.unwrap().len(), 1);

    let timestamp = now().unwrap();
    let auth = VerifiedHttpAuth {
        signer: "22".repeat(32),
        event_id: "second-target".into(),
        created_at: timestamp,
    };
    store.reserve_auth(&auth, timestamp).await.unwrap();
    store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: "22",
                fman_name: "second",
                endpoint_id: "unused-by-fake",
                capability: &[8; 32],
                generation: 1,
            },
            timestamp,
        )
        .await
        .unwrap();

    assert!(!poller.poll_once(receiver).await.unwrap());
    assert_eq!(fetches.load(Ordering::SeqCst), 4);
    assert_eq!(store.final_frame_boundaries().await.unwrap().len(), 1);

    let mut states = Vec::new();
    for target in store.active_collection_targets(timestamp).await.unwrap() {
        let work = store
            .begin_collection_work(&target, timestamp)
            .await
            .unwrap()
            .unwrap();
        let fman_id = work.fman_id().to_owned();
        let state = store
            .open_journal_stream(
                &work,
                &SafeEventJournal::Fman,
                &source_incarnation,
                timestamp,
            )
            .await
            .unwrap()
            .unwrap();
        states.push((fman_id, state.cursor.map(|cursor| cursor.offset)));
    }
    states.sort();
    assert_eq!(
        states,
        [("11".to_owned(), Some(1)), ("22".to_owned(), None)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_registration_after_fetch_rolls_back_frame_and_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, _, _) = poller(store.clone(), archive, responses(1), Some(store.clone()));
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);

    poller.poll_once(receiver).await.unwrap();
    assert!(store.final_frame_boundaries().await.unwrap().is_empty());
    let mut bytes = 0;
    for stream in std::fs::read_dir(directory.path().join("logs")).unwrap() {
        for file in std::fs::read_dir(stream.unwrap().path()).unwrap() {
            bytes += file.unwrap().metadata().unwrap().len();
        }
    }
    assert_eq!(bytes, 0);
    store.reactivate("11").await.unwrap();
    let snapshot = store
        .active_collection_targets(now().unwrap())
        .await
        .unwrap()
        .pop()
        .unwrap();
    let work = store
        .begin_collection_work(&snapshot, now().unwrap())
        .await
        .unwrap()
        .unwrap();
    let state = store
        .open_journal_stream(
            &work,
            &SafeEventJournal::Fman,
            &incarnation(),
            now().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(state.cursor.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_prevents_fetching_a_queued_target() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, connections, fetches) = poller(store, archive, responses(1), None);
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    shutdown.send_replace(true);

    poller.poll_once(receiver).await.unwrap();
    assert_eq!(connections.load(Ordering::SeqCst), 0);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_does_not_connect_or_list_a_target_waiting_for_a_permit() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let now = now().unwrap();
    let auth = VerifiedHttpAuth {
        signer: "22".repeat(32),
        event_id: "second-poll-target".into(),
        created_at: now,
    };
    store.reserve_auth(&auth, now).await.unwrap();
    store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: "22",
                fman_name: "mild-wren",
                endpoint_id: "unused-by-fake",
                capability: &[8; 32],
                generation: 1,
            },
            now,
        )
        .await
        .unwrap();

    let connections = Arc::new(AtomicUsize::new(0));
    let lists = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let poller = JournalPoller::with_source(
        store,
        archive,
        Arc::new(BlockingConnectSource {
            incarnation: incarnation(),
            connections: connections.clone(),
            lists: lists.clone(),
            fetches: fetches.clone(),
            entered: entered.clone(),
            release: release.clone(),
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
    );
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move { poller.poll_once(receiver).await });

    entered.wait().await;
    shutdown.send_replace(true);
    release.wait().await;
    task.await.unwrap().unwrap();

    assert_eq!(connections.load(Ordering::SeqCst), 1);
    assert_eq!(lists.load(Ordering::SeqCst), 0);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_sibling_fences_queued_target_before_permit_release() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let timestamp = now().unwrap();
    let auth = VerifiedHttpAuth {
        signer: "22".repeat(32),
        event_id: "fatal-sibling-second-target".into(),
        created_at: timestamp,
    };
    store.reserve_auth(&auth, timestamp).await.unwrap();
    store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: "22",
                fman_name: "mild-wren",
                endpoint_id: "unused-by-fake",
                capability: &[8; 32],
                generation: 1,
            },
            timestamp,
        )
        .await
        .unwrap();

    let mut queued_target_id = None;
    for target in store.active_collection_targets(timestamp).await.unwrap() {
        let work = store
            .begin_collection_work(&target, timestamp)
            .await
            .unwrap()
            .unwrap();
        if work.fman_id() == "22" {
            queued_target_id = Some(target.target_id);
            break;
        }
    }
    let queued_target_id = queued_target_id.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let fatal_target_connections = Arc::new(AtomicUsize::new(0));
    let fatal_target_entered = Arc::new(tokio::sync::Barrier::new(2));
    let fatal_target_release = Arc::new(tokio::sync::Barrier::new(2));
    let hook = Arc::new(FatalAdmissionHook {
        allow_queued: tokio::sync::Notify::new(),
        queued: tokio::sync::Notify::new(),
        published: tokio::sync::Barrier::new(2),
        release: tokio::sync::Barrier::new(2),
        queued_target_id,
    });
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let poller = JournalPoller {
        fatal_admission_hook: Some(hook.clone()),
        ..JournalPoller::with_source(
            store,
            archive,
            Arc::new(FatalSiblingSource {
                connections: connections.clone(),
                fatal_target_connections: fatal_target_connections.clone(),
                fatal_target_entered: fatal_target_entered.clone(),
                fatal_target_release: fatal_target_release.clone(),
            }),
            NonZeroUsize::new(1).unwrap(),
            NonZeroU16::new(30).unwrap(),
        )
    };
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move { poller.poll_once(receiver).await });

    fatal_target_entered.wait().await;
    hook.allow_queued.notify_one();
    hook.queued.notified().await;
    fatal_target_release.wait().await;
    hook.published.wait().await;
    assert!(
        !task.is_finished(),
        "the coordinator must not observe the fatal result before permit release"
    );
    hook.release.wait().await;

    assert!(matches!(task.await.unwrap(), Err(PollError::Fatal(_))));
    assert_eq!(fatal_target_connections.load(Ordering::SeqCst), 1);
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "the queued target must not connect after fatal admission closes"
    );
}

struct FatalSiblingSource {
    connections: Arc<AtomicUsize>,
    fatal_target_connections: Arc<AtomicUsize>,
    fatal_target_entered: Arc<tokio::sync::Barrier>,
    fatal_target_release: Arc<tokio::sync::Barrier>,
}

#[async_trait]
impl JournalSource for FatalSiblingSource {
    async fn connect(&self, target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        self.connections.fetch_add(1, Ordering::SeqCst);
        if target.fman_id() == "11" {
            self.fatal_target_connections.fetch_add(1, Ordering::SeqCst);
            self.fatal_target_entered.wait().await;
            self.fatal_target_release.wait().await;
            Err(PollError::Fatal("fatal first target"))
        } else {
            Err(PollError::Fatal(
                "queued target connected after fatal sibling",
            ))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_does_not_detach_an_in_flight_archive_append() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let hook = Arc::new(crate::archive::TestAppendHook {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
        fail_after_write: std::sync::atomic::AtomicBool::new(false),
    });
    let archive = JournalArchive::open(directory.path(), 1024 * 1024)
        .unwrap()
        .with_append_hook(hook.clone());
    let (poller, _, _) = poller(store, archive, responses(1), None);
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move { poller.poll_once(receiver).await });

    hook.entered.wait();
    shutdown.send_replace(true);
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    hook.release.wait();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_expiry_after_listing_prevents_queued_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let start = now().unwrap();
    let clock = Arc::new(TestClock(AtomicI64::new(start)));
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, fetches) = clocked_poller(
        store,
        archive,
        responses(1),
        clock,
        Some(start + 200_001),
        None,
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    poller.poll_once(receiver).await.unwrap();
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn reception_day_is_sampled_after_a_midnight_crossing_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let start = now().unwrap();
    let next_midnight = (start.div_euclid(86_400) + 1) * 86_400;
    let clock = Arc::new(TestClock(AtomicI64::new(start)));
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, _) = clocked_poller(
        store.clone(),
        archive,
        responses(1),
        clock,
        None,
        Some(next_midnight),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    poller.poll_once(receiver).await.unwrap();
    let boundaries = store.final_frame_boundaries().await.unwrap();
    assert_eq!(
        boundaries[0].day,
        ReceptionDay::from_unix_seconds(next_midnight).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hot_first_stream_cannot_starve_a_later_backlogged_stream() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let fman_fetches = Arc::new(AtomicUsize::new(0));
    let seat_fetches = Arc::new(AtomicUsize::new(0));
    let poller = JournalPoller::with_source(
        store,
        archive,
        Arc::new(HotTwoSource {
            incarnation: incarnation(),
            fman_fetches: fman_fetches.clone(),
            seat_fetches: seat_fetches.clone(),
            budget_expired: None,
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    assert!(poller.poll_once(receiver).await.unwrap());
    assert_eq!(fman_fetches.load(Ordering::SeqCst), 20);
    assert_eq!(seat_fetches.load(Ordering::SeqCst), 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn capacity_sibling_waits_for_sqlite_commit_before_contained_return() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let timestamp = now().unwrap();
    let auth = VerifiedHttpAuth {
        signer: "22".repeat(32),
        event_id: "second-target".into(),
        created_at: timestamp,
    };
    store.reserve_auth(&auth, timestamp).await.unwrap();
    store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: "22",
                fman_name: "second",
                endpoint_id: "unused-by-fake",
                capability: &[8; 32],
                generation: 1,
            },
            timestamp,
        )
        .await
        .unwrap();
    let jsonl = b"{\"fields\":{\"safe_to_share\":true},\"offset\":1}\n";
    let quota = zstd::stream::encode_all(&jsonl[..], 3).unwrap().len() as u64;
    let archive = JournalArchive::open(directory.path(), quota).unwrap();
    let hook = Arc::new(crate::store::TestCommitHook {
        entered_once: std::sync::atomic::AtomicBool::new(false),
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    let guarded_store = store.with_commit_hook(hook.clone());
    let (poller, _, _) = poller(guarded_store, archive, responses(2), None);
    let poller = JournalPoller {
        concurrency: NonZeroUsize::new(2).unwrap(),
        ..poller
    };
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move { poller.poll_once(receiver).await });

    hook.entered.wait();
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    hook.release.wait();
    assert!(!task.await.unwrap().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn elapsed_target_budget_releases_permit_after_one_slow_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let budget_expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = JournalPoller::with_source_and_clock(
        store,
        archive,
        Arc::new(SlowSource {
            incarnation: incarnation(),
            fetches: fetches.clone(),
            budget_expired: budget_expired.clone(),
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
        Arc::new(BudgetClock {
            wall: now().unwrap(),
            expired: budget_expired,
        }),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    assert!(poller.poll_once(receiver).await.unwrap());
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn elapsed_retry_rotates_to_the_next_unfetched_stream() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory).await;
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let fman_fetches = Arc::new(AtomicUsize::new(0));
    let seat_fetches = Arc::new(AtomicUsize::new(0));
    let budget_expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = JournalPoller::with_source_and_clock(
        store,
        archive,
        Arc::new(HotTwoSource {
            incarnation: incarnation(),
            fman_fetches: fman_fetches.clone(),
            seat_fetches: seat_fetches.clone(),
            budget_expired: Some(budget_expired.clone()),
        }),
        NonZeroUsize::new(1).unwrap(),
        NonZeroU16::new(30).unwrap(),
        Arc::new(BudgetClock {
            wall: now().unwrap(),
            expired: budget_expired.clone(),
        }),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    assert!(poller.poll_once(receiver.clone()).await.unwrap());
    assert_eq!(fman_fetches.load(Ordering::SeqCst), 1);
    assert_eq!(seat_fetches.load(Ordering::SeqCst), 0);

    budget_expired.store(false, Ordering::SeqCst);
    assert!(poller.poll_once(receiver).await.unwrap());
    assert_eq!(fman_fetches.load(Ordering::SeqCst), 1);
    assert_eq!(seat_fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn retention_prunes_once_per_cutoff_day_and_retries_on_day_advance() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(
        &directory.path().join("state.sqlite"),
        "development",
        SecretCipher::new(&[7; 32]),
        "test".into(),
        3600,
    )
    .await
    .unwrap();
    let timestamp = now().unwrap();
    let clock = Arc::new(TestClock(AtomicI64::new(timestamp)));
    let archive = JournalArchive::open(directory.path(), 1024 * 1024).unwrap();
    let (poller, _) = clocked_poller(
        store,
        archive.clone(),
        VecDeque::new(),
        clock.clone(),
        None,
        None,
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    poller.poll_once(receiver.clone()).await.unwrap();

    let incarnation = incarnation();
    let stream_id = JournalStreamId::parse("z".repeat(32)).unwrap();
    let old_day = ReceptionDay::parse("2000-01-01".into()).unwrap();
    let old_batch = ValidatedJournalBatch::new(
        &incarnation,
        None,
        incarnation.clone(),
        b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec(),
        Some(SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: 1,
            offset: 1,
        }),
        false,
    )
    .unwrap();
    archive.append(&stream_id, &old_day, &old_batch).unwrap();
    let old = directory
        .path()
        .join("logs")
        .join(stream_id.as_str())
        .join("2000-01-01.jsonl.zst");
    poller.poll_once(receiver.clone()).await.unwrap();
    assert!(old.exists(), "same-day backlog retry skipped the tree scan");

    clock.0.fetch_add(86_400, Ordering::SeqCst);
    poller.poll_once(receiver.clone()).await.unwrap();
    assert!(!old.exists(), "next UTC cutoff pruned the old orphan");

    archive.append(&stream_id, &old_day, &old_batch).unwrap();
    clock.0.fetch_sub(86_400, Ordering::SeqCst);
    poller.poll_once(receiver.clone()).await.unwrap();
    assert!(
        old.exists(),
        "backward clock movement must not repeat an older cutoff scan"
    );
    clock.0.fetch_add(86_400, Ordering::SeqCst);
    poller.poll_once(receiver).await.unwrap();
    assert!(
        old.exists(),
        "returning to the cutoff high-water must not repeat its scan"
    );
}
