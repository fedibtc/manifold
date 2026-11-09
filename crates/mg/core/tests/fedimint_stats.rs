use std::collections::BTreeMap;

use fedimint_core::Amount;
use fedimint_core::module::audit::ModuleSummary;

use super::*;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const FEDERATION_START_UNIX: u64 = 1_790_000_000;
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

fn readings(formed_at_ms: i64, now_ms: i64) -> GuardianReadings {
    GuardianReadings {
        audit: audit(&[
            ("lnv2", -40_000),
            ("mintv2", -9_000_000),
            ("walletv2", 10_000_000),
        ]),
        backups: BackupStatistics {
            num_backups: 900,
            total_size: 1 << 20,
            refreshed_1d: 3,
            refreshed_1w: 20,
            refreshed_1m: 75,
            refreshed_3m: 120,
        },
        lnv2_volume: LightningVolumeStats {
            last_7_days: lightning(1_000, 2_000, true),
            last_30_days: lightning(10_000, 20_000, true),
        },
        walletv2_volume: WalletVolumeStats {
            last_7_days: wallet(3, 4, true),
            last_30_days: wallet(30, 40, false),
        },
        federation_start_unix: FEDERATION_START_UNIX,
        formed_at_ms,
        now_ms,
    }
}

/// Each figure reads its own input: the walletv2 audit value rather than the
/// federation's net assets, backups refreshed in 30 days rather than all of
/// them, and on-chain sats converted to msats before they are added to the
/// Lightning msats.
#[test]
fn figures_come_from_their_own_readings() {
    let stats = fedimint_stats(&readings(FEDERATION_START_MS, FEDERATION_START_MS + DAY_MS))
        .expect("the readings are valid");

    assert_eq!(stats.custodied_msats, 10_000_000);
    assert_eq!(
        stats.active_wallets_30d,
        WindowedFigure {
            value: 75,
            complete: true,
        }
    );
    assert_eq!(
        stats.volume_7d_msats,
        WindowedFigure {
            value: 1_000 + 2_000 + 3_000 + 4_000,
            complete: true,
        }
    );
    // The wallet did not count the whole 30 days, so neither did the figure.
    assert_eq!(
        stats.volume_30d_msats,
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

/// A guardian that held the config from formation has the whole backup
/// history; one that arrived later has only what clients sent it since, until
/// 30 days have passed.
#[test]
fn backup_window_is_complete_from_formation_or_after_thirty_days() {
    let hour_ms = 60 * 60 * 1000;
    let complete =
        |formed_at_ms, now_ms| backup_window_complete(formed_at_ms, FEDERATION_START_UNIX, now_ms);

    // Formed by DKG here: recorded shortly after the first session.
    assert!(complete(
        FEDERATION_START_MS + hour_ms,
        FEDERATION_START_MS + DAY_MS
    ));
    // A clock behind the federation's still counts as present at formation.
    assert!(complete(
        FEDERATION_START_MS - hour_ms,
        FEDERATION_START_MS + DAY_MS
    ));

    // Moved or restored two days after formation.
    let moved_at = FEDERATION_START_MS + 2 * DAY_MS;
    assert!(!complete(moved_at, moved_at + DAY_MS));
    assert!(!complete(moved_at, moved_at + 30 * DAY_MS - 1));
    assert!(complete(moved_at, moved_at + 30 * DAY_MS));

    // Just past the formation tolerance.
    assert!(!complete(
        FEDERATION_START_MS + hour_ms + 1,
        FEDERATION_START_MS + DAY_MS
    ));
}

#[test]
fn moved_guardian_reports_its_active_wallets_as_partial() {
    let moved_at = FEDERATION_START_MS + 10 * DAY_MS;
    let stats = fedimint_stats(&readings(moved_at, moved_at + DAY_MS)).expect("valid readings");

    assert_eq!(
        stats.active_wallets_30d,
        WindowedFigure {
            value: 75,
            complete: false,
        }
    );
    // Volume completeness is the modules' own and does not depend on the move.
    assert!(stats.volume_7d_msats.complete);
}
