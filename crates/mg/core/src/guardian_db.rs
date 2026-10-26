//! Read-only facts from a seat child's own Fedimint database, taken only
//! while no fedimintd process owns it.

use std::path::Path;

use fedimint_core::db::{Database, IDatabaseTransactionOpsCoreTyped as _};
use fedimint_core::module::registry::ModuleRegistry;
use fedimint_server::config::io::DB_FILE;
use fedimint_server::consensus::db::AlephUnitsPrefix;
use futures::StreamExt as _;

/// Whether the stopped guardian holds no AlephBFT units of an unfinished
/// session. fedimintd saves each unit before it broadcasts it and removes the
/// saved units when it commits the session, so an empty set proves its key
/// signed nothing in any session it did not complete. fedimintd reads the
/// same prefix to decide whether it restarts inside a session.
///
/// The caller guarantees that the guardian's process has exited.
pub(crate) async fn stopped_between_sessions(data_dir: &Path) -> anyhow::Result<bool> {
    let path = data_dir.join(DB_FILE);
    // The RocksDB adapter blocks in place on every read, which a Tokio
    // worker of a current-thread runtime refuses, so the whole read runs on
    // the blocking pool.
    tokio::task::spawn_blocking(move || {
        let raw = fedimint_rocksdb::RocksDbReadOnly::open_read_only_blocking(&path)?;
        let db = Database::new(raw, ModuleRegistry::default());
        futures::executor::block_on(async {
            let mut dbtx = db.begin_transaction_nc().await;
            let in_session = dbtx
                .find_by_prefix(&AlephUnitsPrefix)
                .await
                .next()
                .await
                .is_some();
            Ok(!in_session)
        })
    })
    .await?
}
