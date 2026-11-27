//! SeatGrant state ([SPEC-seat-grant](../../../specs/SPEC-seat-grant.md)):
//! the operator's list of issued grants, and the two sets a presented grant is
//! refused by — revoked nonces and the nonces seats were redeemed from.
//!
//! The list grants nothing. A grant this install never issued (one issued
//! before a restore) is redeemed like any other: its signature is the
//! authority, and only the refusal sets are consulted.

use fedi_decentralized_service_manifold_guardian::{
    QuoteId, SeatGrantLabel, SeatGrantNonce, SeatId,
};

use super::{Db, DbError, now_ms};
use crate::facts::SeatGrantRedemption;

/// An issued grant as stored: the exact signed payload and signature the
/// operator shared, and the grant's state.
pub(crate) struct StoredSeatGrant {
    pub(crate) payload: Vec<u8>,
    pub(crate) signature: [u8; 64],
    pub(crate) state: StoredSeatGrantState,
}

/// Where one issued grant stands. A grant is never both used and revoked:
/// revocation refuses a used grant and admission refuses a revoked one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StoredSeatGrantState {
    Unused,
    Used { seat_id: SeatId },
    Revoked,
}

/// What [`Db::revoke_seat_grant`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeatGrantRevocation {
    /// The grant is revoked now, by this call or an earlier one.
    Revoked,
    /// A seat was already redeemed from it, so there is nothing to revoke.
    AlreadyUsed,
    /// This install never issued it.
    Unknown,
    /// The revocation document already holds as many nonces as it can carry.
    TooMany,
}

#[derive(sqlx::FromRow)]
struct SeatGrantRow {
    grant_nonce: Vec<u8>,
    payload: Vec<u8>,
    signature: Vec<u8>,
    redeemed_by: Option<Vec<u8>>,
    revoked: bool,
}

const SEAT_GRANT_SELECT: &str = "SELECT g.grant_nonce, g.payload, g.signature, \
     r.quote_id AS redeemed_by, v.grant_nonce IS NOT NULL AS revoked \
     FROM seat_grants g \
     LEFT JOIN seat_grant_redemptions r USING (grant_nonce) \
     LEFT JOIN seat_grant_revocations v USING (grant_nonce)";

impl TryFrom<SeatGrantRow> for StoredSeatGrant {
    type Error = DbError;

    fn try_from(row: SeatGrantRow) -> Result<Self, DbError> {
        let corrupt = |detail: &str| DbError::CorruptRow {
            table: "seat_grants",
            key: hex::encode(&row.grant_nonce),
            detail: detail.to_owned(),
        };
        let signature = row
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| corrupt("signature is not 64 bytes"))?;
        let state = match (row.redeemed_by.as_deref(), row.revoked) {
            (None, false) => StoredSeatGrantState::Unused,
            (None, true) => StoredSeatGrantState::Revoked,
            (Some(quote_id), false) => StoredSeatGrantState::Used {
                seat_id: SeatId::from(QuoteId(
                    quote_id
                        .try_into()
                        .map_err(|_| corrupt("redeeming quote_id is not 32 bytes"))?,
                )),
            },
            (Some(_), true) => return Err(corrupt("grant is both redeemed and revoked")),
        };
        Ok(Self {
            payload: row.payload,
            signature,
            state,
        })
    }
}

fn nonce_from_row(table: &'static str, bytes: Vec<u8>) -> Result<SeatGrantNonce, DbError> {
    let key = hex::encode(&bytes);
    bytes
        .try_into()
        .map(SeatGrantNonce::from_bytes)
        .map_err(|_| DbError::CorruptRow {
            table,
            key,
            detail: "grant_nonce is not 32 bytes".to_owned(),
        })
}

