//! The linked operator device (SPEC-guardian-link): one row, replaced by
//! every new link. The hook bearer lives only in `callback` and is never
//! selected into an operator projection.

use super::{Db, DbError, now_ms};
use crate::facts::CompletionCallbackReason;
use fedi_decentralized_service_fleet_manager::{AttentionReason, DkgCompletionCallback, FiId};

const LINK_ID: i64 = 1;

/// Whether the FMan can still invoke the device's hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianLinkDelivery {
    Active,
    /// The gateway rejected the hook for good; the device must renew it.
    Terminal(CompletionCallbackReason),
}

/// The durable link. `callback` is `None` once delivery is terminal.
#[derive(Clone)]
pub struct GuardianLinkRecord {
    pub device_id: FiId,
    pub device_label: String,
    pub callback: Option<DkgCompletionCallback>,
    pub callback_expires_at: u64,
    pub linked_at_ms: i64,
    pub notification_seq: u64,
    pub notified_reasons: Vec<AttentionReason>,
    pub last_notified_at_ms: Option<i64>,
    pub delivery: GuardianLinkDelivery,
}

impl std::fmt::Debug for GuardianLinkRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuardianLinkRecord")
            .field("device_id", &self.device_id)
            .field("device_label", &self.device_label)
            .field("callback", &self.callback.as_ref().map(|_| "<redacted>"))
            .field("callback_expires_at", &self.callback_expires_at)
            .field("linked_at_ms", &self.linked_at_ms)
            .field("notification_seq", &self.notification_seq)
            .field("notified_reasons", &self.notified_reasons)
            .field("last_notified_at_ms", &self.last_notified_at_ms)
            .field("delivery", &self.delivery)
            .finish()
    }
}

#[derive(sqlx::FromRow)]
struct GuardianLinkRow {
    device_id: String,
    device_label: String,
    callback: Option<String>,
    callback_expires_at: i64,
    linked_at_ms: i64,
    notification_seq: i64,
    notified_reasons: String,
    last_notified_at_ms: Option<i64>,
    delivery: String,
    delivery_reason: Option<String>,
}

fn corrupt(detail: impl Into<String>) -> DbError {
    DbError::CorruptRow {
        table: "guardian_link",
        key: LINK_ID.to_string(),
        detail: detail.into(),
    }
}

impl TryFrom<GuardianLinkRow> for GuardianLinkRecord {
    type Error = DbError;

    fn try_from(row: GuardianLinkRow) -> Result<Self, DbError> {
        let device_id = FiId(
            row.device_id
                .parse()
                .map_err(|_| corrupt("device_id is not an x-only key"))?,
        );
        let callback = row
            .callback
            .as_deref()
            .map(serde_json::from_str::<DkgCompletionCallback>)
            .transpose()
            .map_err(|_| corrupt("callback does not decode"))?;
        let notified_reasons = serde_json::from_str::<Vec<AttentionReason>>(&row.notified_reasons)
            .map_err(|_| corrupt("notified_reasons does not decode"))?;
        let delivery = match (row.delivery.as_str(), row.delivery_reason.as_deref()) {
            ("active", None) => GuardianLinkDelivery::Active,
            ("terminal", Some(reason)) => GuardianLinkDelivery::Terminal(
                CompletionCallbackReason::from_str(reason)
                    .ok_or_else(|| corrupt(format!("unknown delivery reason {reason:?}")))?,
            ),
            (delivery, reason) => {
                return Err(corrupt(format!(
                    "delivery {delivery:?} with reason {reason:?} is not a known state"
                )));
            }
        };
        if delivery == GuardianLinkDelivery::Active && callback.is_none() {
            return Err(corrupt("active delivery without a callback"));
        }
        Ok(Self {
            device_id,
            device_label: row.device_label,
            callback,
            callback_expires_at: u64::try_from(row.callback_expires_at)
                .expect("non-negative by schema CHECK"),
            linked_at_ms: row.linked_at_ms,
            notification_seq: u64::try_from(row.notification_seq)
                .expect("non-negative by schema CHECK"),
            notified_reasons,
            last_notified_at_ms: row.last_notified_at_ms,
            delivery,
        })
    }
}

fn reasons_json(reasons: &[AttentionReason]) -> String {
    serde_json::to_string(reasons).expect("reason codes serialize")
}

