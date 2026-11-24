//! The FA Stats figures one seat's guardian reports for its federation.
//!
//! Every input comes from the seat's own `fedimintd`. Custody and volume are
//! derived from consensus, so in-sync guardians agree on them. Active wallets
//! come from client backups, which each guardian stores locally. A guardian
//! that moved or was restored therefore starts a new 30-day backup window and
//! undercounts until it passes, which the FA's median over the guardians
//! absorbs while only a minority of them do.

use anyhow::{Context as _, ensure};
use fedi_decentralized_service_manifold_guardian::{
    ActiveWalletHistory, FedimintStats, WindowedFigure,
};
use fedimint_core::module::audit::AuditSummary;
use fedimint_lnv2_common::{LightningVolume, LightningVolumeStats};
use fedimint_walletv2_common::{WalletVolume, WalletVolumeStats};

/// Everything one guardian's `fedimintd` reported for the figures.
pub(crate) struct GuardianReadings {
    pub audit: AuditSummary,
    pub active_wallets: WindowedFigure,
    pub history: ActiveWalletHistory,
    pub lnv2_volume: LightningVolumeStats,
    pub walletv2_volume: WalletVolumeStats,
}

pub(crate) fn fedimint_stats(readings: &GuardianReadings) -> anyhow::Result<FedimintStats> {
    Ok(FedimintStats {
        custodied_msats: custodied_msats(&readings.audit)?,
        active_wallets_30d: readings.active_wallets,
        active_wallets_history: Some(readings.history),
        volume_1008_blocks_msats: volume(
            &readings.lnv2_volume.last_1008_blocks,
            &readings.walletv2_volume.last_1008_blocks,
        )?,
        volume_4320_blocks_msats: volume(
            &readings.lnv2_volume.last_4320_blocks,
            &readings.walletv2_volume.last_4320_blocks,
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
