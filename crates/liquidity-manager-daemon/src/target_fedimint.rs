//! The bounded pool of target-federation Fedimint clients.
//!
//! Requester-supplied federation ids decide which federations reach this pool,
//! so a boot-configured ceiling closes idle clients rather than letting that
//! input size a set of RocksDB handles and background tasks. Pending opens have
//! their own budget. Client databases are retained past eviction, because they
//! are what operator recovery reads.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use fedimint_bip39::Bip39RootSecretStrategy;
use fedimint_bitcoind::create_esplora_rpc;
use fedimint_client::secret::RootSecretStrategy;
use fedimint_client::{Client, ClientHandleArc, RootSecret};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::db::Database as FedimintDatabase;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::util::SafeUrl;
use fedimint_ln_client::LightningClientInit;
use fedimint_meta_client::MetaClientInit;
use fedimint_mint_client::MintClientInit;
use fedimint_wallet_client::WalletClientInit;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::endpoint_policy::{self, EndpointPolicy};

/// Target federation clients held open at once when the operator configures no
/// ceiling.
///
/// Each open client costs a RocksDB handle, an on-disk file lock, and several
/// background tasks, so this is a resource pool rather than a memoisation
/// table. Eight is well above the number of targets a provider funds
/// concurrently and well below any host limit, which is the shape a default
/// wants: operators who never think about it are unaffected, and the number
/// exists so that FI-supplied federation ids cannot decide it.
pub(crate) const DEFAULT_MAX_OPEN_TARGET_CLIENTS: NonZeroUsize = NonZeroUsize::new(8).unwrap();

/// How long one open may spend downloading a target federation's config before
/// it gives up.
///
/// **This does not give the open a terminal condition.** The unbounded wait is
/// not here: `preview` is `download_from_invite_code`, which
/// already retries under `aggressive_backoff` — 14 attempts, 5s cap — and
/// terminates. The wait that does not terminate is inside `join`, where
/// `load_and_refresh_common_api_version_static` loops on a 30-second timeout and
/// `continue`s unconditionally while `block_until_ok`. A target that serves its
/// config and then stops answering `api_version` holds its slot for the life of
/// the process. The already-initialized `open()` branch reaches the same loop.
///
/// **So `opens` can still grow past the ceiling.** `make_room` counts pending
/// opens against it but can never evict one — eviction selects victims among
/// installed clients — so on finding no idle victim it opens anyway rather than
/// closing a client a worker is using.
///
/// Bounding the loop that actually hangs is not available at this layer. It runs
/// after `TaskGroup::new()` inside the build, so a timeout there drops a future
/// owning the group and detaches a task still holding the RocksDB file lock.
/// Closing the gap needs an upstream bound on that negotiation, or a pool that
/// refuses to exceed its ceiling instead of opening anyway — a behaviour change,
/// since refusing fails an item that today proceeds.
///
/// What this bound is still worth: abandoning a network fetch is safe, and at
/// the preview nothing has been consumed — `db` is still owned by
/// `create_or_load_client` and drops on the error path, releasing its file lock,
/// and no `TaskGroup` exists yet. It caps one call that has no cap of its own,
/// and it fails closed. It is not the repair the pool needs.
const TARGET_PREVIEW_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct TargetFedimintClients {
    inner: Arc<Mutex<TargetFedimintClientsInner>>,
    max_open: NonZeroUsize,

    /// Address policy applied to a target invite before this pool dials it.
    ///
    /// The verification pipeline's preview provider applies the same policy to
    /// the same invite, but that is a different dial: acceptance previews the
    /// federation, and this joins it, on a path the pipeline never touches.
    /// Leaving it unguarded would make the join a second, unfiltered route to
    /// `download_from_invite_code`, reachable with any endpoint a requester
    /// puts in an invite.
    endpoint_policy: EndpointPolicy,
}

/// Test-only, and gated so that stays true. The permissive policy is right for
/// a harness on loopback and wrong for a deployment, and nothing but this `cfg`
/// would stop a future production `Default::default()` selecting it.
#[cfg(test)]
impl Default for TargetFedimintClients {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_OPEN_TARGET_CLIENTS,
            EndpointPolicy::AllowPrivate,
        )
    }
}

#[derive(Default)]
struct TargetFedimintClientsInner {
    clients: BTreeMap<String, ClientHandleArc>,

    /// Federation ids of open clients in least-recently-used order, most
    /// recently used last.
    ///
    /// Kept beside `clients` rather than folded into it because the map is
    /// ordered by federation id, and eviction needs the opposite order: which
    /// client nothing has wanted for longest.
    usage: Vec<String>,

    locks: BTreeMap<String, Arc<Mutex<()>>>,

