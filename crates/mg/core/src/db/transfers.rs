//! Transactional transfer admission, generation checks and permanent retirement.
//!
//! The seat loop serializes process effects. These transactions additionally
//! fence restart, renewal and callback choice across crashes. Accepted material
//! is bounded and committed with its acceptance state, not referenced through an
//! uncommitted filesystem path.

use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, InviteCode, SeatId, SeatTransferGrant, SignedRequest, Timestamp,
};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::{Db, DbError, now_ms};
use crate::transfer_package::TransferMaterial;
use crate::transfer_state::{
    IncomingState, IncomingTransfer, OutgoingTransfer, TransferMove, TransferPhase,
};

fn corrupt(seat_id: &SeatId) -> DbError {
    DbError::CorruptRow {
        table: "seat_transfer_in",
        key: seat_id.to_string(),
        detail: "invalid durable transfer record".into(),
    }
}

fn decode(seat_id: &SeatId, bytes: &[u8]) -> Result<IncomingTransfer, DbError> {
    let mut cursor = std::io::Cursor::new(bytes);
    let record: IncomingTransfer =
        ciborium::from_reader(&mut cursor).map_err(|_| corrupt(seat_id))?;
    if cursor.position() != bytes.len() as u64 || record.generation == 0 {
        return Err(corrupt(seat_id));
    }
    if let Some((material, digest)) = record.material() {
        let material = TransferMaterial::decode(material).map_err(|_| corrupt(seat_id))?;
        if material.archive.digest() != digest {
            return Err(corrupt(seat_id));
        }
    }
    Ok(record)
}

async fn incoming(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
) -> Result<Option<IncomingTransfer>, DbError> {
    let row: Option<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT generation, import_complete, record FROM seat_transfer_in WHERE quote_id = ?",
    )
    .bind(seat_id.as_bytes().as_slice())
    .fetch_optional(conn)
    .await?;
    row.map(|(generation, complete, bytes)| {
        let record = decode(seat_id, &bytes)?;
        if generation < 1
            || generation as u64 != record.generation
            || complete != matches!(record.state, IncomingState::Imported { .. })
        {
            return Err(corrupt(seat_id));
        }
        Ok(record)
    })
    .transpose()
}

pub(super) async fn save(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
    record: &IncomingTransfer,
) -> Result<(), DbError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(record, &mut bytes).expect("transfer facts serialize");
    sqlx::query(
        "INSERT INTO seat_transfer_in (quote_id, generation, import_complete, record) VALUES (?, ?, ?, ?) \
         ON CONFLICT(quote_id) DO UPDATE SET generation = excluded.generation, \
         import_complete = excluded.import_complete, record = excluded.record",
    )
    .bind(seat_id.as_bytes().as_slice())
    .bind(i64::try_from(record.generation).map_err(|_| corrupt(seat_id))?)
    .bind(matches!(record.state, IncomingState::Imported { .. }))
    .bind(bytes)
    .execute(conn)
    .await?;
    Ok(())
}

async fn is_retired(conn: &mut SqliteConnection, seat_id: &SeatId) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM decommissioned_seats WHERE quote_id = ?) \
         OR EXISTS(SELECT 1 FROM seat_transfer_out WHERE quote_id = ?)",
    )
    .bind(seat_id.as_bytes().as_slice())
    .bind(seat_id.as_bytes().as_slice())
    .fetch_one(conn)
    .await?)
}

async fn federation_available(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
    binding: &TransferMove,
) -> Result<bool, DbError> {
    let invites: Vec<String> = sqlx::query_scalar(
        "SELECT f.federation_invite FROM formed_seats f \
         WHERE f.quote_id != ? \
         AND NOT EXISTS (SELECT 1 FROM decommissioned_seats d WHERE d.quote_id = f.quote_id) \
         AND NOT EXISTS (SELECT 1 FROM seat_transfer_out t WHERE t.quote_id = f.quote_id AND t.phase != 'stopping')",
    )
    .bind(seat_id.as_bytes().as_slice())
    .fetch_all(&mut *conn)
    .await?;
    for invite in invites {
        let invite: fedimint_core::invite_code::InviteCode =
            invite.parse().map_err(|_| corrupt(seat_id))?;
        if invite.federation_id().to_string() == binding.federation_id.0 {
            return Ok(false);
        }
    }
    let records: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT t.record FROM seat_transfer_in t WHERE t.quote_id != ? \
         AND NOT EXISTS (SELECT 1 FROM decommissioned_seats d WHERE d.quote_id = t.quote_id) \
         AND NOT EXISTS (SELECT 1 FROM seat_transfer_out o WHERE o.quote_id = t.quote_id AND o.phase != 'stopping')",
    )
    .bind(seat_id.as_bytes().as_slice())
    .fetch_all(&mut *conn)
    .await?;
    for bytes in records {
        let record = decode(seat_id, &bytes)?;
        if !matches!(record.state, IncomingState::Expired)
            && record.binding.federation_id == binding.federation_id
        {
            return Ok(false);
        }
    }
    Ok(true)
}

