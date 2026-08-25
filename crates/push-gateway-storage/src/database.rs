use std::time::Duration;

use sqlx::{AnyConnection, AnyPool, Executor, any::AnyPoolOptions};

use crate::DatabaseWriteLock;

/// Database backend selected by the configured database URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    /// Local or development SQLite database.
    Sqlite,
    /// Production-oriented PostgreSQL database.
    Postgres,
}

/// Database handle supporting SQLite for local/dev and PostgreSQL for production.
#[derive(Clone, Debug)]
pub struct Database {
    /// Runtime-selected SQLx pool.
    pool: AnyPool,
    /// Backend selected from the configured URL scheme.
    backend: DatabaseBackend,
    /// Clone-shared gateway mutation coordinator for this pool.
    write_lock: DatabaseWriteLock,
}

impl Database {
    /// Connects to the configured database backend and runs migrations.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        Self::connect_url(database_url, true).await
    }

    /// Connects to an existing database without running migrations.
    ///
    /// This is intended for read-only or explicitly-confirmed admin tooling where
    /// a newer binary must not mutate schema state while another service version
    /// may be active.
    pub async fn connect_existing(database_url: &str) -> Result<Self, sqlx::Error> {
        Self::connect_url(database_url, false).await
    }

    async fn connect_url(database_url: &str, run_migrations: bool) -> Result<Self, sqlx::Error> {
        sqlx::any::install_default_drivers();
        let backend = DatabaseBackend::from_url(database_url)?;
        let mut pool_options = AnyPoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5));
        if backend == DatabaseBackend::Sqlite {
            pool_options = pool_options
                .after_connect(|connection, _metadata| Box::pin(configure_sqlite(connection)));
        } else {
            pool_options = pool_options
                .after_connect(|connection, _metadata| Box::pin(configure_postgres(connection)));
        }

        let pool = pool_options.connect(database_url).await?;

        if run_migrations {
            sqlx::migrate!("./migrations").run(&pool).await?;
        }

        Ok(Self {
            pool,
            backend,
            write_lock: DatabaseWriteLock::default(),
        })
    }

    /// Returns the runtime-selected SQLx pool.
    #[must_use]
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Returns the configured database backend.
    #[must_use]
    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }

    /// Returns the clone-shared gateway database-write coordinator for this pool.
    #[must_use]
    pub fn write_lock(&self) -> DatabaseWriteLock {
        self.write_lock.clone()
    }

    /// Returns true when the database is reachable and migrations metadata exists.
    pub async fn is_ready(&self) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&self.pool)
            .await
            .is_ok()
            && sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }
}

impl DatabaseBackend {
    /// Detects the backend from a SQLx database URL.
    fn from_url(database_url: &str) -> Result<Self, sqlx::Error> {
        let lower = database_url.to_ascii_lowercase();
        if lower.starts_with("sqlite:") {
            Ok(Self::Sqlite)
        } else if lower.starts_with("postgres:") || lower.starts_with("postgresql:") {
            Ok(Self::Postgres)
        } else {
            Err(sqlx::Error::Configuration(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsupported PUSH_GATEWAY_DATABASE_URL scheme; use sqlite:// or postgres://",
                )
                .into(),
            ))
        }
    }
}

async fn configure_sqlite(connection: &mut AnyConnection) -> Result<(), sqlx::Error> {
    connection.execute("PRAGMA foreign_keys = ON").await?;
    connection.execute("PRAGMA journal_mode = WAL").await?;
    connection.execute("PRAGMA synchronous = NORMAL").await?;
    connection.execute("PRAGMA busy_timeout = 5000").await?;
    Ok(())
}

async fn configure_postgres(connection: &mut AnyConnection) -> Result<(), sqlx::Error> {
    // Keep an indefinitely stalled database statement from holding the gateway's
    // process-local mutation coordinator forever. This hook runs only for pools
    // selected as PostgreSQL, so SQLite never sees PostgreSQL-specific SQL.
    connection
        .execute("SET statement_timeout = '5000ms'")
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