    /// Opens in flight, keyed by federation id.
    ///
    /// An open is owned by the pool, not by whichever caller asked first. A
    /// caller that goes away — the stability worker's per-item budget drops the
    /// item's future at whatever await it is suspended on — must not take the
    /// open with it, because `ClientBuilder::build_stopped` creates its own
    /// `TaskGroup`, spawns a config-refresh task holding a clone of the client
    /// database into it, and that group has no `Drop`. Dropping the build
    /// therefore detaches a task that holds the RocksDB file lock for the life
    /// of the process, while the pool holds no handle to it, and the next
    /// caller's `open_rocksdb` blocks forever on `flock` inside a
    /// `block_in_place` section no timeout can interrupt.
    opens: BTreeMap<String, PendingOpen>,

    /// Source of `PendingOpen::seq`.
    next_open_seq: u64,
}

/// The most opens that may be in flight at once, independent of the configured
/// client ceiling. See [`TargetFedimintClientsInner::may_start_open`].
const MAX_PENDING_OPENS: usize = 4;

/// How long a pending open may run before it is reported as stuck.
///
/// Not a timeout. Nothing cancels the open when this expires, because nothing
/// can: `ClientBuilder` builds its own `TaskGroup`, spawns a task holding a
/// clone of the client database into it before the wait that hangs, and that
/// group has no `Drop`. Dropping the build detaches that task and leaks the
/// RocksDB file lock for the life of the process, which is a worse failure than
/// the one being repaired.
///
/// So this is a reporting threshold, and reporting is the whole remedy FLIP has
/// for a target that goes quiet. Set well above any healthy open, which finishes
/// in seconds.
const STUCK_OPEN_REPORT_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

/// What [`TargetFedimintClients::open_slot`] found under one guard.
enum OpenSlot {
    /// A client was already installed; no open is needed.
    Cached(ClientHandleArc),

    /// Too many opens are already in flight for another to start.
    ///
    /// The caller retries on its next pass rather than failing the item: an
    /// unreachable target already behaves this way, and refusing outright would
    /// fail work that today proceeds.
    AtCapacity,

    /// An open is in flight — this caller's, or one it attached to.
    Pending {
        done: tokio::sync::watch::Receiver<Option<Arc<Result<ClientHandleArc, String>>>>,
        seq: u64,
    },
}

/// A client open in flight.
///
/// Waiters clone the receiver and await it. Because the open runs on a task the
/// pool spawned, a waiter that is dropped cancels only its own wait.
struct PendingOpen {
    done: tokio::sync::watch::Receiver<Option<Arc<Result<ClientHandleArc, String>>>>,

    /// Identifies this open across the awaits its task performs.
    ///
    /// The task releases its own mutex guard while building, so by the time it
    /// finishes the slot may have been dropped by `evict` or `shutdown_all` and
    /// a newer open started. Comparing the sequence is how the task tells
    /// "still wanted" from "superseded", and how a failed waiter clears its own
    /// slot without clearing a successor's.
    seq: u64,

    /// When this open started, for the stuck-open report.
    ///
    /// A pending open has no terminal condition — see [`MAX_PENDING_OPENS`] —
    /// so its age is the only thing that distinguishes a slow target from one
    /// that will never answer, and the operator has nothing else to go on.
    started_at: std::time::Instant,

    /// Whether this open has already been reported as stuck.
    ///
    /// One line per stuck federation, not one per open attempt: the condition
    /// lasts until the process restarts, so a message repeated on every later
    /// open would bury everything else in the log for as long as the fault
    /// lasts.
    stuck_reported: bool,
}

impl TargetFedimintClientsInner {
    /// Whether one more client can open without exceeding `max_open`.
    ///
    /// **Installed clients only.** Counting pending opens here reads as the
    /// stricter arithmetic while being the weaker one:
    /// [`Self::least_recently_used_idle`] can select only an installed client,
    /// so a pending open would hold a ceiling slot it could never yield, and
    /// once opens filled the ceiling `make_room` would find no victim and open
    /// anyway. Opens have their own budget — see [`Self::may_start_open`] and
    /// [`MAX_PENDING_OPENS`] — so a target that never answers costs a pending
    /// slot instead of crowding a healthy federation out of the ceiling.
    ///
    /// This is a necessary condition and not a sufficient one: `make_room`
    /// opens anyway when it can find no idle victim, rather than closing a
    /// client a worker is holding. What keeps *that* overshoot finite is how
    /// many handles are held at once, and no requester sets that number: the
    /// stability worker awaits each pass inline so passes cannot overlap, it
    /// iterates its items serially, and each backend call clones the handle for
    /// the length of one call. The remaining holders are operator-driven.
    ///
    /// It is **not** [`TARGET_PREVIEW_BUDGET`]. That bounds the preview and not
    /// the `api_version` negotiation the open can hang in, so a stuck open has
    /// no terminal condition at this layer; that is why opens are bounded by a
    /// budget rather than by a timeout.
    fn has_room(&self, max_open: NonZeroUsize) -> bool {
        self.clients.len() < max_open.get()
    }