fn callback_json(callback: &DkgCompletionCallback) -> String {
    serde_json::to_string(callback).expect("a validated callback always serializes")
}

impl Db {
    /// The linked device, if any.
    pub async fn guardian_link(&self) -> Result<Option<GuardianLinkRecord>, DbError> {
        sqlx::query_as::<_, GuardianLinkRow>(
            "SELECT device_id, device_label, callback, callback_expires_at, linked_at_ms, \
             notification_seq, notified_reasons, last_notified_at_ms, delivery, delivery_reason \
             FROM guardian_link WHERE id = ?",
        )
        .bind(LINK_ID)
        .fetch_optional(self.pool())
        .await?
        .map(GuardianLinkRecord::try_from)
        .transpose()
    }

    /// Link a device, replacing whatever device was linked before. Everything
    /// the previous link accumulated is discarded with it.
    pub async fn replace_guardian_link(
        &self,
        device_id: &FiId,
        device_label: &str,
        callback: &DkgCompletionCallback,
        callback_expires_at: u64,
    ) -> Result<i64, DbError> {
        let linked_at_ms = now_ms();
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM guardian_link WHERE id = ?")
            .bind(LINK_ID)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO guardian_link (id, device_id, device_label, callback, \
             callback_expires_at, linked_at_ms) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(LINK_ID)
        .bind(device_id.0.to_string())
        .bind(device_label)
        .bind(callback_json(callback))
        .bind(i64::try_from(callback_expires_at).unwrap_or(i64::MAX))
        .bind(linked_at_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(linked_at_ms)
    }

    /// Replace the linked device's hook and make delivery active again. False
    /// when that device is not the linked one.
    pub async fn renew_guardian_link_callback(
        &self,
        device_id: &FiId,
        callback: &DkgCompletionCallback,
        callback_expires_at: u64,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE guardian_link SET callback = ?, callback_expires_at = ?, \
             delivery = 'active', delivery_reason = NULL WHERE id = ? AND device_id = ?",
        )
        .bind(callback_json(callback))
        .bind(i64::try_from(callback_expires_at).unwrap_or(i64::MAX))
        .bind(LINK_ID)
        .bind(device_id.0.to_string())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Forget the link. With a device, only that device's link; without one,
    /// whatever is linked (the operator's revoke). False when nothing matched.
    pub async fn delete_guardian_link(&self, device_id: Option<&FiId>) -> Result<bool, DbError> {
        let device_id = device_id.map(|id| id.0.to_string());
        let result =
            sqlx::query("DELETE FROM guardian_link WHERE id = ? AND (? IS NULL OR device_id = ?)")
                .bind(LINK_ID)
                .bind(&device_id)
                .bind(&device_id)
                .execute(self.pool())
                .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Record a delivered notification: the sequence the idempotency key was
    /// minted from advances, and these reasons count as told. Guarded on the
    /// expected sequence so a stale worker result cannot double-advance.
    pub async fn record_guardian_link_notified(
        &self,
        expected_seq: u64,
        reasons: &[AttentionReason],
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE guardian_link SET notification_seq = notification_seq + 1, \
             notified_reasons = ?, last_notified_at_ms = ? \
             WHERE id = ? AND notification_seq = ? AND delivery = 'active'",
        )
        .bind(reasons_json(reasons))
        .bind(now_ms())
        .bind(LINK_ID)
        .bind(i64::try_from(expected_seq).unwrap_or(i64::MAX))
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Narrow the told reasons to those that still hold, so a reason that
    /// returns later is told again.
    pub async fn set_guardian_link_notified_reasons(
        &self,
        reasons: &[AttentionReason],
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE guardian_link SET notified_reasons = ? WHERE id = ?")
            .bind(reasons_json(reasons))
            .bind(LINK_ID)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// The gateway rejected the hook for good: clear the bearer and keep the
    /// reason for the operator and the device.
    pub async fn record_guardian_link_terminal(
        &self,
        reason: CompletionCallbackReason,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE guardian_link SET callback = NULL, delivery = 'terminal', \
             delivery_reason = ? WHERE id = ? AND delivery = 'active'",
        )
        .bind(reason.as_str())
        .bind(LINK_ID)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
