//! The operator's chat with Fedi support
//! ([`SPEC-fman-support-chat`](../../specs/SPEC-fman-support-chat.md)).
//!
//! The thread lives in the fleet database. The Nostr boundary fills it from
//! relays and sends the operator's messages through [`SupportSender`], a
//! capability this crate defines and does not implement.

use serde::Serialize;
use sqlx::Row as _;

use crate::db::Db;
use crate::fleet::Fleet;

/// Longest operator message, in characters.
pub const MAX_SUPPORT_MESSAGE_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportAuthor {
    Operator,
    Fedi,
}

/// One private message of the thread.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SupportMessage {
    /// The NIP-17 rumor id, which every copy of the message shares.
    pub id: String,
    pub author: SupportAuthor,
    pub body: String,
    /// Unix seconds, as the author stated it.
    pub created_at: u64,
}

/// Sends the operator's messages to Fedi support.
#[async_trait::async_trait]
pub trait SupportSender: Send + Sync {
    /// Whether this deployment has a Fedi support identity to talk to.
    fn available(&self) -> bool;

    /// Deliver `body` to Fedi and record it in the thread.
    async fn send(&self, body: String) -> anyhow::Result<SupportMessage>;
}

/// The durable thread.
#[derive(Clone)]
pub struct SupportStore {
    db: Db,
}

impl SupportStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn for_fleet(fleet: &Fleet) -> Self {
        Self::new(fleet.db.clone())
    }

    /// Record a message unless the thread already holds it.
    pub async fn record(&self, message: &SupportMessage) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO support_messages (rumor_id, from_fedi, body, created_at) \
             VALUES (?, ?, ?, ?) ON CONFLICT (rumor_id) DO NOTHING",
        )
        .bind(&message.id)
        .bind(message.author == SupportAuthor::Fedi)
        .bind(&message.body)
        .bind(i64::try_from(message.created_at)?)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// The whole thread, oldest first; a same-second tie keeps arrival order.
    pub async fn messages(&self) -> anyhow::Result<Vec<SupportMessage>> {
        let rows = sqlx::query(
            "SELECT rumor_id, from_fedi, body, created_at FROM support_messages \
             ORDER BY created_at, rowid",
        )
        .fetch_all(self.db.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SupportMessage {
                    id: row.try_get("rumor_id")?,
                    author: if row.try_get("from_fedi")? {
                        SupportAuthor::Fedi
                    } else {
                        SupportAuthor::Operator
                    },
                    body: row.try_get("body")?,
                    created_at: u64::try_from(row.try_get::<i64, _>("created_at")?)?,
                })
            })
            .collect()
    }

    /// Fedi messages the operator has not read.
    pub async fn unread(&self) -> anyhow::Result<u64> {
        let unread: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM support_messages WHERE from_fedi = 1 \
             AND created_at > (SELECT read_until FROM support_chat_state WHERE id = 1)",
        )
        .fetch_one(self.db.pool())
        .await?;
        Ok(u64::try_from(unread)?)
    }

    /// Mark every Fedi message created at or before `up_to` read. Read state
    /// only moves forward, so a stale page cannot unread a newer message.
    pub async fn mark_read(&self, up_to: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE support_chat_state SET read_until = max(read_until, ?) WHERE id = 1")
            .bind(i64::try_from(up_to)?)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }
}