    /// Whether another open may start.
    ///
    /// Pending opens have their own small budget rather than sharing the client
    /// ceiling. Sharing it would leave the set unbounded: because
    /// `least_recently_used_idle` can evict only *installed* clients, a pending
    /// open occupies a slot it can never yield, so an FI driving federations
    /// whose config download never completes crowds healthy, already-open
    /// federations out of the ceiling and `make_room` opens anyway.
    ///
    /// Separating the two budgets is what bounds it. A stuck open costs a
    /// pending slot, not a client slot, so work on federations that are already
    /// open keeps running.
    ///
    /// The bound is small on purpose: opens are transient, and more than a
    /// handful in flight at once means targets are not answering — which is
    /// exactly when starting more is wrong.
    fn may_start_open(&self) -> bool {
        self.opens.len() < MAX_PENDING_OPENS
    }

    /// Every pending open with its age, oldest first.
    ///
    /// The oldest is the one an operator wants named: a refusal that says only
    /// "at capacity" tells them the budget is full and not which target filled
    /// it, and the whole remedy available to them is knowing which federation
    /// to stop endorsing before they restart.
    fn pending_open_ages(&self, now: std::time::Instant) -> Vec<(String, std::time::Duration)> {
        let mut ages: Vec<_> = self
            .opens
            .iter()
            .map(|(federation_id, open)| {
                (
                    federation_id.clone(),
                    now.saturating_duration_since(open.started_at),
                )
            })
            .collect();
        ages.sort_by_key(|(_, age)| std::cmp::Reverse(*age));
        ages
    }

    /// Marks and returns pending opens that have passed
    /// [`STUCK_OPEN_REPORT_AFTER`] and have not been reported yet.
    ///
    /// Marking as it reports is what keeps this to one line per stuck
    /// federation. The condition does not clear without a restart, so a report
    /// on every later open attempt would be the same message forever.
    fn take_newly_stuck_opens(
        &mut self,
        now: std::time::Instant,
    ) -> Vec<(String, std::time::Duration)> {
        let mut stuck = Vec::new();
        for (federation_id, open) in &mut self.opens {
            let age = now.saturating_duration_since(open.started_at);
            if age >= STUCK_OPEN_REPORT_AFTER && !open.stuck_reported {
                open.stuck_reported = true;
                stuck.push((federation_id.clone(), age));
            }
        }
        stuck
    }

    /// Records that `federation_id` was just used, moving it to the most-recent
    /// end.
    fn touch(&mut self, federation_id: &str) {
        self.usage.retain(|id| id != federation_id);
        self.usage.push(federation_id.to_owned());
    }

    /// Drops a federation from both the client map and the usage order,
    /// returning the handle if one was open.
    fn take(&mut self, federation_id: &str) -> Option<ClientHandleArc> {
        self.usage.retain(|id| id != federation_id);
        self.clients.remove(federation_id)
    }

    /// The longest-unused open client that nothing else currently holds.
    fn least_recently_used_idle(&self) -> Option<String> {
        least_recently_used_idle(&self.usage, |federation_id| {
            self.clients
                .get(federation_id)
                .is_some_and(|client| Arc::strong_count(client) == 1)
        })
    }