/// The grant one seat was redeemed from, if any, on the caller's connection
/// (so recovery facts read it from their one snapshot).
pub(super) async fn seat_grant_redemption(
    conn: &mut sqlx::SqliteConnection,
    seat_id: &SeatId,
) -> Result<Option<SeatGrantRedemption>, DbError> {
    let row: Option<(Vec<u8>, Option<String>)> =
        sqlx::query_as("SELECT grant_nonce, label FROM seat_grant_redemptions WHERE quote_id = ?")
            .bind(seat_id.as_bytes().as_slice())
            .fetch_optional(conn)
            .await?;
    row.map(|(nonce, label)| {
        Ok(SeatGrantRedemption {
            grant_nonce: nonce_from_row("seat_grant_redemptions", nonce)?,
            label: label
                .map(|label| {
                    SeatGrantLabel::new(label).map_err(|err| DbError::CorruptRow {
                        table: "seat_grant_redemptions",
                        key: seat_id.to_string(),
                        detail: err.to_string(),
                    })
                })
                .transpose()?,
        })
    })
    .transpose()
}

/// Whether a presented nonce is refused: used by a seat or revoked. Read on
/// the caller's connection, so admission decides it inside its writer
/// transaction.
pub(super) async fn seat_grant_refused(
    conn: &mut sqlx::SqliteConnection,
    grant_nonce: &SeatGrantNonce,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM seat_grant_redemptions WHERE grant_nonce = ?1) \
         OR EXISTS (SELECT 1 FROM seat_grant_revocations WHERE grant_nonce = ?1)",
    )
    .bind(grant_nonce.as_bytes().as_slice())
    .fetch_one(conn)
    .await?)
}

/// Record that a seat was redeemed from a grant, in the admitting
/// transaction. The primary key refuses a second seat for one nonce even if
/// a caller skipped [`seat_grant_refused`].
pub(super) async fn insert_seat_grant_redemption(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    seat_id: &SeatId,
    redemption: &SeatGrantRedemption,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO seat_grant_redemptions (grant_nonce, quote_id, label) VALUES (?, ?, ?)",
    )
    .bind(redemption.grant_nonce.as_bytes().as_slice())
    .bind(seat_id.as_bytes().as_slice())
    .bind(redemption.label.as_ref().map(SeatGrantLabel::as_str))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Record a restored seat's grant in the restore transaction.
///
/// Two hosts that ran one mnemonic at once may each have redeemed the same
/// grant, and a restore of both must still succeed: the first seat in
/// creation order keeps the redemption, and the nonce is refused either way.
pub(super) async fn restore_seat_grant_redemption(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    seat_id: &SeatId,
    redemption: &SeatGrantRedemption,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO seat_grant_redemptions (grant_nonce, quote_id, label) VALUES (?, ?, ?) \
         ON CONFLICT (grant_nonce) DO NOTHING",
    )
    .bind(redemption.grant_nonce.as_bytes().as_slice())
    .bind(seat_id.as_bytes().as_slice())
    .bind(redemption.label.as_ref().map(SeatGrantLabel::as_str))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert restored revocations in the restore transaction.
