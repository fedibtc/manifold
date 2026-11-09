//! The FA Stats figures one seat's guardian reports for its federation.
//!
//! Every input comes from the seat's own `fedimintd`. Custody and volume are
//! derived from consensus, so in-sync guardians agree on them. Active wallets
//! come from client backups, which each guardian stores locally. A guardian
//! that moved or was restored therefore starts a new 30-day backup window,
//! and the figure says so instead of under-counting silently.

use std::time::Duration;

use anyhow::{Context as _, ensure};
use fedi_decentralized_service_manifold_guardian::{FedimintStats, WindowedFigure};
use fedimint_core::backup::BackupStatistics;
use fedimint_core::module::audit::AuditSummary;
use fedimint_lnv2_common::{LightningVolume, LightningVolumeStats};
use fedimint_walletv2_common::{WalletVolume, WalletVolumeStats};

/// The window `backup_statistics` reports as `refreshed_1m`.
const BACKUP_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long after the federation's first session a guardian may first hold
/// its config and still count as present from formation. The MG records a
/// DKG-formed seat when it observes the finished ceremony, a few seconds to
/// minutes after the first session.
const FORMATION_TOLERANCE: Duration = Duration::from_secs(60 * 60);

/// Everything one guardian's `fedimintd` reported for the figures.
pub(crate) struct GuardianReadings {
    pub audit: AuditSummary,
    pub backups: BackupStatistics,
    pub lnv2_volume: LightningVolumeStats,
    pub walletv2_volume: WalletVolumeStats,
    /// When the federation's first session ran, in unix seconds.
    pub federation_start_unix: u64,
    /// When this install first held the guardian's config.
    pub formed_at_ms: i64,
    pub now_ms: i64,
}

pub(crate) fn fedimint_stats(readings: &GuardianReadings) -> anyhow::Result<FedimintStats> {
    Ok(FedimintStats {
        custodied_msats: custodied_msats(&readings.audit)?,
        active_wallets_30d: WindowedFigure {
            value: u64::try_from(readings.backups.refreshed_1m)?,
            complete: backup_window_complete(
                readings.formed_at_ms,
                readings.federation_start_unix,
                readings.now_ms,
            ),
        },
        volume_7d_msats: volume(
            &readings.lnv2_volume.last_7_days,
            &readings.walletv2_volume.last_7_days,
        )?,
        volume_30d_msats: volume(
            &readings.lnv2_volume.last_30_days,
            &readings.walletv2_volume.last_30_days,
        )?,
    })
}

/// The walletv2 audit value: the bitcoin the federation holds on chain.
fn custodied_msats(audit: &AuditSummary) -> anyhow::Result<u64> {
    let mut wallets = audit
        .module_summaries
        .values()
        .filter(|summary| summary.kind == fedimint_walletv2_common::KIND.as_str());
    let wallet = wallets.next().context("the audit has no walletv2 module")?;
    ensure!(
        wallets.next().is_none(),
        "the audit has more than one walletv2 module"
    );
    u64::try_from(wallet.net_assets).context("the walletv2 audit value is negative")
}

/// Whether this guardian's client backups cover the last 30 days: it has held
/// the guardian since the federation formed, or for at least 30 days.
fn backup_window_complete(formed_at_ms: i64, federation_start_unix: u64, now_ms: i64) -> bool {
    let formed_at = i128::from(formed_at_ms);
    let federation_start = i128::from(federation_start_unix) * 1000;
    let since_formation = formed_at - federation_start <= i128::from(millis(FORMATION_TOLERANCE));
    let held_for_window = i128::from(now_ms) - formed_at >= i128::from(millis(BACKUP_WINDOW));
    since_formation || held_for_window
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).expect("a constant window fits in u64 milliseconds")
}

/// Lightning sent and received plus on-chain deposited and withdrawn. The
/// figure is complete only if both modules counted the whole window.
fn volume(lightning: &LightningVolume, wallet: &WalletVolume) -> anyhow::Result<WindowedFigure> {
    let onchain_msats = |amount: bitcoin::Amount| {
        amount
            .to_sat()
            .checked_mul(1000)
            .context("an on-chain amount overflows millisatoshis")
    };
    let value = [
        lightning.sent.msats,
        lightning.received.msats,
        onchain_msats(wallet.deposited)?,
        onchain_msats(wallet.withdrawn)?,
    ]
    .into_iter()
    .try_fold(0u64, u64::checked_add)
    .context("the volume overflows millisatoshis")?;
    Ok(WindowedFigure {
        value,
        complete: lightning.complete && wallet.complete,
    })
}

#[cfg(test)]
#[path = "../tests/fedimint_stats.rs"]
mod tests;
