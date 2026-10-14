//! A database that stops the process at a chosen write commit.
//!
//! Every write transaction of a test FA commits through [`CrashDb`]. The plan
//! counts those commits and, at the chosen one, stops before or after the
//! commit applies: the commit future never returns, as if the process died
//! there, and every later commit of the same process never applies either. A
//! reopened process gets a fresh plan over the same stored bytes.

use std::fmt;
use std::future::pending;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fedimint_core::db::mem_impl::{MemDatabase, MemTransaction};
use fedimint_core::db::{
    Database, DatabaseResult, IDatabaseTransactionOps, IDatabaseTransactionOpsCore, IRawDatabase,
    IRawDatabaseExt as _, IRawDatabaseTransaction, PrefixStream,
};
use tokio::sync::Notify;

/// Where a crash stops relative to its write commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CrashPoint {
    /// The commit never applies.
    BeforeCommit,
    /// The commit applies, and its caller never learns it did.
    AfterCommit,
}

/// One process's crash plan.
#[derive(Default)]
pub(super) struct CrashPlan {
    commits: AtomicUsize,
    crash_at: Mutex<Option<(usize, CrashPoint)>>,
    crashed: AtomicBool,
    reached: Notify,
}

impl CrashPlan {
    /// A plan that stops at the one-based write commit `commit`.
    pub(super) fn at(commit: usize, point: CrashPoint) -> Arc<Self> {
        let plan = Self::default();
        *plan.crash_at.lock().expect("crash plan lock") = Some((commit, point));
        Arc::new(plan)
    }

    /// A plan that never crashes and only counts commits.
    pub(super) fn never() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Write commits this process started.
    pub(super) fn commits(&self) -> usize {
        self.commits.load(Ordering::SeqCst)
    }

    pub(super) fn crashed(&self) -> bool {
        self.crashed.load(Ordering::SeqCst)
    }

    /// Wait until the process stops at its crash point.
    pub(super) async fn wait(&self) {
        self.reached.notified().await;
    }

    async fn checkpoint(&self, commit: usize, point: CrashPoint) {
        if self.crashed() {
            pending::<()>().await;
        }
        if *self.crash_at.lock().expect("crash plan lock") == Some((commit, point)) {
            self.crashed.store(true, Ordering::SeqCst);
            self.reached.notify_one();
            pending::<()>().await;
        }
    }
}

/// The stored bytes, shared by every process that opens them.
#[derive(Clone, Default)]
pub(super) struct CrashStorage(Arc<MemDatabase>);

impl CrashStorage {
    /// Open the bytes in a new process that follows `plan`.
    pub(super) fn open(&self, plan: Arc<CrashPlan>) -> Database {
        CrashDb {
            storage: self.0.clone(),
            plan,
        }
        .into_database()
    }
}

struct CrashDb {
    storage: Arc<MemDatabase>,
    plan: Arc<CrashPlan>,
}

impl fmt::Debug for CrashDb {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CrashDb")
    }
}

#[async_trait::async_trait]
impl IRawDatabase for CrashDb {
    type Transaction<'a> = CrashTx<'a>;

    async fn begin_transaction<'a>(&'a self) -> CrashTx<'a> {
        CrashTx {
            inner: self.storage.begin_transaction().await,
            writes: false,
            plan: self.plan.clone(),
        }
    }

    fn checkpoint(&self, _backup_path: &std::path::Path) -> DatabaseResult<()> {
        Ok(())
    }
}

struct CrashTx<'a> {
    inner: MemTransaction<'a>,
    writes: bool,
    plan: Arc<CrashPlan>,
}

impl fmt::Debug for CrashTx<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CrashTx")
    }
}

#[async_trait::async_trait]
impl IDatabaseTransactionOpsCore for CrashTx<'_> {
    async fn raw_insert_bytes(
        &mut self,
        key: &[u8],
        value: &[u8],
    ) -> DatabaseResult<Option<Vec<u8>>> {
        self.writes = true;
        self.inner.raw_insert_bytes(key, value).await
    }

    async fn raw_get_bytes(&mut self, key: &[u8]) -> DatabaseResult<Option<Vec<u8>>> {
        self.inner.raw_get_bytes(key).await
    }

    async fn raw_remove_entry(&mut self, key: &[u8]) -> DatabaseResult<Option<Vec<u8>>> {
        self.writes = true;
        self.inner.raw_remove_entry(key).await
    }

    async fn raw_find_by_prefix(&mut self, key_prefix: &[u8]) -> DatabaseResult<PrefixStream<'_>> {
        self.inner.raw_find_by_prefix(key_prefix).await
    }

    async fn raw_find_by_prefix_sorted_descending(
        &mut self,
        key_prefix: &[u8],
    ) -> DatabaseResult<PrefixStream<'_>> {
        self.inner
            .raw_find_by_prefix_sorted_descending(key_prefix)
            .await
    }

    async fn raw_find_by_range(&mut self, range: Range<&[u8]>) -> DatabaseResult<PrefixStream<'_>> {
        self.inner.raw_find_by_range(range).await
    }

    async fn raw_remove_by_prefix(&mut self, key_prefix: &[u8]) -> DatabaseResult<()> {
        self.writes = true;
        self.inner.raw_remove_by_prefix(key_prefix).await
    }
}

impl IDatabaseTransactionOps for CrashTx<'_> {}

#[async_trait::async_trait]
impl IRawDatabaseTransaction for CrashTx<'_> {
    async fn commit_tx(self) -> DatabaseResult<()> {
        if !self.writes {
            return self.inner.commit_tx().await;
        }
        let plan = self.plan;
        let commit = plan.commits.fetch_add(1, Ordering::SeqCst) + 1;
        plan.checkpoint(commit, CrashPoint::BeforeCommit).await;
        self.inner.commit_tx().await?;
        plan.checkpoint(commit, CrashPoint::AfterCommit).await;
        Ok(())
    }
}