pub(super) async fn insert_seat_grant_revocations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    revoked: &[SeatGrantNonce],
) -> Result<(), DbError> {
    for grant_nonce in revoked {
        sqlx::query(
            "INSERT INTO seat_grant_revocations (grant_nonce) VALUES (?) \
             ON CONFLICT (grant_nonce) DO NOTHING",
        )
        .bind(grant_nonce.as_bytes().as_slice())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

impl Db {
    /// Add one freshly signed grant to the operator's list.
    pub(crate) async fn insert_seat_grant(
        &self,
        grant_nonce: &SeatGrantNonce,
        payload: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO seat_grants (grant_nonce, payload, signature) VALUES (?, ?, ?)")
            .bind(grant_nonce.as_bytes().as_slice())
            .bind(payload)
            .bind(signature.as_slice())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Every grant this install issued, with its state, in the order it
    /// issued them. The signed issue time has one-second resolution, so two
    /// grants issued in one second are ordered by their insertion instead.
    pub(crate) async fn seat_grants(&self) -> Result<Vec<StoredSeatGrant>, DbError> {
        sqlx::query_as::<_, SeatGrantRow>(&format!("{SEAT_GRANT_SELECT} ORDER BY g.rowid"))
            .fetch_all(self.pool())
            .await?
            .into_iter()
            .map(StoredSeatGrant::try_from)
            .collect()
    }

    /// One issued grant with its state, if this install issued it.
    pub(crate) async fn seat_grant(
        &self,
        grant_nonce: &SeatGrantNonce,
    ) -> Result<Option<StoredSeatGrant>, DbError> {
        sqlx::query_as::<_, SeatGrantRow>(&format!("{SEAT_GRANT_SELECT} WHERE g.grant_nonce = ?"))
            .bind(grant_nonce.as_bytes().as_slice())
            .fetch_optional(self.pool())
            .await?
            .map(StoredSeatGrant::try_from)
            .transpose()
    }

    /// Revoke one unused issued grant. Decided at the writer boundary, so a
    /// concurrent admission either redeems the grant first, and this call
    /// refuses, or finds it revoked.
    pub(crate) async fn revoke_seat_grant(
        &self,
        grant_nonce: &SeatGrantNonce,
        max_revoked: usize,
    ) -> Result<SeatGrantRevocation, DbError> {
        let nonce = grant_nonce.as_bytes().as_slice();
        let mut tx = self.begin_write().await?;
        let (issued, used, revoked, revocations): (bool, bool, bool, i64) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM seat_grants WHERE grant_nonce = ?1), \
             EXISTS (SELECT 1 FROM seat_grant_redemptions WHERE grant_nonce = ?1), \
             EXISTS (SELECT 1 FROM seat_grant_revocations WHERE grant_nonce = ?1), \
             (SELECT COUNT(*) FROM seat_grant_revocations)",
        )
        .bind(nonce)
        .fetch_one(&mut *tx)
        .await?;
        let full = usize::try_from(revocations).unwrap_or(usize::MAX) >= max_revoked;
        let outcome = match (issued, used, revoked) {
            (false, _, _) => SeatGrantRevocation::Unknown,
            (true, true, _) => SeatGrantRevocation::AlreadyUsed,
            (true, false, true) => SeatGrantRevocation::Revoked,
            (true, false, false) if full => SeatGrantRevocation::TooMany,
            (true, false, false) => {
                sqlx::query("INSERT INTO seat_grant_revocations (grant_nonce) VALUES (?)")
                    .bind(nonce)
                    .execute(&mut *tx)
                    .await?;
                SeatGrantRevocation::Revoked
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    /// Whether a presented nonce is used or revoked, for `GetQuote`'s
    /// preview. `CreateSeat` decides again inside its admission transaction.
    pub(crate) async fn is_seat_grant_refused(
        &self,
        grant_nonce: &SeatGrantNonce,
    ) -> Result<bool, DbError> {
        seat_grant_refused(&mut *self.pool().acquire().await?, grant_nonce).await
    }

    /// The grant one seat was redeemed from, if any.
    pub(crate) async fn seat_grant_redemption(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<SeatGrantRedemption>, DbError> {
        seat_grant_redemption(&mut *self.pool().acquire().await?, seat_id).await
    }

    /// Every revoked nonce, in nonce order: the contents of the fleet-level
    /// revocation backup document.
    pub(crate) async fn revoked_seat_grants(&self) -> Result<Vec<SeatGrantNonce>, DbError> {
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT grant_nonce FROM seat_grant_revocations ORDER BY grant_nonce",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|nonce| nonce_from_row("seat_grant_revocations", nonce))
        .collect()
    }

    /// The SHA-256 of the revocation document last confirmed on the relay
    /// under `schema_version`, if any.
    pub(crate) async fn seat_grant_revocations_publication(
        &self,
        schema_version: u32,
    ) -> Result<Option<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT doc_sha256 FROM seat_grant_revocations_publication \
             WHERE id = 1 AND schema_version = ?",
        )
        .bind(i64::from(schema_version))
        .fetch_optional(self.pool())
        .await?)
    }

    /// Record a confirmed publication of the revocation document. Called by
    /// the backup worker alone, after the relay's read-back.
    pub(crate) async fn record_seat_grant_revocations_publication(
        &self,
        doc_sha256: &str,
        schema_version: u32,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO seat_grant_revocations_publication \
             (id, doc_sha256, schema_version, published_at_ms) VALUES (1, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET doc_sha256 = excluded.doc_sha256, \
             schema_version = excluded.schema_version, \
             published_at_ms = excluded.published_at_ms",
        )
        .bind(doc_sha256)
        .bind(i64::from(schema_version))
        .bind(now_ms())
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
