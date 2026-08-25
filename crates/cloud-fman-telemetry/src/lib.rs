//! Single-active FMan admission, durable safe-journal archive, and sparse metrics collector.

mod admission;
mod archive;
mod auth;
mod cipher;
mod config;
mod data_root_lock;
mod iroh_journal_source;
mod journal_catalog;
mod journal_collector;
mod journal_commit;
mod journal_poller;
mod journal_target;
mod journal_types;
mod logging;
mod metrics_observability;
mod metrics_policy;
mod metrics_poller;
mod metrics_snapshot;
mod metrics_types;
mod metrics_worker;
mod server;
mod store;

pub use config::Args;
pub use logging::init as init_logging;
#[cfg(feature = "test-support")]
pub use server::registration_router_for_test;
pub use server::serve;
