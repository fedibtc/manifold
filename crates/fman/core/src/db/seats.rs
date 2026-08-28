//! L3 durable seat store: the facts recorded for each guardian seat.
//!
//! A seat row is created once payment is accepted and holds what the seat is:
//! lifecycle facts and port allocation. A paid seat's typed recovery evidence
//! and terminal claim observation live in its claim row;
//! A completion callback is durable delivery work, and a formed-seat row is
//! the immutable fact that this seat has installed its final configuration.
//! Everything else fedimintd owns — setup phase and health — is runtime state
//! on the in-memory seat, rederived by probing, never persisted.
//!
//! SQLite owns durable admission serialization; the in-memory registry is a
//! rebuildable runtime projection. Per-seat ceremony consistency remains
//! owned by each seat task. Uniqueness and set-once constraints make invalid
//! transitions fail loudly instead of upserting.

use super::{Db, DbError, IDENTITY_ID, now_ms};
use crate::facts::{CompletionCallbackReason, CompletionCallbackStatus, SeatFacts, SeatNo};
use crate::identity::RootMnemonic;
use crate::wallet::{ClaimOutcome, EcashClaimEvidence};
use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallback, FederationSize, FiId, InviteCode, OfferEpoch, Plan, QuoteId, SeatId,
};

/// Exhaustive terminal result for a callback delivery attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompletionCallbackOutcome {
    /// The gateway durably accepted the callback.
    Delivered,
    /// The gateway definitively rejected the callback.
    Terminal(CompletionCallbackReason),
}

/// One durable guardian seat and its terminal decommission fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeatRecord {
    pub facts: SeatFacts,
    /// Operator decommission time; terminal once set.
    pub decommissioned_at_ms: Option<i64>,
}

/// Facts recorded atomically at `CreateSeat`. The caller derives the seat id
/// first because the signed response bytes must already contain it.
pub struct NewSeat {
    pub seat_id: SeatId,
    pub fi_id: FiId,
    pub plan: Plan,
    pub federation_size: FederationSize,
    pub payment: Option<NewPayment>,
}

/// Typed recovery material recorded for a paid seat.
/// One seat as a restore reconstructs it: its facts and everything durable
/// that hangs off them.
pub struct RestoredSeat {
    pub facts: SeatFacts,
    pub payment: Option<NewPayment>,
    /// The guardian archive reference the seat's document carried. Presence
    /// means the seat came back holding a guardian config and is formed from
    /// the moment it is restored.
    pub guardian: Option<RestoredGuardianConfig>,
    pub decommissioned_at_ms: Option<i64>,
    /// SHA-256 of the plaintext document as fetched from the relay, seeding
    /// [`seat_backup_publications`]: the relay demonstrably serves exactly
    /// this document, so a restored fleet starts with nothing to republish.
    pub published_doc_sha256: String,
}

/// One seat's last relay-confirmed backup publication
/// (`seat_backup_publications`): what the semi-trusted relay demonstrably
/// serves, against which the backup worker reconciles.
pub(crate) struct SeatBackupPublication {
    /// SHA-256 (hex) of the plaintext document.
    pub(crate) doc_sha256: String,
    pub(crate) published_at_ms: i64,
    /// Digest of the archive whose events were confirmed, once one was.
    pub(crate) archive_digest: Option<String>,
}

/// What a restored seat's document said about its guardian archive.
pub struct RestoredGuardianConfig {
    /// Digest of the archive restore verified and installed.
    pub archive_digest: String,
    /// The formed federation's invite code, recorded in `formed_seats`.
    pub federation_invite: Option<InviteCode>,
}

#[derive(Clone)]
pub struct NewPayment {
    pub evidence: EcashClaimEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentRecord {
    pub seat_id: SeatId,
    pub evidence: EcashClaimEvidence,
    pub outcome: Option<ClaimOutcome>,
    pub outcome_at_ms: Option<i64>,
}

pub(crate) enum SeatAdmissionResult {
    Inserted(SeatFacts),
    Existing(SeatFacts),
    OfferChanged,
}

#[derive(sqlx::FromRow)]
struct SeatRow {
    quote_id: Vec<u8>,
    seat_no: i64,
    fi_id: String,
    plan: String,
    federation_size: i64,
    created_at_ms: i64,
    decommissioned_at_ms: Option<i64>,
}

impl TryFrom<SeatRow> for SeatRecord {
    type Error = DbError;