impl Db {
    /// Load the accepted material or polling state; never format its contents.
    pub(crate) async fn incoming_transfer(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<IncomingTransfer>, DbError> {
        incoming(&mut *self.pool().acquire().await?, seat_id).await
    }

    /// Load the source's permanent retirement fence.
    pub(crate) async fn outgoing_transfer(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<OutgoingTransfer>, DbError> {
        let phase: Option<String> =
            sqlx::query_scalar("SELECT phase FROM seat_transfer_out WHERE quote_id = ?")
                .bind(seat_id.as_bytes().as_slice())
                .fetch_optional(self.pool())
                .await?;
        phase
            .map(|phase| {
                Ok(match phase.as_str() {
                    "stopping" => OutgoingTransfer::Stopping,
                    "stopped" => OutgoingTransfer::Stopped,
                    "removing" => OutgoingTransfer::Removing,
                    "removed" => OutgoingTransfer::Removed,
                    _ => return Err(corrupt(seat_id)),
                })
            })
            .transpose()
    }

    /// Source retirement always overrides imported provenance on a later move.
    pub(crate) async fn transfer_phase(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<TransferPhase>, DbError> {
        if let Some(outgoing) = self.outgoing_transfer(seat_id).await? {
            return Ok(Some(TransferPhase::Outgoing(outgoing)));
        }
        Ok(self
            .incoming_transfer(seat_id)
            .await?
            .map(|record| record.phase()))
    }

    /// Admit a verified, locally addressed pull before replying to its FI.
    ///
    /// `None` refuses the lifecycle transition. An exact waiting replay or a
    /// same-move accepted replay does not replace material, generation or callback.
    pub(crate) async fn admit_transfer(
        &self,
        grant: &SignedRequest<SeatTransferGrant>,
        completion_callback: Option<&DkgCompletionCallback>,
    ) -> Result<Option<IncomingTransfer>, DbError> {
        let mut tx = self.begin_write().await?;
        let Ok(verified) = grant.verify(Timestamp((now_ms() / 1000) as u64)) else {
            return Ok(None);
        };
        let seat_id = &verified.destination_seat_id;
        let owner: Option<String> =
            sqlx::query_scalar("SELECT fi_id FROM seats WHERE quote_id = ?")
                .bind(seat_id.as_bytes().as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        if owner.as_deref() != Some(&verified.fa_id.0.to_string())
            || is_retired(&mut tx, seat_id).await?
        {
            return Ok(None);
        }
        let binding = TransferMove::from(&*verified);
        let callback_commitment = completion_callback.map(|callback| {
            let bytes = serde_json_canonicalizer::to_vec(callback)
                .expect("validated completion callback serializes");
            let mut hash = Sha256::new();
            hash.update(b"mg/guardian-transfer/v1/callback\0");
            hash.update(bytes);
            hash.finalize().into()
        });
        let previous = incoming(&mut tx, seat_id).await?;
        if let Some(previous) = &previous {
            if previous.callback_commitment != callback_commitment {
                return Ok(None);
            }
            match &previous.state {
                IncomingState::Waiting { grant: old } if old == grant => {
                    return Ok(Some(previous.clone()));
                }
                IncomingState::Waiting { grant: old }
                    if old.verify(Timestamp((now_ms() / 1000) as u64)).is_ok() =>
                {
                    return Ok(None);
                }
                IncomingState::Imported { .. } | IncomingState::Importing { .. } => {
                    if previous.binding != binding {
                        return Ok(None);
                    }
                    // Whole-fleet recovery retains only the callback commitment,
                    // not its bearer. A matching FI retry may restore the missing
                    // capability, never overwrite an existing choice/delivery.
                    super::seats::install_completion_callback(
                        &mut tx,
                        seat_id,
                        completion_callback,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(Some(previous.clone()));
                }
                _ => {}
            }
        }
        let unavailable: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM formed_seats WHERE quote_id = ?) \
             OR EXISTS(SELECT 1 FROM dkg_inputs WHERE quote_id = ?)",
        )
        .bind(seat_id.as_bytes().as_slice())
        .bind(seat_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if unavailable {
            return Ok(None);
        }
        if !federation_available(&mut tx, seat_id, &binding).await? {
            return Ok(None);
        }
        let generation = previous
            .map_or(Some(1), |old| old.generation.checked_add(1))
            .filter(|generation| *generation <= i64::MAX as u64)
            .ok_or_else(|| corrupt(seat_id))?;
        let record = IncomingTransfer {
            generation,
            binding,
            callback_commitment,
            state: IncomingState::Waiting {
                grant: grant.clone(),
            },
        };
        save(&mut tx, seat_id, &record).await?;
        super::seats::install_completion_callback(&mut tx, seat_id, completion_callback).await?;
        tx.commit().await?;
        Ok(Some(record))
    }

    /// Discard only expired polling authority, retaining the non-spare marker.
    pub(crate) async fn expire_transfer(
        &self,
        seat_id: &SeatId,
        generation: u64,
    ) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let Some(mut record) = incoming(&mut tx, seat_id).await? else {
            return Ok(false);
        };
        let IncomingState::Waiting { grant } = &record.state else {
            return Ok(false);
        };
        if record.generation != generation
            || grant.verify(Timestamp((now_ms() / 1000) as u64)).is_ok()
        {
            return Ok(false);
        }
        record.state = IncomingState::Expired;
        save(&mut tx, seat_id, &record).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Commit native-validated material and its generation before filesystem work.
    ///
    /// Recheck grant validity inside the committing write transaction; a late
    /// response can neither cross expiry nor install into a renewed generation.
    pub(crate) async fn accept_transfer(
        &self,
        seat_id: &SeatId,
        generation: u64,
        binding: &TransferMove,
        material: &TransferMaterial,
        invite_code: &InviteCode,
    ) -> Result<bool, DbError> {
        let bytes = material.encode().map_err(|_| corrupt(seat_id))?;
        let digest = material.archive.digest();
        let mut tx = self.begin_write().await?;
        let Some(mut record) = incoming(&mut tx, seat_id).await? else {
            return Ok(false);
        };
        let IncomingState::Waiting { grant } = &record.state else {
            return Ok(false);
        };
        if record.generation != generation
            || &record.binding != binding
            || is_retired(&mut tx, seat_id).await?
            || grant.verify(Timestamp((now_ms() / 1000) as u64)).is_err()
        {
            return Ok(false);
        }
        record.state = IncomingState::Importing {
            material: bytes,
            archive_digest: digest,
            invite_code: invite_code.clone(),
        };
        save(&mut tx, seat_id, &record).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Mark an already installed, backup-confirmed, configured import complete.
    pub(crate) async fn complete_transfer(
        &self,
        seat_id: &SeatId,
        generation: u64,
    ) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let Some(mut record) = incoming(&mut tx, seat_id).await? else {
            return Ok(false);
        };
        if record.generation != generation || is_retired(&mut tx, seat_id).await? {
            return Ok(false);
        }
        let IncomingState::Importing {
            material,
            archive_digest,
            invite_code,
        } = record.state
        else {
            return Ok(matches!(record.state, IncomingState::Imported { .. }));
        };
        record.state = IncomingState::Imported {
            material,
            archive_digest,
            invite_code,
        };
        save(&mut tx, seat_id, &record).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Commit the no-start source fence before attempting to stop its child.
    /// A later valid grant may export the same guardian; no destination is pinned.
    pub(crate) async fn retire_for_transfer(&self, seat_id: &SeatId) -> Result<bool, DbError> {
        let result = sqlx::query(
            "INSERT INTO seat_transfer_out (quote_id, phase) \
             SELECT ?, 'stopping' WHERE EXISTS(SELECT 1 FROM formed_seats WHERE quote_id = ?) \
             AND NOT EXISTS(SELECT 1 FROM decommissioned_seats WHERE quote_id = ?) \
             AND NOT EXISTS(SELECT 1 FROM seat_transfer_in WHERE quote_id = ? AND import_complete = 0) \
             ON CONFLICT(quote_id) DO NOTHING",
        )
        .bind(seat_id.as_bytes().as_slice())
        .bind(seat_id.as_bytes().as_slice())
        .bind(seat_id.as_bytes().as_slice())
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Advance exactly one source barrier; database triggers forbid regression.
    pub(crate) async fn advance_retirement(
        &self,
        seat_id: &SeatId,
        from: OutgoingTransfer,
        to: OutgoingTransfer,
    ) -> Result<bool, DbError> {
        let name = |state| match state {
            OutgoingTransfer::Stopping => "stopping",
            OutgoingTransfer::Stopped => "stopped",
            OutgoingTransfer::Removing => "removing",
            OutgoingTransfer::Removed => "removed",
        };
        let result =
            sqlx::query("UPDATE seat_transfer_out SET phase = ? WHERE quote_id = ? AND phase = ?")
                .bind(name(to))
                .bind(seat_id.as_bytes().as_slice())
                .bind(name(from))
                .execute(self.pool())
                .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Drop accepted secret copies only after the owner's filesystem deletion
    /// and exact archive-less publication barrier. The outgoing tombstone,
    /// formed invite, accounting and wallets remain permanent.
    pub(crate) async fn finish_transfer_removal(&self, seat_id: &SeatId) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let changed = sqlx::query("UPDATE seat_transfer_out SET phase = 'removed' WHERE quote_id = ? AND phase = 'removing'")
            .bind(seat_id.as_bytes().as_slice()).execute(&mut *tx).await?.rows_affected() == 1;
        if changed {
            sqlx::query("DELETE FROM seat_transfer_in WHERE quote_id = ?")
                .bind(seat_id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests;
