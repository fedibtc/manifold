use std::collections::BTreeMap;

use fedimint_core::Amount;
use fedimint_core::module::audit::ModuleSummary;

use super::*;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const FEDERATION_START_MS: i64 = 1_790_000_000_000;

fn audit(modules: &[(&str, i64)]) -> AuditSummary {
    AuditSummary {
        net_assets: modules.iter().map(|(_, net_assets)| net_assets).sum(),
        module_summaries: modules
            .iter()
            .zip(0..)
            .map(|((kind, net_assets), instance_id)| {
                (
                    instance_id,
                    ModuleSummary {
                        net_assets: *net_assets,
                        kind: (*kind).to_owned(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

fn lightning(sent: u64, received: u64, complete: bool) -> LightningVolume {
    LightningVolume {
        received: Amount::from_msats(received),
        sent: Amount::from_msats(sent),
        complete,
    }
}

fn wallet(deposited: u64, withdrawn: u64, complete: bool) -> WalletVolume {
    WalletVolume {
        deposited: bitcoin::Amount::from_sat(deposited),
        withdrawn: bitcoin::Amount::from_sat(withdrawn),
        complete,
    }
}

fn readings(now_ms: i64) -> GuardianReadings {
    GuardianReadings {
        audit: audit(&[
            ("lnv2", -40_000),
            ("mintv2", -9_000_000),
            ("walletv2", 10_000_000),
        ]),
        active_wallets: WindowedFigure {
            value: 75,
            complete: true,
        },
        history: ActiveWalletHistory {
            utc_day: (now_ms / DAY_MS) as u64,
            previous_30d: None,
        },
        lnv2_volume: LightningVolumeStats {
            block_count: 144_000,
            last_1008_blocks: lightning(1_000, 2_000, true),
            last_4320_blocks: lightning(10_000, 20_000, true),
        },
        walletv2_volume: WalletVolumeStats {
            block_count: 144_000,
            last_1008_blocks: wallet(3, 4, true),
            last_4320_blocks: wallet(30, 40, false),
        },
    }
}

/// Each figure reads its own input: the walletv2 audit value rather than the
/// federation's net assets, backups refreshed in 30 days rather than all of
/// them, and on-chain sats converted to msats before they are added to the
/// Lightning msats.
#[test]
fn figures_come_from_their_own_readings() {
    let stats =
        fedimint_stats(&readings(FEDERATION_START_MS + DAY_MS)).expect("the readings are valid");

    assert_eq!(stats.custodied_msats, 10_000_000);
    assert_eq!(
        stats.active_wallets_30d,
        WindowedFigure {
            value: 75,
            complete: true,
        }
    );
    assert_eq!(
        stats.volume_1008_blocks_msats,
        WindowedFigure {
            value: 1_000 + 2_000 + 3_000 + 4_000,
            complete: true,
        }
    );
    // The wallet did not count the whole 30 days, so neither did the figure.
    assert_eq!(
        stats.volume_4320_blocks_msats,
        WindowedFigure {
            value: 10_000 + 20_000 + 30_000 + 40_000,
            complete: false,
        }
    );
}

#[test]
fn custody_needs_exactly_one_non_negative_walletv2_audit() {
    assert!(custodied_msats(&audit(&[("mintv2", -5)])).is_err());
    assert!(custodied_msats(&audit(&[("walletv2", 5), ("walletv2", 6)])).is_err());
    assert!(custodied_msats(&audit(&[("walletv2", -1)])).is_err());
    assert_eq!(custodied_msats(&audit(&[("walletv2", 0)])).ok(), Some(0));
}