    fn try_from(row: SeatRow) -> Result<Self, DbError> {
        let key = hex::encode(&row.quote_id);
        let corrupt = |detail: String| DbError::CorruptRow {
            table: "seats",
            key: key.clone(),
            detail,
        };

        let quote_id = QuoteId(
            row.quote_id
                .clone()
                .try_into()
                .map_err(|_| corrupt("quote_id is not 32 bytes".into()))?,
        );
        Ok(SeatRecord {
            decommissioned_at_ms: row.decommissioned_at_ms,
            facts: SeatFacts {
                seat_id: SeatId::from(quote_id),
                seat_no: SeatNo(
                    u32::try_from(row.seat_no)
                        .map_err(|_| corrupt(format!("seat_no {}", row.seat_no)))?,
                ),
                fi_id: FiId(
                    row.fi_id
                        .parse()
                        .map_err(|err| corrupt(format!("fi_id: {err}")))?,
                ),
                plan: serde_json::from_str(&row.plan)
                    .map_err(|err| corrupt(format!("plan: {err}")))?,
                federation_size: FederationSize(
                    u16::try_from(row.federation_size)
                        .map_err(|_| corrupt(format!("federation_size {}", row.federation_size)))?,
                ),
                created_at_ms: row.created_at_ms,
            },
        })
    }
}

#[derive(sqlx::FromRow)]
struct PaymentRow {
    quote_id: Vec<u8>,
    evidence: Vec<u8>,
    claim_outcome: Option<String>,
    claim_outcome_at_ms: Option<i64>,
}

impl TryFrom<PaymentRow> for PaymentRecord {
    type Error = DbError;

    fn try_from(row: PaymentRow) -> Result<Self, DbError> {
        let key = hex::encode(&row.quote_id);
        let corrupt = |detail: String| DbError::CorruptRow {
            table: "ecash_claims",
            key: key.clone(),
            detail,
        };
        let quote_id = QuoteId(
            row.quote_id
                .try_into()
                .map_err(|_| corrupt("quote_id is not 32 bytes".into()))?,
        );
        let outcome = row
            .claim_outcome
            .map(|value| match value.as_str() {
                "success" => Ok(ClaimOutcome::Success),
                "already_spent" => Ok(ClaimOutcome::AlreadySpent),
                _ => Err(corrupt(format!("unknown claim_outcome {value}"))),
            })
            .transpose()?;
        let evidence: EcashClaimEvidence = ciborium::from_reader(row.evidence.as_slice())
            .map_err(|error| corrupt(format!("evidence CBOR: {error}")))?;
        Ok(Self {
            seat_id: SeatId::from(quote_id),
            evidence,
            outcome,
            outcome_at_ms: row.claim_outcome_at_ms,
        })
    }
}

fn claim_evidence_cbor(evidence: &EcashClaimEvidence) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(evidence, &mut bytes).expect("claim evidence serializes as CBOR");
    bytes
}

/// One independently durable completion-hook record. Bearer material remains
/// private to the delivery worker; operator views expose only `status`.
#[derive(Clone)]
pub(crate) struct CompletionCallbackRecord {
    pub(crate) seat_id: SeatId,
    pub(crate) callback: Option<DkgCompletionCallback>,
    pub(crate) status: CompletionCallbackStatus,
}