    /// Forgets per-federation lock entries nothing holds.
    ///
    /// The lock map is the other half of this cache that FI input can grow: it
    /// gained an entry per distinct federation id and never lost one, so a
    /// stream of unique ids grew it without bound even when every client had
    /// been closed. An entry with a single reference is held only by this map,
    /// so no open or eviction is relying on it to serialize anything, and the
    /// next caller for that federation simply creates a fresh one.
    fn prune_idle_locks(&mut self) {
        self.locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
}

impl TargetFedimintClients {
    pub(crate) fn new(max_open: NonZeroUsize, endpoint_policy: EndpointPolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TargetFedimintClientsInner::default())),
            max_open,
            endpoint_policy,
        }
    }

    /// Opens (or returns the cached) client for one target federation.
    ///
    /// `esplora_url` is the chain backend the wallet client should watch, taken
    /// from this daemon's own chain-observer configuration. A cached client
    /// keeps the backend it was opened with, so a chain-observer change reaches
    /// existing targets through `reopen_federation_client` like every other
    /// client-affecting config change.
    pub(crate) async fn create_or_load(
        &self,
        federations_dir: &Path,
        federation_id: &str,
        invite_code: &str,
        esplora_url: Option<&SafeUrl>,
    ) -> Result<ClientHandleArc, TargetFedimintError> {
        {
            let mut inner = self.inner.lock().await;
            if let Some(client) = inner.clients.get(federation_id).cloned() {
                inner.touch(federation_id);
                return Ok(client);
            }
        }

        // Make room before taking this federation's lock, never while holding
        // it: `make_room` takes the lock of whichever federation it closes, and
        // holding two of these at once is how two concurrent opens would pick
        // each other as victims and deadlock.
        self.make_room().await;

        let federation_lock = {
            let mut inner = self.inner.lock().await;
            if let Some(client) = inner.clients.get(federation_id).cloned() {
                inner.touch(federation_id);
                return Ok(client);
            }
            inner
                .locks
                .entry(federation_id.to_owned())
                .or_default()
                .clone()
        };

        let _guard = federation_lock.lock().await;
        {
            let mut inner = self.inner.lock().await;
            if let Some(client) = inner.clients.get(federation_id).cloned() {
                inner.touch(federation_id);
                return Ok(client);
            }
        }

        // Before the dial and before the slot, so a refused invite opens no
        // RocksDB and takes no file lock. It is not free: the cache reads,
        // `make_room`, and this federation's lock entry all precede it. The preview provider checks the
        // same invite at acceptance, but this is a separate dial on a path that
        // does not go through it, and the FI can repoint a name between the two.
        let approved_invite =
            endpoint_policy::check_invite_endpoints(self.endpoint_policy, invite_code)
                .await
                .map_err(|error| TargetFedimintError::EndpointPolicy(error.to_string()))?;

        let invite_code = InviteCode::from_str(&approved_invite.to_string())
            .map_err(TargetFedimintError::InviteCode)?;
        let (mut done, seq) = match self
            .open_slot(
                federation_id,
                invite_code,
                federation_client_db_path(federations_dir, federation_id),
                esplora_url.cloned(),
            )
            .await
        {
            OpenSlot::Cached(client) => return Ok(client),
            OpenSlot::AtCapacity => return Err(TargetFedimintError::OpensAtCapacity),
            OpenSlot::Pending { done, seq } => (done, seq),
        };

        // Awaiting the slot is cancellable; the open behind it is not. A caller
        // dropped here — the item budget expiring — leaves the open running and
        // owned, and the next caller attaches to this same slot instead of
        // starting a second `open_rocksdb` against a lock the first one holds.
        if done.borrow().is_none() && done.changed().await.is_err() {
            // The sender went away without publishing, which means the task
            // panicked. Nothing else clears a slot, so leaving it would make
            // every later caller attach to a dead one and never retry.
            self.forget_open(federation_id, seq).await;
            return Err(TargetFedimintError::SharedOpen(
                "the open task ended without reporting a result".to_owned(),
            ));
        }
        let result = done.borrow().clone().ok_or_else(|| {
            TargetFedimintError::SharedOpen("the open reported no result".to_owned())
        })?;
        match &*result {
            Ok(client) => Ok(client.clone()),
            Err(reason) => Err(TargetFedimintError::SharedOpen(reason.clone())),
        }
    }

    /// Returns the in-flight open for `federation_id`, starting one if none is
    /// running.
    ///
    /// The open runs on a spawned task so that no caller's cancellation can
    /// drop it. Deliberately never aborted: aborting would drop the very build
    /// whose detached Fedimint task is the leak, which is what this exists to
    /// prevent.
    async fn open_slot(
        &self,
        federation_id: &str,
        invite_code: InviteCode,
        db_path: PathBuf,
        esplora_url: Option<SafeUrl>,
    ) -> OpenSlot {
        let mut inner = self.inner.lock().await;
        // Re-check the installed clients here, under the same guard that
        // registers a slot. The caller's own check happened earlier and a
        // finishing open installs its client before clearing its slot, so a
        // caller preempted between the two would otherwise find neither, spawn
        // a redundant open, and block that open on the file lock of the client
        // just installed. Checking both under one guard is what closes it.
        if let Some(client) = inner.clients.get(federation_id).cloned() {
            inner.touch(federation_id);
            return OpenSlot::Cached(client);
        }
        if let Some(pending) = inner.opens.get(federation_id) {
            return OpenSlot::Pending {
                done: pending.done.clone(),
                seq: pending.seq,
            };
        }

        // Report before the capacity branch, not inside it. A stuck open is
        // worth saying while slots remain: by the time the budget is full the
        // deployment has already stopped opening new target clients, and the
        // point of the report is to reach the operator before that.
        let now = std::time::Instant::now();
        for (stuck_federation, age) in inner.take_newly_stuck_opens(now) {
            warn!(
                federation_id = %stuck_federation,
                age_seconds = age.as_secs(),
                pending = inner.opens.len(),
                budget = MAX_PENDING_OPENS,
                "target federation client open has not completed; its pending slot is held \
                 until this process restarts"
            );
        }

        if !inner.may_start_open() {
            // Name the occupants. Nothing here can reclaim a slot, so the
            // actionable content of this message is which federations are
            // holding them.
            warn!(
                federation_id = %federation_id,
                budget = MAX_PENDING_OPENS,
                occupants = ?inner.pending_open_ages(now),
                "refusing a target federation client open: every pending slot is in use"
            );
            return OpenSlot::AtCapacity;
        }

        let (tx, rx) = tokio::sync::watch::channel(None);
        inner.next_open_seq += 1;
        let seq = inner.next_open_seq;
        let pool = self.clone();
        let id = federation_id.to_owned();
        tokio::spawn(async move {
            let outcome = create_or_load_client(&invite_code, &db_path, esplora_url.as_ref())
                .await
                .map_err(|error| error.to_string());

            // The guard is scoped so nothing awaits while holding it. A
            // superseded client's `shutdown` awaits a 30-second join, and
            // holding the pool mutex across that would stall every federation,
            // not just this one.
            let (outcome, superseded) = {
                let mut inner = pool.inner.lock().await;
                // Only install the client if this is still the open the pool is
                // waiting for. `evict` and `shutdown_all` drop the slot to say
                // the answer is no longer wanted, and without this check an open
                // already in flight would silently undo an operator's eviction
                // and reinstall a client built against the pre-change
                // configuration.
                let current = inner
                    .opens
                    .get(&id)
                    .is_some_and(|pending| pending.seq == seq);
                match (current, outcome) {
                    (true, Ok(client)) => {
                        inner.clients.insert(id.clone(), client.clone());
                        inner.touch(&id);
                        (Ok(client), None)
                    }
                    (false, Ok(client)) => (
                        Err("the open was superseded before it finished".to_owned()),
                        Some(client),
                    ),
                    (_, Err(reason)) => (Err(reason), None),
                }
            };

            // Publish before clearing the slot, and clear it only after. A
            // caller that takes the pool mutex in between must find the slot
            // and attach to this answer; if it found the slot already gone it
            // would start a fresh open against the database this one just
            // installed a client for, and block on that client's file lock.
            let _ = tx.send(Some(Arc::new(outcome)));
            pool.forget_open(&id, seq).await;

            // Superseded, and this task holds the only reference. Shut it down
            // rather than dropping it, so the task group and file lock go now
            // instead of when the last waiter's clone falls away.
            if let Some(client) = superseded
                && let Ok(handle) = Arc::try_unwrap(client)
            {
                handle.shutdown().await;
            }
        });

        inner.opens.insert(
            federation_id.to_owned(),
            PendingOpen {
                done: rx.clone(),
                seq,
                started_at: now,
                stuck_reported: false,
            },
        );
        OpenSlot::Pending { done: rx, seq }
    }

    /// Drops a slot, but only if it is still the one `seq` names.
    ///
    /// Guards against clearing a fresh open started after the caller's own
    /// failed.
    async fn forget_open(&self, federation_id: &str, seq: u64) {
        let mut inner = self.inner.lock().await;
        if inner
            .opens
            .get(federation_id)
            .is_some_and(|pending| pending.seq == seq)
        {
            inner.opens.remove(federation_id);
        }
    }

    /// Closes idle clients, least recently used first, until one more can open
    /// without exceeding the configured ceiling.
    ///
    /// The pool is keyed by federation id, and FI-supplied invite data decides
    /// which ids reach it, so without this every distinct target a requester
    /// named cost a RocksDB handle and a set of background tasks for the life
    /// of the process.
    ///
    /// When every open client is in use this returns having closed nothing.
    /// Briefly exceeding the ceiling is the better failure: the alternative is
    /// closing a client a worker is mid-deposit against, which does not stop
    /// that worker but does leave its file lock outstanding against the reopen
    /// the next pass performs. The overshoot is bounded by how many clients
    /// workers hold at once, not by requester input.
    async fn make_room(&self) {
        loop {
            let victim = {
                let mut inner = self.inner.lock().await;
                // Prune before the branch, not inside one of them. Pruning used
                // to happen only where there was already room, so the map grew
                // per distinct federation id exactly when it was under pressure
                // and never shrank — a requester-set unbounded map hiding
                // behind the client ceiling, which does not count locks.
                inner.prune_idle_locks();
                if inner.has_room(self.max_open) {
                    return;
                }
                inner.least_recently_used_idle()
            };
            let Some(victim) = victim else {
                debug!(
                    open = self.max_open.get(),
                    "every open target federation client is in use; \
                     opening one more rather than closing a client under a worker"
                );
                return;
            };
            if !self.evict_if_idle(&victim).await {
                // Something acquired the victim between selecting it and
                // locking it. Leave the pool as it is: retrying immediately
                // would spin against a worker that is going to hold the client
                // for the length of a deposit.
                return;
            }
        }
    }

    /// Closes one target federation's client, but only while nothing holds it.
    ///
    /// Unlike [`Self::evict`], which is an operator remediation that must work
    /// on a wedged client whatever else is using it, this is routine capacity
    /// reclamation and must not disturb work in progress. Holding the
    /// federation lock across the check keeps a concurrent open from handing
    /// out a clone of the very handle being closed; holding the inner lock
    /// across the removal keeps the cached reference count meaningful, so
    /// `try_unwrap` cannot lose a race and leave the RocksDB lock held.
    async fn evict_if_idle(&self, federation_id: &str) -> bool {
        let federation_lock = {
            let mut inner = self.inner.lock().await;
            inner
                .locks
                .entry(federation_id.to_owned())
                .or_default()
                .clone()
        };
        let _guard = federation_lock.lock().await;

        let client = {
            let mut inner = self.inner.lock().await;
            let idle = inner
                .clients
                .get(federation_id)
                .is_some_and(|client| Arc::strong_count(client) == 1);
            if !idle {
                return false;
            }
            inner.take(federation_id)
        };
        let Some(client) = client else {
            return false;
        };

        match Arc::try_unwrap(client) {
            // Spawned, not awaited. `make_room` runs on whichever future asked
            // for a client, and for the stability worker that future is under a
            // per-item budget. `shutdown` awaits `shutdown_join_all` with its
            // own 30-second deadline, so awaiting it here would let the budget
            // drop the shutdown part-way and leave the victim's task group
            // alive holding database clones — the same leak this pool exists to
            // prevent, reached through reclamation instead of through an open.
            Ok(handle) => {
                tokio::spawn(async move { handle.shutdown().await });
            }
            Err(client) => {
                // Not reachable through the checks above, and not worth a panic
                // if it ever becomes so: the handle still shuts down when the
                // last reference drops.
                debug!(
                    federation_id,
                    outstanding = Arc::strong_count(&client),
                    "idle target federation client was referenced after removal; \
                     shutdown completes when the last use drops it"
                );
            }
        }
        true
    }

    /// Closes a target federation's client so the next [`Self::create_or_load`]
    /// reopens it, releasing the RocksDB file lock in between.
    ///
    /// This is the remediation path for a client that has become unusable
    /// (a wedged executor, a database that needs to be replaced underneath it).
    /// Without it the only way to reopen one is to restart the daemon, because
    /// the map is otherwise insert-only and the handle lives as long as the
    /// process.
    ///
    /// Returns whether anything was closed or cancelled — a built client, an
    /// open in flight, or both. A dropped slot is reported as action taken even
    /// though its file lock is released only when that open finishes.
    ///
    /// An open already in flight is cancelled as far as this pool is concerned:
    /// its slot is dropped, so when it finishes it shuts the client down
    /// instead of installing it. Without that, an eviction could be silently
    /// undone moments later by an open that started before it and reinstall a
    /// client built against the configuration this call exists to change. The
    /// per-federation lock does not serialise against a concurrent open, because
    /// the open runs on its own task; the sequence check is what orders them.
    pub(crate) async fn evict(&self, federation_id: &str) -> bool {
        let federation_lock = {
            let mut inner = self.inner.lock().await;
            inner
                .locks
                .entry(federation_id.to_owned())
                .or_default()
                .clone()
        };
        let _guard = federation_lock.lock().await;

        let client = {
            let mut inner = self.inner.lock().await;
            let superseded = inner.opens.remove(federation_id).is_some();
            match inner.take(federation_id) {
                Some(client) => client,
                None => return superseded,
            }
        };

        // Removing the map entry already guarantees no *new* clones are handed
        // out. A worker mid-pass may still hold one, in which case `ClientHandle`'s
        // own `Drop` performs the shutdown when that clone goes away; taking the
        // explicit path when we can avoids relying on `block_in_place` inside a
        // drop.
        match Arc::try_unwrap(client) {
            Ok(handle) => handle.shutdown().await,
            Err(client) => {
                debug!(
                    federation_id,
                    outstanding = Arc::strong_count(&client),
                    "target federation client evicted while still referenced; \
                     shutdown completes when the last in-flight use drops it"
                );
            }
        }
        true
    }

    /// Closes every open target federation client, releasing the RocksDB file
    /// locks under the data dir.
    ///
    /// This is the teardown a live restore runs before replacing the data dir:
    /// a runtime generation may not outlive the files it was opened against.
    /// Returns the federation ids that were open.
    ///
    /// A client still referenced by an in-flight caller cannot be shut down
    /// here, and is logged rather than waited on. That is not a correctness gap
    /// for restore: the swap moves the whole data dir aside with `rename`, so a
    /// lingering handle keeps operating on the moved-aside directory and cannot
    /// reach the restored state.
    pub(crate) async fn shutdown_all(&self) -> Vec<String> {
        let open = {
            let mut inner = self.inner.lock().await;
            inner.usage.clear();
            if !inner.opens.is_empty() {
                // Deliberately not aborted: aborting drops the build, which is
                // the orphan this pool exists to prevent. Dropping the slots
                // instead makes every in-flight open shut its client down when
                // it finishes rather than install it, so a resurrected client
                // cannot land in a pool no shutdown path can reach again.
                //
                // It does not make teardown complete. An open still holds a
                // RocksDB handle and file lock until it finishes, and unlike a
                // built client it may not have opened its files yet — so it can
                // take a lock on the *restored* data dir after the swap rather
                // than on the moved-aside one. That is a live residual, not
                // something the moved-aside-directory reasoning covers.
                debug!(
                    pending = inner.opens.len(),
                    "target federation opens still in flight at runtime teardown; \
                     superseded rather than cancelled mid-build"
                );
                inner.opens.clear();
            }
            std::mem::take(&mut inner.clients)
        };

        let mut federation_ids = Vec::with_capacity(open.len());
        for (federation_id, client) in open {
            match Arc::try_unwrap(client) {
                Ok(handle) => handle.shutdown().await,
                Err(client) => {
                    debug!(
                        federation_id,
                        outstanding = Arc::strong_count(&client),
                        "target federation client still referenced at runtime teardown; \
                         shutdown completes when the last in-flight use drops it"
                    );
                }
            }
            federation_ids.push(federation_id);
        }
        federation_ids
    }

    /// Opens the pool currently owns. Test-only: the property this exists to
    /// pin is that a cancelled caller leaves exactly one.
    #[cfg(test)]
    pub(crate) async fn pending_open_count(&self) -> usize {
        self.inner.lock().await.opens.len()
    }

    /// Sequence of the open in flight for `federation_id`, if any.
    ///
    /// Test-only, and the only way to tell "attached to the running open" from
    /// "started another one": `opens` is keyed by federation id, so a second
    /// open would overwrite at the same key and leave the count unchanged.
    #[cfg(test)]
    pub(crate) async fn pending_open_seq(&self, federation_id: &str) -> Option<u64> {
        self.inner
            .lock()
            .await
            .opens
            .get(federation_id)
            .map(|pending| pending.seq)
    }
}

