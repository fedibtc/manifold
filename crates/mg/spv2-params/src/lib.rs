//! The Manifold SPv2 (stability pool) module parameters shared by every
//! bundled `fedimintd` line (ARCH-manifold-guardian-seat-processes *Child
//! environment*).
//!
//! Each line builds its own `StabilityPoolInit` from these primitives, because
//! the init type comes from a stability-pool build linked against that line's
//! Fedimint.

use std::time::Duration;

pub const CYCLE_DURATION: Duration = Duration::from_secs(10 * 60);
pub const MIN_SEEK_MSATS: u64 = 10_000;
pub const MIN_PROVIDE_MSATS: u64 = 100_000;
pub const MAX_PROVIDER_FEE_RATE_PPB: u64 = 22_062;
pub const MIN_CANCELLATION_BPS: u32 = 100;
/// Provider and seeker collateral ratio parts.
pub const COLLATERAL_RATIO: (u8, u8) = (1, 1);

/// Whether this child runs in the network-isolated local E2E environment, where
/// real fedimintd processes use the module's deterministic oracle rather than
/// waiting forever on public price sources.
pub fn use_mock_oracle() -> bool {
    std::env::var_os("MG_E2E_LOCAL_IROH").is_some()
}