#[derive(sqlx::FromRow)]
struct CompletionCallbackRow {
    quote_id: Vec<u8>,
    completion_callback: Option<String>,
    completion_callback_status: String,
    completion_callback_attempts: i64,
    completion_callback_next_attempt_at_ms: Option<i64>,
    completion_callback_reason: Option<String>,
    completion_callback_completed_at_ms: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct CompletionCallbackStatusRow {
    completion_callback_status: String,
    completion_callback_attempts: i64,
    completion_callback_next_attempt_at_ms: Option<i64>,
    completion_callback_reason: Option<String>,
    completion_callback_completed_at_ms: Option<i64>,
}

fn parse_completion_callback_status(
    key: &str,
    row: &CompletionCallbackStatusRow,
) -> Result<CompletionCallbackStatus, DbError> {
    let corrupt = |detail: String| DbError::CorruptRow {
        table: "completion_callbacks",
        key: key.to_owned(),
        detail,
    };
    let attempts = u32::try_from(row.completion_callback_attempts).map_err(|_| {
        corrupt(format!(
            "completion_callback_attempts {}",
            row.completion_callback_attempts
        ))
    })?;
    let reason = row
        .completion_callback_reason
        .as_deref()
        .map(|reason| {
            CompletionCallbackReason::from_str(reason)
                .ok_or_else(|| corrupt(format!("unknown completion callback reason {reason}")))
        })
        .transpose()?;
    let status = match row.completion_callback_status.as_str() {
        "not_configured" if attempts == 0 => CompletionCallbackStatus::NotConfigured,
        "pending" => CompletionCallbackStatus::Pending {
            attempts,
            next_attempt_at_ms: row.completion_callback_next_attempt_at_ms.ok_or_else(|| {
                corrupt("pending callback has no next-attempt timestamp".to_owned())
            })?,
            last_reason: reason,
        },
        "operator_blocked" => CompletionCallbackStatus::OperatorBlocked {
            attempts,
            reason: reason
                .ok_or_else(|| corrupt("operator-blocked callback has no reason".to_owned()))?,
        },
        "delivered" => CompletionCallbackStatus::Delivered {
            attempts,
            at_ms: row.completion_callback_completed_at_ms.ok_or_else(|| {
                corrupt("delivered callback has no completion timestamp".to_owned())
            })?,
        },
        "terminal" => CompletionCallbackStatus::Terminal {
            attempts,
            at_ms: row.completion_callback_completed_at_ms.ok_or_else(|| {
                corrupt("terminal callback has no completion timestamp".to_owned())
            })?,
            reason: reason.ok_or_else(|| corrupt("terminal callback has no reason".to_owned()))?,
        },
        value => {
            return Err(corrupt(format!(
                "invalid completion callback state {value}"
            )));
        }
    };
    match &status {
        CompletionCallbackStatus::NotConfigured
            if row.completion_callback_next_attempt_at_ms.is_some()
                || reason.is_some()
                || row.completion_callback_completed_at_ms.is_some() =>
        {
            return Err(corrupt(
                "not-configured callback retained lifecycle fields".to_owned(),
            ));
        }
        CompletionCallbackStatus::Pending { .. }
            if row.completion_callback_completed_at_ms.is_some() =>
        {
            return Err(corrupt(
                "pending callback retained completion fields".to_owned(),
            ));
        }
        CompletionCallbackStatus::OperatorBlocked { .. }
            if row.completion_callback_next_attempt_at_ms.is_some()
                || row.completion_callback_completed_at_ms.is_some() =>
        {
            return Err(corrupt(
                "operator-blocked callback retained retry or completion fields".to_owned(),
            ));
        }
        CompletionCallbackStatus::Delivered { .. }
            if reason.is_some() || row.completion_callback_next_attempt_at_ms.is_some() =>
        {
            return Err(corrupt(
                "delivered callback has contradictory reason or retry timestamp".to_owned(),
            ));
        }
        CompletionCallbackStatus::Terminal { .. }
            if row.completion_callback_next_attempt_at_ms.is_some() =>
        {
            return Err(corrupt(
                "terminal callback retained retry timestamp".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(status)
}

impl TryFrom<CompletionCallbackRow> for CompletionCallbackRecord {
    type Error = DbError;

    fn try_from(row: CompletionCallbackRow) -> Result<Self, Self::Error> {
        let key = hex::encode(&row.quote_id);
        let corrupt = |detail: String| DbError::CorruptRow {
            table: "completion_callbacks",
            key: key.clone(),
            detail,
        };
        let quote_id = QuoteId(
            row.quote_id
                .try_into()
                .map_err(|_| corrupt("quote_id is not 32 bytes".to_owned()))?,
        );
        let callback = row
            .completion_callback
            .map(|json| {
                serde_json::from_str(&json)
                    .map_err(|err| corrupt(format!("completion_callback: {err}")))
            })
            .transpose()?;
        let status_row = CompletionCallbackStatusRow {
            completion_callback_status: row.completion_callback_status,
            completion_callback_attempts: row.completion_callback_attempts,
            completion_callback_next_attempt_at_ms: row.completion_callback_next_attempt_at_ms,
            completion_callback_reason: row.completion_callback_reason,
            completion_callback_completed_at_ms: row.completion_callback_completed_at_ms,
        };
        let status = parse_completion_callback_status(&key, &status_row)?;
        if matches!(status, CompletionCallbackStatus::NotConfigured) && callback.is_some() {
            return Err(corrupt(
                "not-configured callback retained bearer".to_owned(),
            ));
        }
        let resumable = matches!(
            status,
            CompletionCallbackStatus::Pending { .. }
                | CompletionCallbackStatus::OperatorBlocked { .. }
        );
        if resumable && callback.is_none() {
            return Err(corrupt(
                "resumable callback state is missing bearer".to_owned(),
            ));
        }
        if matches!(
            status,
            CompletionCallbackStatus::Delivered { .. } | CompletionCallbackStatus::Terminal { .. }
        ) && callback.is_some()
        {
            return Err(corrupt(
                "completed callback state retained bearer".to_owned(),
            ));
        }
        Ok(Self {
            seat_id: SeatId::from(quote_id),
            callback,
            status,
        })
    }
}

impl Db {
    /// Resolve immutable replay and already-stale requests from a read snapshot,
    /// then make every potentially accepting decision again at SQLite's writer
    /// boundary. An existing seat always wins over a later epoch change.
    pub(crate) async fn admit_seat(
        &self,
        new_seat: NewSeat,
        offer_epoch: OfferEpoch,
        first_port_base: crate::facts::PortBase,
    ) -> Result<SeatAdmissionResult, DbError> {
        let (existing, epoch) = self.admission_snapshot(&new_seat.seat_id).await?;
        if let Some(facts) = existing {
            return Ok(SeatAdmissionResult::Existing(facts));
        }
        if epoch != offer_epoch {
            return Ok(SeatAdmissionResult::OfferChanged);
        }

        self.admit_seat_at_writer_boundary(new_seat, offer_epoch, first_port_base)
            .await
    }

    /// Read one quote's durable outcome and the offer epoch at the same instant.
    async fn admission_snapshot(
        &self,
        seat_id: &SeatId,
    ) -> Result<(Option<SeatFacts>, OfferEpoch), DbError> {
        let mut snapshot = self.pool().begin().await?;
        let existing = sqlx::query_as::<_, SeatRow>(
            "SELECT s.quote_id, s.seat_no, s.fi_id, s.plan, s.federation_size, \
             created_at_ms, decommissioned_at_ms \
             FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE s.quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_optional(&mut *snapshot)
        .await?
        .map(SeatRecord::try_from)
        .transpose()?
        .map(|record| record.facts);
        let epoch = sqlx::query_scalar("SELECT offer_epoch FROM offer_state WHERE id = 1")
            .fetch_one(&mut *snapshot)
            .await?;
        snapshot.commit().await?;
        Ok((existing, super::parse_offer_epoch(epoch)?))
    }

    /// Recheck and resolve a potentially accepting request at the writer boundary.
    async fn admit_seat_at_writer_boundary(
        &self,
        new_seat: NewSeat,
        offer_epoch: OfferEpoch,
        first_port_base: crate::facts::PortBase,
    ) -> Result<SeatAdmissionResult, DbError> {
        let mut tx = self.begin_write().await?;
        let existing = sqlx::query_as::<_, SeatRow>(
            "SELECT s.quote_id, s.seat_no, s.fi_id, s.plan, s.federation_size, \
             created_at_ms, decommissioned_at_ms \
             FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE s.quote_id = ?",
        )
        .bind(new_seat.seat_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .map(SeatRecord::try_from)
        .transpose()?
        .map(|record| record.facts);
        if let Some(facts) = existing {
            tx.commit().await?;
            return Ok(SeatAdmissionResult::Existing(facts));
        }
        let (epoch, max_seats): (Vec<u8>, i64) =
            sqlx::query_as("SELECT offer_epoch, max_seats FROM offer_state WHERE id = 1")
                .fetch_one(&mut *tx)
                .await?;
        let epoch = super::parse_offer_epoch(epoch)?;
        if epoch != offer_epoch {
            tx.commit().await?;
            return Ok(SeatAdmissionResult::OfferChanged);
        }

        let id = new_seat.seat_id.as_bytes().as_slice();
        let (active, next_no): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE d.quote_id IS NULL), \
             COALESCE(MAX(s.seat_no) + 1, 0) \
             FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id)",
        )
        .fetch_one(&mut *tx)
        .await?;
        let slots = super::available_slots(
            u32::try_from(max_seats).map_err(|_| DbError::CorruptRow {
                table: "offer_state",
                key: "1".to_owned(),
                detail: "max_seats is outside the u32 domain".to_owned(),
            })?,
            first_port_base,
            active,
            next_no,
        );
        assert!(slots > 0, "current-epoch live quote must have capacity");

        let created_at_ms = now_ms();
        let plan = serde_json_canonicalizer::to_string(&new_seat.plan)
            .expect("Plan always serializes to canonical JSON");
        sqlx::query(
            "INSERT INTO seats (quote_id, seat_no, fi_id, plan, federation_size, created_at_ms) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(next_no)
        .bind(new_seat.fi_id.0.to_string())
        .bind(plan)
        .bind(i64::from(new_seat.federation_size.0))
        .bind(created_at_ms)
        .execute(&mut *tx)
        .await?;
        if let Some(payment) = &new_seat.payment {
            sqlx::query("INSERT INTO ecash_claims (quote_id, evidence) VALUES (?, ?)")
                .bind(id)
                .bind(claim_evidence_cbor(&payment.evidence))
                .execute(&mut *tx)
                .await?;
        }
        if slots == 1 {
            sqlx::query("UPDATE offer_state SET offer_epoch = ? WHERE id = 1")
                .bind(super::fresh_offer_epoch().as_bytes().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        Ok(SeatAdmissionResult::Inserted(SeatFacts {
            seat_id: new_seat.seat_id,
            seat_no: SeatNo(u32::try_from(next_no).expect("seat_no fits u32")),
            fi_id: new_seat.fi_id,
            plan: new_seat.plan,
            federation_size: new_seat.federation_size,
            created_at_ms,
        }))
    }

    /// All seats, for the startup load that rebuilds the in-memory registry.
    pub async fn list_seats(&self) -> Result<Vec<SeatRecord>, DbError> {
        sqlx::query_as::<_, SeatRow>(
            "SELECT s.quote_id, s.seat_no, s.fi_id, s.plan, s.federation_size, \
             created_at_ms, decommissioned_at_ms \
             FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) ORDER BY created_at_ms, quote_id",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(SeatRecord::try_from)
        .collect()
    }

    pub async fn payments(&self) -> Result<Vec<PaymentRecord>, DbError> {
        sqlx::query_as::<_, PaymentRow>(
            "SELECT p.quote_id, p.evidence, p.claim_outcome, p.claim_outcome_at_ms \
             FROM ecash_claims p JOIN seats s USING (quote_id) ORDER BY s.created_at_ms, p.quote_id",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(PaymentRecord::try_from)
        .collect()
    }

    /// Every accepted ecash claim without a locally recorded terminal result.
    /// The fman-fedimint worker owns reconciliation; this store owns the schema.
    pub async fn pending_ecash_claims(&self) -> Result<Vec<PaymentRecord>, DbError> {
        sqlx::query_as::<_, PaymentRow>(
            "SELECT quote_id, evidence, claim_outcome, claim_outcome_at_ms \
             FROM ecash_claims WHERE claim_outcome IS NULL ORDER BY quote_id",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(PaymentRecord::try_from)
        .collect()
    }

    pub async fn payment(&self, seat_id: &SeatId) -> Result<Option<PaymentRecord>, DbError> {
        sqlx::query_as::<_, PaymentRow>(
            "SELECT p.quote_id, p.evidence, p.claim_outcome, p.claim_outcome_at_ms \
             FROM ecash_claims p WHERE p.quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_optional(self.pool())
        .await?
        .map(PaymentRecord::try_from)
        .transpose()
    }

    pub async fn record_claim_outcome(
        &self,
        seat_id: &SeatId,
        outcome: ClaimOutcome,
    ) -> Result<i64, DbError> {
        let at_ms = now_ms();
        let outcome = match outcome {
            ClaimOutcome::Success => "success",
            ClaimOutcome::AlreadySpent => "already_spent",
        };
        let result = sqlx::query(
            "UPDATE ecash_claims SET claim_outcome = ?, claim_outcome_at_ms = ? \
             WHERE quote_id = ? AND claim_outcome IS NULL",
        )
        .bind(outcome)
        .bind(at_ms)
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        if result.rows_affected() != 1 {
            return Err(DbError::CorruptRow {
                table: "ecash_claims",
                key: seat_id.to_string(),
                detail: format!(
                    "claim-outcome update affected {} rows",
                    result.rows_affected()
                ),
            });
        }
        Ok(at_ms)
    }

    /// Mark a seat decommissioned (terminal, set-once). The ceremony-protected
    /// runtime mirror has already established that it is live; a guarded
    /// update that affects anything but one row is therefore divergence.
    pub async fn decommission_seat(&self, seat_id: &SeatId) -> Result<i64, DbError> {
        let at_ms = now_ms();
        let id = seat_id.as_bytes().as_slice();
        let mut transaction = self.pool().begin().await?;
        let result = sqlx::query(
            "INSERT INTO decommissioned_seats (quote_id, decommissioned_at_ms) VALUES (?, ?)",
        )
        .bind(id)
        .bind(at_ms)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DbError::CorruptRow {
                table: "seats",
                key: seat_id.to_string(),
                detail: format!(
                    "decommission update affected {} rows",
                    result.rows_affected()
                ),
            });
        }
        sqlx::query(
            "UPDATE completion_callbacks SET completion_callback = NULL, \
             completion_callback_status = 'terminal', \
             completion_callback_next_attempt_at_ms = NULL, \
             completion_callback_reason = 'decommissioned', \
             completion_callback_completed_at_ms = ? \
             WHERE quote_id = ? AND completion_callback_status IN ('pending', 'operator_blocked')",
        )
        .bind(at_ms)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(at_ms)
    }

    /// Retain the first optional completion callback chosen for this seat.
    /// Later ceremony sessions do not replace that formation-level notification.
    pub async fn install_completion_callback(
        &self,
        seat_id: &SeatId,
        completion_callback: Option<&DkgCompletionCallback>,
    ) -> Result<(), DbError> {
        let callback_json = completion_callback.map(|callback| {
            serde_json::to_string(callback).expect("a validated callback always serializes")
        });
        let callback_status = if completion_callback.is_some() {
            "pending"
        } else {
            "not_configured"
        };
        let next_attempt_at_ms = completion_callback.map(|_| now_ms());
        let id = seat_id.as_bytes().as_slice();
        sqlx::query(
            "INSERT INTO completion_callbacks (quote_id, completion_callback, \
             completion_callback_status, completion_callback_next_attempt_at_ms) \
             VALUES (?, ?, ?, ?) ON CONFLICT(quote_id) DO NOTHING",
        )
        .bind(id)
        .bind(&callback_json)
        .bind(callback_status)
        .bind(next_attempt_at_ms)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Record the one-way configured/no-wipe latch and return the stored
    /// timestamp (the caller mirrors it in memory, so db and mirror hold
    /// the same value). Idempotent: an already-set timestamp is kept and
    /// returned — the first observation is the fact worth keeping.
    pub async fn record_formed(
        &self,
        seat_id: &SeatId,
        invite: &InviteCode,
    ) -> Result<i64, DbError> {
        let at_ms = now_ms();
        sqlx::query("INSERT OR IGNORE INTO formed_seats (quote_id, federation_invite, formed_at_ms) VALUES (?, ?, ?)")
            .bind(seat_id.as_bytes().as_slice()).bind(&invite.0).bind(at_ms)
            .execute(self.pool()).await?;
        let (stored_invite, stored_at_ms): (String, i64) = sqlx::query_as(
            "SELECT federation_invite, formed_at_ms FROM formed_seats WHERE quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_one(self.pool())
        .await?;
        if stored_invite != invite.0 {
            return Err(DbError::CorruptRow {
                table: "formed_seats",
                key: seat_id.to_string(),
                detail: "formed invite differs from the immutable stored invite".to_owned(),
            });
        }
        Ok(stored_at_ms)
    }

    /// One seat's terminal decommission time, if it has one.
    pub async fn decommissioned_at_ms(&self, seat_id: &SeatId) -> Result<Option<i64>, DbError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT decommissioned_at_ms FROM decommissioned_seats WHERE quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_optional(self.pool())
        .await?)
    }

    /// One seat's last confirmed backup publication under the given envelope
    /// schema version, if any. The version scopes the record because the
    /// version lives outside the hashed plaintext: a record written by
    /// another version describes events this build would itself refuse to
    /// read back, so it does not count as confirmed here.
    pub(crate) async fn backup_publication(
        &self,
        seat_id: &SeatId,
        schema_version: u32,
    ) -> Result<Option<SeatBackupPublication>, DbError> {
        Ok(sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT doc_sha256, published_at_ms, archive_digest \
             FROM seat_backup_publications WHERE quote_id = ? AND schema_version = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .bind(i64::from(schema_version))
        .fetch_optional(self.pool())
        .await?
        .map(
            |(doc_sha256, published_at_ms, archive_digest)| SeatBackupPublication {
                doc_sha256,
                published_at_ms,
                archive_digest,
            },
        ))
    }

    /// Record a confirmed publication. Called by the backup worker alone,
    /// after the relay's read-back confirmation — never before, so a crash
    /// errs toward one redundant republication of an addressable event.
    ///
    /// The archive digest never regresses to absent *within one schema
    /// version*: the archive is immutable, and a later document-only
    /// publication must not forget that its bytes are already confirmed.
    /// Across a version change the digest is taken as given, absent
    /// included — the recorded archive is laid out under rules the new
    /// version's restore would refuse, so they must republish.
    pub(crate) async fn record_backup_publication(
        &self,
        seat_id: &SeatId,
        doc_sha256: &str,
        archive_digest: Option<&str>,
        schema_version: u32,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO seat_backup_publications \
             (quote_id, doc_sha256, published_at_ms, archive_digest, schema_version) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (quote_id) DO UPDATE SET \
             doc_sha256 = excluded.doc_sha256, \
             published_at_ms = excluded.published_at_ms, \
             archive_digest = CASE \
                 WHEN seat_backup_publications.schema_version = excluded.schema_version \
                 THEN COALESCE(excluded.archive_digest, seat_backup_publications.archive_digest) \
                 ELSE excluded.archive_digest END, \
             schema_version = excluded.schema_version",
        )
        .bind(seat_id.as_bytes().as_slice())
        .bind(doc_sha256)
        .bind(now_ms())
        .bind(archive_digest)
        .bind(i64::from(schema_version))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The immutable invite for this formed seat, if it has formed.
    pub(crate) async fn formed_federation_invite(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<InviteCode>, DbError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT federation_invite FROM formed_seats WHERE quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_optional(self.pool())
        .await?
        .map(InviteCode))
    }

    /// Insert one seat exactly as a backup describes it.
    ///
    /// Distinct from [`Self::admit_seat`] in every way that matters, which is
    /// why it is a separate verb rather than a flag: the seat number and
    /// creation time come from the document instead of being allocated, and no
    /// offer epoch moves — a restore is not a sale.
    ///
    /// Only ever runs on a fresh install (SPEC-nostr-backup-restore,
    /// *Restore*), so it may insert without reconciling against anything.
    /// Write a whole recovered fleet, and the identity that owns it, as one
    /// durable step.
    ///
    /// The identity row goes in **last**, inside the same transaction as the
    /// seats. That makes database adoption all-or-nothing: "has an identity"
    /// means "has been onboarded", so an interrupted transaction cannot come
    /// back onboarded with only some seat rows. Archive-directory writes precede
    /// this transaction and can remain after an interrupted install;
    /// `SPEC-nostr-backup-restore` owns that separate retryability deviation.
    ///
    /// A second call fails on the identity's primary key exactly as
    /// [`Db::install_identity`] does, and rolls the seats back with it.
    pub async fn install_restored_fleet(
        &self,
        identity: &RootMnemonic,
        seats: &[RestoredSeat],
        // The envelope schema version the documents were fetched under — one
        // value for the whole fleet, because the restore's unseal accepts
        // exactly one version.
        backup_schema_version: u32,
    ) -> Result<(), DbError> {
        let mut tx = self.begin_write().await?;
        let stage: String = sqlx::query_scalar("SELECT stage FROM onboarding_state WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
        let stage = super::OnboardingStage::parse(&stage)?;
        if stage != super::OnboardingStage::Identity {
            return Err(DbError::WrongOnboardingStage {
                expected: super::OnboardingStage::Identity,
                actual: stage,
            });
        }
        for seat in seats {
            let facts = &seat.facts;
            let plan = serde_json_canonicalizer::to_string(&facts.plan)
                .expect("Plan always serializes to canonical JSON");
            let quote_id = facts.seat_id.as_bytes().as_slice();
            sqlx::query(
                "INSERT INTO seats (quote_id, seat_no, fi_id, plan, federation_size, created_at_ms) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(quote_id)
            .bind(i64::from(facts.seat_no.0))
            .bind(facts.fi_id.0.to_string())
            .bind(plan)
            .bind(i64::from(facts.federation_size.0))
            .bind(facts.created_at_ms)
            .execute(&mut *tx)
            .await?;
            if let Some(guardian) = seat.guardian.as_ref() {
                let invite =
                    guardian
                        .federation_invite
                        .as_ref()
                        .ok_or_else(|| DbError::CorruptRow {
                            table: "backup_guardians",
                            key: facts.seat_id.to_string(),
                            detail: "guardian archive has no formed federation invite".to_owned(),
                        })?;
                sqlx::query("INSERT INTO formed_seats (quote_id, federation_invite, formed_at_ms) VALUES (?, ?, ?)")
                    .bind(quote_id)
                    .bind(&invite.0)
                    .bind(now_ms())
                    .execute(&mut *tx)
                    .await?;
            }
            // The fetched document is by definition what the relay serves, so
            // the publication record starts confirmed and the backup worker
            // has nothing to republish for a freshly restored fleet.
            sqlx::query(
                "INSERT INTO seat_backup_publications \
                 (quote_id, doc_sha256, published_at_ms, archive_digest, schema_version) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(quote_id)
            .bind(seat.published_doc_sha256.as_str())
            .bind(now_ms())
            .bind(
                seat.guardian
                    .as_ref()
                    .map(|guardian| guardian.archive_digest.as_str()),
            )
            .bind(i64::from(backup_schema_version))
            .execute(&mut *tx)
            .await?;
            if let Some(payment) = &seat.payment {
                sqlx::query("INSERT INTO ecash_claims (quote_id, evidence) VALUES (?, ?)")
                    .bind(quote_id)
                    .bind(claim_evidence_cbor(&payment.evidence))
                    .execute(&mut *tx)
                    .await?;
            }
            if let Some(at_ms) = seat.decommissioned_at_ms {
                sqlx::query(
                    "INSERT INTO decommissioned_seats (quote_id, decommissioned_at_ms) VALUES (?, ?)",
                )
                .bind(quote_id)
                .bind(at_ms)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query(
            "INSERT INTO identity (id, mnemonic, wallet_origin, created_at_ms) \
             VALUES (?, ?, 'restored', ?)",
        )
        .bind(IDENTITY_ID)
        .bind(identity.phrase())
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        let stage_update = sqlx::query(
            "UPDATE onboarding_state SET stage = 'holder_authorization', updated_at_ms = ? \
             WHERE id = 1 AND stage = 'identity'",
        )
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        if stage_update.rows_affected() != 1 {
            return Err(DbError::WrongOnboardingStage {
                expected: super::OnboardingStage::Identity,
                actual: stage,
            });
        }
        tx.commit().await?;
        Ok(())
    }

    /// Persist an outbound attempt before network I/O. The Fleet's single
    /// worker prevents concurrent attempts for one callback; the gateway's
    /// FI-supplied idempotency key deduplicates retries across crashes and
    /// guardians.
    pub(crate) async fn record_completion_callback_attempt_started(
        &self,
        seat_id: &SeatId,
        next_attempt_at_ms: i64,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE completion_callbacks SET completion_callback_status = 'pending', \
             completion_callback_attempts = MIN(completion_callback_attempts + 1, 4294967295), \
             completion_callback_next_attempt_at_ms = ?, \
             completion_callback_reason = NULL \
             WHERE quote_id = ? AND completion_callback IS NOT NULL \
             AND completion_callback_status IN ('pending', 'operator_blocked') \
             AND EXISTS (SELECT 1 FROM formed_seats f WHERE f.quote_id = completion_callbacks.quote_id) \
             AND NOT EXISTS (SELECT 1 FROM decommissioned_seats d WHERE d.quote_id = completion_callbacks.quote_id)",
        )
        .bind(next_attempt_at_ms)
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn record_completion_callback_retry_reason(
        &self,
        seat_id: &SeatId,
        reason: CompletionCallbackReason,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE completion_callbacks SET completion_callback_reason = ? \
             WHERE quote_id = ? AND completion_callback_status = 'pending'",
        )
        .bind(reason.as_str())
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn record_completion_callback_operator_blocked(
        &self,
        seat_id: &SeatId,
        reason: CompletionCallbackReason,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE completion_callbacks SET completion_callback_status = 'operator_blocked', \
             completion_callback_next_attempt_at_ms = NULL, completion_callback_reason = ? \
             WHERE quote_id = ? AND completion_callback IS NOT NULL \
             AND completion_callback_status IN ('pending', 'operator_blocked')",
        )
        .bind(reason.as_str())
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn record_completion_callback_completed(
        &self,
        seat_id: &SeatId,
        outcome: CompletionCallbackOutcome,
    ) -> Result<Option<i64>, DbError> {
        let at_ms = now_ms();
        let (status, reason) = match outcome {
            CompletionCallbackOutcome::Delivered => ("delivered", None),
            CompletionCallbackOutcome::Terminal(reason) => ("terminal", Some(reason.as_str())),
        };
        let result = sqlx::query(
            "UPDATE completion_callbacks SET completion_callback = NULL, \
             completion_callback_status = ?, completion_callback_next_attempt_at_ms = NULL, \
             completion_callback_reason = ?, completion_callback_completed_at_ms = ? \
             WHERE quote_id = ? AND completion_callback IS NOT NULL \
             AND completion_callback_status = 'pending'",
        )
        .bind(status)
        .bind(reason)
        .bind(at_ms)
        .bind(seat_id.as_bytes().as_slice())
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 1 {
            return Ok(Some(at_ms));
        }
        Ok(None)
    }

    #[cfg(test)]
    pub(crate) async fn completion_callback(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<CompletionCallbackRecord>, DbError> {
        self.completion_callbacks("WHERE c.quote_id = ?", Some(seat_id))
            .await
            .map(|mut rows| rows.pop())
    }

    /// Operator-safe callback lifecycle projection. This query deliberately
    /// does not select or deserialize the callback bearer.
    pub(crate) async fn completion_callback_status(
        &self,
        seat_id: &SeatId,
    ) -> Result<Option<CompletionCallbackStatus>, DbError> {
        let row = sqlx::query_as::<_, CompletionCallbackStatusRow>(
            "SELECT completion_callback_status, completion_callback_attempts, \
             completion_callback_next_attempt_at_ms, completion_callback_reason, \
             completion_callback_completed_at_ms FROM completion_callbacks \
             WHERE quote_id = ?",
        )
        .bind(seat_id.as_bytes().as_slice())
        .fetch_optional(self.pool())
        .await?;
        row.map(|row| parse_completion_callback_status(&seat_id.to_string(), &row))
            .transpose()
    }

    /// Resumable callbacks whose seats are durably formed and not terminal.
    pub(crate) async fn deliverable_completion_callbacks(
        &self,
    ) -> Result<Vec<CompletionCallbackRecord>, DbError> {
        self.completion_callbacks(
            "JOIN formed_seats f USING (quote_id) \
             LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE d.quote_id IS NULL AND c.completion_callback IS NOT NULL \
             AND c.completion_callback_status IN ('pending', 'operator_blocked')",
            None,
        )
        .await
    }

    /// Validate the complete callback state table before any worker can
    /// perform network I/O.
    pub(crate) async fn validate_completion_callbacks(&self) -> Result<(), DbError> {
        self.completion_callbacks("", None).await.map(drop)
    }

    async fn completion_callbacks(
        &self,
        suffix: &str,
        seat_id: Option<&SeatId>,
    ) -> Result<Vec<CompletionCallbackRecord>, DbError> {
        let sql = format!(
            "SELECT c.quote_id, c.completion_callback, c.completion_callback_status, \
             c.completion_callback_attempts, \
             c.completion_callback_next_attempt_at_ms, c.completion_callback_reason, \
             c.completion_callback_completed_at_ms FROM completion_callbacks c {suffix}"
        );
        let mut query = sqlx::query_as::<_, CompletionCallbackRow>(&sql);
        if let Some(seat_id) = seat_id {
            query = query.bind(seat_id.as_bytes().as_slice());
        }
        query
            .fetch_all(self.pool())
            .await?
            .into_iter()
            .map(CompletionCallbackRecord::try_from)
            .collect()
    }
}

#[cfg(test)]
#[path = "../../tests/db/seats.rs"]
mod tests;