/// Picks the eviction victim: the longest-unused federation `idle` accepts.
///
/// A client whose handle has escaped the map is in use by a worker. Closing it
/// does not stop that worker — it keeps the clone it already holds — but it
/// does leave that worker's RocksDB file lock outstanding against the reopen
/// the next pass performs, so a busy client is not a candidate however old it
/// is. Skipping past it rather than stopping matters: one long deposit against
/// the oldest target must not make the whole pool unreclaimable.
///
/// Separate from the pool so the ordering rule can be tested without a live
/// Fedimint client, which is the part a later change is most likely to get
/// wrong.
fn least_recently_used_idle(usage: &[String], idle: impl Fn(&str) -> bool) -> Option<String> {
    usage
        .iter()
        .find(|federation_id| idle(federation_id))
        .cloned()
}

fn federation_client_db_path(federations_dir: &Path, federation_id: &str) -> PathBuf {
    federations_dir.join(federation_id).join("client.db")
}

#[cfg(test)]
#[path = "../tests/target_fedimint.rs"]
mod tests;

#[derive(Debug, Error)]
pub(crate) enum TargetFedimintError {
    #[error("failed to parse target federation invite code: {0}")]
    InviteCode(#[source] anyhow::Error),
    #[error("failed to load stored client secret: {0}")]
    LoadClientSecret(#[source] anyhow::Error),
    #[error("failed to convert entropy into mnemonic: {0}")]
    MnemonicFromEntropy(#[source] anyhow::Error),
    #[error("failed to store generated client secret: {0}")]
    StoreClientSecret(#[source] anyhow::Error),
    #[error("failed to build fedimint client builder: {0}")]
    ClientBuilder(#[source] anyhow::Error),
    #[error("failed to build fedimint connectors: {0}")]
    Connectors(#[source] anyhow::Error),
    #[error("failed to build the target client's chain backend: {0}")]
    ChainBackend(#[source] anyhow::Error),
    #[error("failed to open RocksDB at {path}: {source}")]
    OpenRocksDb {
        path: String,
        #[source]
        source: anyhow::Error,
    },
    #[error(
        "too many target federation client opens are already in flight; retry on the next pass"
    )]
    OpensAtCapacity,
    #[error("failed to preview federation client config: {0}")]
    Preview(#[source] anyhow::Error),
    #[error("timed out after {seconds}s previewing federation client config")]
    PreviewTimeout { seconds: u64 },
    #[error("failed to join federation client: {0}")]
    Join(#[source] anyhow::Error),
    #[error("failed to open existing federation client: {0}")]
    Open(#[source] anyhow::Error),
    #[error("target federation endpoint is not one FLIP will dial: {0}")]
    EndpointPolicy(String),
    /// The failure of an open another caller started and this one waited on.
    ///
    /// Carried as text because the originating error is not `Clone` and one
    /// open now answers every waiter. Every caller maps this to `unavailable`,
    /// so nothing downstream distinguishes it from the original.
    #[error("target federation client open failed: {0}")]
    SharedOpen(String),
}

async fn load_or_generate_mnemonic(
    db: &FedimintDatabase,
) -> Result<fedimint_bip39::Mnemonic, TargetFedimintError> {
    let mnemonic = if let Ok(entropy) = Client::load_decodable_client_secret::<Vec<u8>>(db).await {
        fedimint_bip39::Mnemonic::from_entropy(&entropy)
            .map_err(|source| TargetFedimintError::MnemonicFromEntropy(source.into()))?
    } else {
        debug!("generating target federation client mnemonic");
        let mnemonic = fedimint_bip39::Bip39RootSecretStrategy::<12>::random(
            &mut fedimint_core::secp256k1::rand::thread_rng(),
        );
        Client::store_encodable_client_secret(db, mnemonic.to_entropy())
            .await
            .map_err(TargetFedimintError::StoreClientSecret)?;
        mnemonic
    };
    Ok(mnemonic)
}

async fn make_client_builder(
    esplora_url: Option<&SafeUrl>,
) -> Result<(fedimint_client::ClientBuilder, ConnectorRegistry), TargetFedimintError> {
    let mut client_builder = Client::builder()
        .await
        .map_err(TargetFedimintError::ClientBuilder)?;
    client_builder.with_module(MetaClientInit);
    client_builder.with_module(MintClientInit);
    client_builder.with_module(LightningClientInit::default());
    // The Fedimint wallet client watches the chain over esplora and nothing
    // else, and left to itself it builds one from whatever the target
    // federation advertises as its `default_bitcoin_rpc`. That makes claiming a
    // peg-in depend on a third party's endpoint being reachable and honest.
    // Point it at the chain backend this daemon was configured with instead,
    // when that backend is one the client can use.
    let wallet = match esplora_url {
        Some(url) => WalletClientInit::new(
            create_esplora_rpc(url).map_err(TargetFedimintError::ChainBackend)?,
        ),
        None => WalletClientInit::default(),
    };
    client_builder.with_module(wallet);
    client_builder.with_module(stability_pool_client::StabilityPoolClientInit::default());
    let connectors = ConnectorRegistry::build_from_client_defaults()
        .bind()
        .await
        .map_err(TargetFedimintError::Connectors)?;
    Ok((client_builder, connectors))
}

async fn load_root_secret(db: &FedimintDatabase) -> Result<RootSecret, TargetFedimintError> {
    let secret = Client::load_decodable_client_secret::<Vec<u8>>(db)
        .await
        .map_err(TargetFedimintError::LoadClientSecret)?;
    let mnemonic = fedimint_bip39::Mnemonic::from_entropy(&secret)
        .map_err(|source| TargetFedimintError::MnemonicFromEntropy(source.into()))?;
    Ok(RootSecret::StandardDoubleDerive(Bip39RootSecretStrategy::<
        12,
    >::to_root_secret(
        &mnemonic
    )))
}

async fn open_rocksdb(
    db_path: &Path,
) -> Result<fedimint_db_locked::Locked<fedimint_rocksdb::RocksDb>, TargetFedimintError> {
    // Open the client database exactly the way every other Fedimint client
    // does (standard options, WAL consistency, and the on-disk file lock)
    // instead of a bespoke `OptimisticTransactionDB::open`. The client spawns
    // concurrent background tasks (module migrations, the peg-in monitor, the
    // deposit-version probe) that share this database; the blessed opener gives
    // them the transaction semantics they expect, so they do not panic on a
    // spurious write conflict at startup.
    fedimint_rocksdb::RocksDb::build(db_path)
        .open()
        .await
        .map_err(|source| TargetFedimintError::OpenRocksDb {
            path: db_path.display().to_string(),
            source,
        })
}

async fn create_or_load_client(
    invite_code: &InviteCode,
    rocksdb: &Path,
    esplora_url: Option<&SafeUrl>,
) -> Result<ClientHandleArc, TargetFedimintError> {
    let db: FedimintDatabase = open_rocksdb(rocksdb).await?.into();
    let (client_builder, connectors) = make_client_builder(esplora_url).await?;

    let client = if Client::is_initialized(&db).await {
        debug!("target federation client is already initialized");
        let root_secret = load_root_secret(&db).await?;
        client_builder
            .open(connectors, db, root_secret)
            .await
            .map_err(TargetFedimintError::Open)?
    } else {
        debug!(
            federation_id = %invite_code.federation_id(),
            "joining target federation client"
        );
        let mnemonic = load_or_generate_mnemonic(&db).await?;
        let root_secret = RootSecret::StandardDoubleDerive(
            Bip39RootSecretStrategy::<12>::to_root_secret(&mnemonic),
        );
        let preview = tokio::time::timeout(
            TARGET_PREVIEW_BUDGET,
            client_builder.preview(connectors, invite_code),
        )
        .await
        .map_err(|_| TargetFedimintError::PreviewTimeout {
            seconds: TARGET_PREVIEW_BUDGET.as_secs(),
        })?
        .map_err(TargetFedimintError::Preview)?;
        preview
            .join(db, root_secret)
            .await
            .map_err(TargetFedimintError::Join)?
    };
    Ok(Arc::new(client))
}
