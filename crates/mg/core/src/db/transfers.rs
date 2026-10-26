//! Transactional transfer admission, generation checks and permanent retirement.
//!
//! The seat loop serializes process effects. These transactions additionally
//! fence restart and callback choice across crashes. Acceptance is
//! committed only after the guardian files are durably on disk, together with
//! the formed row, and records only the guardian's Iroh secrets.

use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, InviteCode, SeatId, SeatTransferGrant, SignedRequest, Timestamp,
};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::{Db, DbError, now_ms};
use crate::transfer_state::{
    ImportedKeys, IncomingState, IncomingTransfer, OutgoingTransfer, TransferMove, TransferPhase,
};

fn corrupt(seat_id: &SeatId) -> DbError {
    DbError::CorruptRow {
        table: "seat_transfer_in",
        key: seat_id.to_string(),
        detail: "invalid durable transfer record".into(),
    }
}

type IncomingRow = (
    i64,
    String,
    String,
    Option<Vec<u8>>,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

fn decode(seat_id: &SeatId, row: IncomingRow) -> Result<IncomingTransfer, DbError> {
    let (generation, state, binding, callback_commitment, grant, iroh_api_sk, iroh_p2p_sk) = row;
    let generation = u64::try_from(generation)
        .ok()
        .filter(|generation| *generation > 0)
        .ok_or_else(|| corrupt(seat_id))?;
    let binding = serde_json::from_str(&binding).map_err(|_| corrupt(seat_id))?;
    let callback_commitment = callback_commitment
        .map(|bytes| <[u8; 32]>::try_from(bytes).map_err(|_| corrupt(seat_id)))
        .transpose()?;
    let keys = |iroh_api_sk: Vec<u8>, iroh_p2p_sk: Vec<u8>| {
        Ok::<_, DbError>(ImportedKeys::from_bytes(
            &<[u8; 32]>::try_from(iroh_api_sk).map_err(|_| corrupt(seat_id))?,
            &<[u8; 32]>::try_from(iroh_p2p_sk).map_err(|_| corrupt(seat_id))?,
        ))
    };
    let state = match (state.as_str(), grant, iroh_api_sk, iroh_p2p_sk) {
        ("waiting", Some(grant), None, None) => IncomingState::Waiting {
            grant: serde_json::from_str(&grant).map_err(|_| corrupt(seat_id))?,
        },
        ("imported", None, Some(api), Some(p2p)) => IncomingState::Imported(keys(api, p2p)?),
        _ => return Err(corrupt(seat_id)),
    };
    Ok(IncomingTransfer {
        generation,
        binding,
        callback_commitment,
        state,
    })
}

const INCOMING_COLUMNS: &str =
    "generation, state, binding, callback_commitment, grant, iroh_api_sk, iroh_p2p_sk";

pub(super) async fn incoming(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
) -> Result<Option<IncomingTransfer>, DbError> {
    let row: Option<IncomingRow> = sqlx::query_as(&format!(
        "SELECT {INCOMING_COLUMNS} FROM seat_transfer_in WHERE quote_id = ?"
    ))
    .bind(seat_id.as_bytes().as_slice())
    .fetch_optional(conn)
    .await?;
    row.map(|row| decode(seat_id, row)).transpose()
}

pub(super) async fn outgoing(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
) -> Result<Option<OutgoingTransfer>, DbError> {
    let phase: Option<String> =
        sqlx::query_scalar("SELECT phase FROM seat_transfer_out WHERE quote_id = ?")
            .bind(seat_id.as_bytes().as_slice())
            .fetch_optional(&mut *conn)
            .await?;
    phase
        .map(|phase| OutgoingTransfer::from_db_str(&phase).ok_or_else(|| corrupt(seat_id)))
        .transpose()
}

pub(super) async fn save(
    conn: &mut SqliteConnection,
    seat_id: &SeatId,
    record: &IncomingTransfer,
) -> Result<(), DbError> {
    let (state, grant, keys) = match &record.state {
        IncomingState::Waiting { grant } => (
            "waiting",
            Some(serde_json::to_string(grant).expect("signed grant serializes")),
            None,
        ),
        IncomingState::Imported(keys) => ("imported", None, Some(keys)),
    };
    sqlx::query(&format!(
        "INSERT INTO seat_transfer_in (quote_id, {INCOMING_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(quote_id) DO UPDATE SET generation = excluded.generation, \
         state = excluded.state, binding = excluded.binding, \
         callback_commitment = excluded.callback_commitment, grant = excluded.grant, \
         iroh_api_sk = excluded.iroh_api_sk, iroh_p2p_sk = excluded.iroh_p2p_sk"
    ))
    .bind(seat_id.as_bytes().as_slice())
    .bind(i64::try_from(record.generation).map_err(|_| corrupt(seat_id))?)
    .bind(state)
    .bind(serde_json::to_string(&record.binding).expect("transfer binding serializes"))
    .bind(record.callback_commitment.map(|bytes| bytes.to_vec()))
    .bind(grant)
    .bind(keys.map(|keys| keys.iroh_api.to_bytes().to_vec()))
    .bind(keys.map(|keys| keys.iroh_p2p.to_bytes().to_vec()))
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
         AND NOT EXISTS (SELECT 1 FROM seat_transfer_out t WHERE t.quote_id = f.quote_id)",
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
    let records: Vec<IncomingRow> = sqlx::query_as(&format!(
        "SELECT {INCOMING_COLUMNS} FROM seat_transfer_in t WHERE t.quote_id != ? \
         AND NOT EXISTS (SELECT 1 FROM decommissioned_seats d WHERE d.quote_id = t.quote_id) \
         AND NOT EXISTS (SELECT 1 FROM seat_transfer_out o WHERE o.quote_id = t.quote_id)"
    ))
    .bind(seat_id.as_bytes().as_slice())
    .fetch_all(&mut *conn)
    .await?;
    for row in records {
        let record = decode(seat_id, row)?;
        if record.binding.federation_id == binding.federation_id {
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
        outgoing(&mut *self.pool().acquire().await?, seat_id).await
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
    /// `None` refuses the lifecycle transition. A grant for the same move,
    /// also one the FA signed again, is a replay: it does not replace the
    /// stored grant, material, generation or callback.
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
            if previous.binding != binding {
                return Ok(None);
            }
            match &previous.state {
                IncomingState::Waiting { .. } => return Ok(Some(previous.clone())),
                IncomingState::Imported(_) => {
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
        let record = IncomingTransfer {
            generation: 1,
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

    /// Commit acceptance after the validated guardian is durably installed in
    /// the seat data directory, together with the formed row. The seat is
    /// formed from here on; its backup follows like any formed seat's.
    ///
    /// Recheck the generation inside the committing write transaction, so a
    /// late response cannot install into another admission.
    pub(crate) async fn accept_transfer(
        &self,
        seat_id: &SeatId,
        generation: u64,
        binding: &TransferMove,
        keys: &ImportedKeys,
        invite_code: &InviteCode,
    ) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let Some(mut record) = incoming(&mut tx, seat_id).await? else {
            return Ok(false);
        };
        let IncomingState::Waiting { .. } = &record.state else {
            return Ok(false);
        };
        if record.generation != generation
            || &record.binding != binding
            || is_retired(&mut tx, seat_id).await?
        {
            return Ok(false);
        }
        record.state = IncomingState::Imported(keys.clone());
        save(&mut tx, seat_id, &record).await?;
        sqlx::query(
            "INSERT INTO formed_seats (quote_id, federation_invite, formed_at_ms) VALUES (?, ?, ?)",
        )
        .bind(seat_id.as_bytes().as_slice())
        .bind(&invite_code.0)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Commit the no-start source fence once the owner has proved its child's
    /// exit. A later valid grant may export the same guardian; no destination
    /// is pinned.
    pub(crate) async fn retire_for_transfer(&self, seat_id: &SeatId) -> Result<bool, DbError> {
        let result = sqlx::query(
            "INSERT INTO seat_transfer_out (quote_id, phase) \
             SELECT ?, 'stopped' WHERE EXISTS(SELECT 1 FROM formed_seats WHERE quote_id = ?) \
             AND NOT EXISTS(SELECT 1 FROM decommissioned_seats WHERE quote_id = ?) \
             AND NOT EXISTS(SELECT 1 FROM seat_transfer_in WHERE quote_id = ? AND state != 'imported') \
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

    /// Record the deletion of the guardian material and drop the accepted
    /// keys. The outgoing tombstone, formed invite, accounting and wallets
    /// remain permanent; the archive-less backup document follows from the
    /// row on the next scan.
    pub(crate) async fn record_transfer_removal(&self, seat_id: &SeatId) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let changed = sqlx::query("UPDATE seat_transfer_out SET phase = 'removed' WHERE quote_id = ? AND phase = 'stopped'")
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
