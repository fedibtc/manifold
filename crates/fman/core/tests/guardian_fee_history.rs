//! What a seat's remittance-account history adds up to, and what a display
//! window is allowed to be.
//!
//! The fault these exist for is arithmetic, not networking: a lifetime total
//! taken from the windowed remittance list stops counting at the window and
//! then shrinks relative to reality with every further payment. So the walk is
//! driven here over a fabricated history, with no federation involved, and the
//! histories are deliberately longer than the display window the dashboard
//! asks for.

use std::ops::Range;
use std::time::SystemTime;

use fedimint_core::{BitcoinHash as _, TransactionId};
use stability_pool_client::common::{
    AccountHistoryItem, AccountHistoryItemKind, BtcBalanceDepositMetadata, CycleInfo, FiatAmount,
};

use super::*;

/// The default the `GuardianFees` verb applies when a consumer states no
/// preference. The histories below are longer than this on purpose.
const DISPLAY_WINDOW: u64 = 20;

/// A history the vault would read out of the module, oldest entry first.
struct FakeHistory {
    items: Vec<AccountHistoryItem>,
}

#[async_trait::async_trait]
impl AccountHistory for FakeHistory {
    async fn count(&self) -> anyhow::Result<u64> {
        Ok(self.items.len() as u64)
    }

    async fn page(&self, range: Range<u64>) -> anyhow::Result<Vec<AccountHistoryItem>> {
        Ok(self.items[range.start as usize..range.end as usize].to_vec())
    }
}

fn key() -> GuardianFeeAccountKey {
    GuardianFeeAccountKey::from_secret_bytes(&[0x11; 32])
}

fn item(msats: u64, kind: AccountHistoryItemKind) -> AccountHistoryItem {
    AccountHistoryItem {
        cycle: CycleInfo {
            idx: 0,
            start_price: FiatAmount(1),
            start_time: SystemTime::UNIX_EPOCH,
        },
        txid: TransactionId::from_slice(&[msats as u8; 32]).expect("32 bytes is a transaction id"),
        deposit_sequence: 0,
        amount: Amount::from_msats(msats),
        kind,
    }
}

/// A remittance a payer made, with paperwork this seat's key can open.
fn remittance(msats: u64) -> AccountHistoryItem {
    let sealed = crate::remittance_metadata::encrypt(
        &key().keypair().public_key(),
        &crate::remittance_metadata::RemittanceMetadata {
            version: 1,
            total_msats: msats * 6,
            breakdown: vec![],
            remitted_at_unix: 1_753_000_000,
        },
    )
    .expect("seal a breakdown");
    item(
        msats,
        AccountHistoryItemKind::DepositToBtcBalance {
            metadata: BtcBalanceDepositMetadata(sealed),
        },
    )
}

/// Something in the account history that is not money a payer sent us.
fn other_entry(msats: u64) -> AccountHistoryItem {
    item(msats, AccountHistoryItemKind::StagedToLocked)
}

/// `count` remittances of 1_000, 2_000, ... msats, oldest first.
fn remittances_of(count: u64) -> FakeHistory {
    FakeHistory {
        items: (1..=count).map(|n| remittance(n * 1_000)).collect(),
    }
}

fn sum_of(remittances: &[Remittance]) -> u64 {
    remittances.iter().map(|entry| entry.amount.msats).sum()
}

/// The headline: 21 remittances is one more than the display window, and the
/// lifetime total has to count the one that falls out of it. 1_000 + 2_000 +
/// ... + 21_000 = 231_000; the newest twenty alone are 230_000.
#[tokio::test]
async fn the_lifetime_total_counts_remittances_older_than_the_display_window() {
    let history = remittances_of(21);

    let total = total_remitted(&history).await.expect("walk the history");

    assert_eq!(total, Amount::from_msats(231_000));

    // Stated separately so a failure says which mistake was made: the windowed
    // sum is the number a consumer gets by totalling `remittances`, and it is
    // short by the oldest remittance.
    let window = recent_remittances(&history, &key(), DISPLAY_WINDOW)
        .await
        .expect("read the window");
    assert_eq!(window.len(), DISPLAY_WINDOW as usize);
    assert_eq!(sum_of(&window), 230_000);
    assert_ne!(total.msats, sum_of(&window));
}

/// Remittances are a subset of history, so the walk must not stop at a page
/// boundary or at the first entry that is not a deposit.
#[tokio::test]
async fn the_lifetime_total_ignores_entries_that_are_not_remittances() {
    let mut items = Vec::new();
    for n in 1..=25_u64 {
        items.push(other_entry(1_000_000));
        items.push(remittance(n * 1_000));
        items.push(other_entry(1_000_000));
    }
    let history = FakeHistory { items };

    let total = total_remitted(&history).await.expect("walk the history");

    assert_eq!(total, Amount::from_msats(325_000));
}

/// More entries than one page holds, so the backward page walk is exercised
/// rather than a single read that happened to cover everything.
#[tokio::test]
async fn the_lifetime_total_spans_every_page_of_a_long_history() {
    let count = HISTORY_PAGE * 3 + 7;
    let history = remittances_of(count);

    let total = total_remitted(&history).await.expect("walk the history");

    assert_eq!(total, Amount::from_msats((1..=count).sum::<u64>() * 1_000));
}

/// The window keeps its contract: newest first, capped at what was asked for.
#[tokio::test]
async fn the_window_is_the_newest_entries_and_stops_at_the_limit() {
    let history = remittances_of(21);

    let window = recent_remittances(&history, &key(), DISPLAY_WINDOW)
        .await
        .expect("read the window");

    assert_eq!(window.len(), DISPLAY_WINDOW as usize);
    assert_eq!(window[0].amount, Amount::from_msats(21_000));
    assert_eq!(window[19].amount, Amount::from_msats(2_000));
    assert!(
        window[0].metadata.is_ok(),
        "this seat's key opens its own paperwork"
    );
}

/// An account nobody has paid yet is zero, not an error.
#[tokio::test]
async fn an_empty_history_totals_nothing() {
    let history = FakeHistory { items: Vec::new() };

    assert_eq!(
        total_remitted(&history).await.expect("walk the history"),
        Amount::ZERO
    );
}

/// Dependency history is untrusted arithmetic input. A wrapped lifetime total
/// would under-report remitted revenue, so an impossible aggregate fails closed.
#[tokio::test]
async fn an_overflowing_lifetime_total_is_rejected() {
    let history = FakeHistory {
        items: vec![
            remittance(1),
            item(
                u64::MAX,
                AccountHistoryItemKind::DepositToBtcBalance {
                    metadata: BtcBalanceDepositMetadata(vec![]),
                },
            ),
        ],
    };

    assert!(total_remitted(&history).await.is_err());
}
