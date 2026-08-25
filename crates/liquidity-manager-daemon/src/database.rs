//! The local SQLite database that owns every piece of FLIP's durable state.
//!
//! Opened in WAL mode with a busy timeout, migrations embedded. `begin_write`
//! opens `BEGIN IMMEDIATE`, so a writer that must fence against a concurrent
//! commit takes the write lock before it reads.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use fedi_decentralized_service_liquidity_manager::{
    ListResponse, PageCursor, PageRequest, ServiceError, ServiceErrorCode, ServiceResult,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, migrate::Migrator};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone, Debug)]
pub struct Database {
    pool: SqlitePool,
    path: PathBuf,
}

impl Database {
    /// Opens SQLite and runs embedded migrations.
    pub async fn connect(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create SQLite parent dir {parent:?}"))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("failed to open SQLite database {path:?}"))?;

        MIGRATOR
            .run(&pool)
            .await
            .with_context(|| format!("failed to run SQLite migrations for {path:?}"))?;
        tracing::debug!(path = %path.display(), "opened SQLite and ran migrations");

        Ok(Self {
            pool,
            path: path.to_path_buf(),
        })
    }

    /// Begins a write transaction, taking SQLite's single write lock up
    /// front (`BEGIN IMMEDIATE`) so concurrent writers queue on the busy
    /// timeout. A default deferred `begin()` that reads before writing
    /// cannot upgrade to the write lock while another writer holds it —
    /// SQLite fails that upgrade immediately with `SQLITE_BUSY` because the
    /// transaction's snapshot is already stale. Every transaction that will
    /// write must start here.
    ///
    /// # Nothing slow may be awaited while one is open
    ///
    /// Taking the write lock up front is what makes the queueing predictable,
    /// and it is also what makes an open transaction expensive: it is a
    /// process-wide write lock, so every other writer — every worker pass,
    /// every admin verb — waits on the busy timeout for as long as this
    /// transaction lives. A transaction is for the writes, not for the work
    /// that decided them.
    ///
    /// The sharp case is network I/O. `VerificationProvider::verify` performs
    /// relay revocation lookups and a federation preview, so its own doc
    /// forbids calling it inside a transaction; the reason it must be obeyed
    /// is here. `public.rs` shows the intended shape — verify first, then open
    /// the transaction and re-read the setup revision inside it to fence the
    /// commit against anything that changed meanwhile.
    pub async fn begin_write(&self) -> sqlx::Result<sqlx::Transaction<'static, Sqlite>> {
        self.pool.begin_with("BEGIN IMMEDIATE").await
    }

    /// Checks that the database connection can execute a simple query.
    pub async fn ping(&self) -> anyhow::Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .context("SQLite health query failed")?;

        Ok(())
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Closes the pool and waits for its connections to finish.
    ///
    /// Dropping every clone eventually has the same effect, but a live restore
    /// replaces the SQLite file underneath this handle and must know the old
    /// connections are gone before it does. `SqlitePool` is `Arc` internally, so
    /// dropping is not a synchronization point; this is.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Offset pagination shared by the admin list APIs: the wire cursor is the
/// decimal row offset, limits clamp to 1..=100, and queries fetch one extra
/// row so `list_response` can tell whether a next page exists.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OffsetPage {
    limit: u32,
    offset: u32,
}

impl OffsetPage {
    pub(crate) fn from_page(page: &PageRequest, what: &str) -> ServiceResult<Self> {
        let offset = page
            .cursor
            .as_ref()
            .map(|cursor| cursor.0.parse::<u32>())
            .transpose()
            .map_err(|_| {
                ServiceError::with_code(
                    ServiceErrorCode::InvalidArgument,
                    format!("invalid {what} page cursor"),
                )
            })?
            .unwrap_or_default();
        Ok(Self {
            limit: page.limit.clamp(1, 100),
            offset,
        })
    }

    /// LIMIT to bind: one past the page size, to detect a next page.
    pub(crate) fn fetch_limit(&self) -> i64 {
        i64::from(self.limit + 1)
    }

    pub(crate) fn offset(&self) -> i64 {
        i64::from(self.offset)
    }

    pub(crate) fn list_response<R, T>(
        &self,
        rows: Vec<R>,
        mapper: impl FnMut(R) -> ServiceResult<T>,
    ) -> ServiceResult<ListResponse<T>> {
        let has_more = rows.len() > self.limit as usize;
        let items = rows
            .into_iter()
            .take(self.limit as usize)
            .map(mapper)
            .collect::<Result<Vec<_>, _>>()?;
        let next_page = has_more.then(|| PageCursor((self.offset + self.limit).to_string()));
        Ok(ListResponse { items, next_page })
    }
}

/// Appends `column IN (?, ...)` to the query, binding each value's wire
/// string (`Display`), so status vocabularies stay owned by their enums
/// instead of being restated as SQL literals.
pub(crate) fn push_in_list<T: std::fmt::Display>(
    builder: &mut QueryBuilder<'_, Sqlite>,
    column: &str,
    values: &[T],
) {
    builder.push(column);
    builder.push(" IN (");
    let mut separated = builder.separated(", ");
    for value in values {
        separated.push_bind(value.to_string());
    }
    separated.push_unseparated(")");
}

#[cfg(test)]
#[path = "../tests/database.rs"]
mod tests;
