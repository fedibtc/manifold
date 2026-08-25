//! Fleet Manager core: everything an FMan *is*, with holes where the
//! capabilities it uses would be.
//!
//! Seats, the store, allocation, identity derivation, and guardian-fee
//! policy live here. The things an FMan talks to — an ecash wallet
//! ([`wallet::EcashWallet`]), a fee vault
//! ([`guardian_fee::GuardianFeeVault`]), a backup sink
//! ([`backup::BackupSink`]) — are traits this crate defines and does not
//! implement, so adding a capability adds a dependency to the binary that
//! wires it, never to this crate.
//!
//! Module responsibilities, the concurrency model, and state ownership are
//! recorded in
//! [`ARCH-fleet-manager`](../../specs/ARCH-fleet-manager.md); the other records
//! under [`crates/fman/specs`](../../specs/) govern individual decisions and
//! behaviors (Linked Specs).

pub mod admin;
pub mod admin_http;
pub mod backup;
pub mod backup_worker;
pub mod bundled_fedimintd;
pub mod db;
pub mod directory;
pub mod facts;
pub mod fedimint_api;
pub mod fleet;
pub mod guardian_fee;
pub mod identity;
pub mod onboarding;
pub mod payout_wire;
pub mod push_callback;
pub mod remittance_metadata;
pub mod restore;
pub mod seat;
pub mod seat_process;
pub mod service;
pub mod wallet;

#[cfg(test)]
#[path = "../tests/support/db.rs"]
pub(crate) mod test_support;
