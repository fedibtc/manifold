use fedi_decentralized_service_manifold_guardian::{FederationId, SeatId};
use mg_core::db::Db;
use sqlx::FromRow;

use crate::payout_job::{
    DestinationCap, LightningAttempt, Payout, PayoutJob, PayoutJobOperation, PayoutMethod,
    PayoutRequestId, PayoutScope,
};
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::OutgoingRail;

const COLUMNS: &str = "request_id, scope_kind, federation_id, seat_id, invite_code, method, destination, lightning_rail, invoice, operation_id, amount_msat, cap_maximum_msat, cap_remaining_msat, created_at_ms, committed_at_ms, confirmed_at_ms";

pub(crate) async fn destination(db: &Db) -> anyhow::Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT destination FROM payout_settings WHERE id = 1")
            .fetch_one(db.pool())
            .await?,
    )
}

pub(crate) async fn create(
    db: &Db,
    request_id: &PayoutRequestId,
    scope: &PayoutScope,
    method: &PayoutMethod,
) -> anyhow::Result<PayoutJob> {
    let (kind, federation_id, seat_id, invite_code) = match scope {
        PayoutScope::PaymentFederation { federation_id } => {
            ("payment_federation", federation_id.0.clone(), None, None)
        }
        PayoutScope::GuardianFee {
            federation_id,
            seat_id,
            invite_code,
        } => (
            "guardian_fee",
            federation_id.0.clone(),
            Some(seat_id.to_string()),
            Some(invite_code.to_string()),
        ),
    };
    let (method_kind, destination) = match method {
        PayoutMethod::Lightning { destination } => ("lightning", Some(destination.as_str())),
        PayoutMethod::Ecash => ("ecash", None),
    };
    sqlx::query("INSERT INTO payout_jobs (request_id, scope_kind, federation_id, seat_id, invite_code, method, destination, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(request_id) DO NOTHING")
        .bind(request_id.as_str()).bind(kind).bind(federation_id).bind(seat_id).bind(invite_code).bind(method_kind).bind(destination).bind(now_ms()).execute(db.pool()).await?;
    let job = get(db, request_id)
        .await?
        .expect("inserted payout row is readable");
    anyhow::ensure!(
        job.scope == *scope && job.method == *method,
        "payout request id is already bound to different inputs"
    );
    Ok(job)
}

pub(crate) async fn get(
    db: &Db,
    request_id: &PayoutRequestId,
) -> anyhow::Result<Option<PayoutJob>> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM payout_jobs WHERE request_id = ?"
    ))
    .bind(request_id.as_str())
    .fetch_optional(db.pool())
    .await?;
    row.map(parse).transpose()
}

/// Ecash jobs the operator has not confirmed, oldest first. Reclaimed and
/// already-spent jobs are among them: whether a reclaim exists is the wallet
/// database's fact, not this ledger's.
pub(crate) async fn unconfirmed_ecash(db: &Db) -> anyhow::Result<Vec<PayoutJob>> {
    let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM payout_jobs WHERE method = 'ecash' AND confirmed_at_ms IS NULL ORDER BY created_at_ms, request_id"))
        .fetch_all(db.pool()).await?;
    rows.into_iter().map(parse).collect()
}

/// The newest `limit` jobs of both methods, newest first.
pub(crate) async fn recent(db: &Db, limit: u32) -> anyhow::Result<Vec<PayoutJob>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM payout_jobs ORDER BY created_at_ms DESC, request_id DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    rows.into_iter().map(parse).collect()
}

/// Record the operator's confirmation of a committed ecash payout once.
/// Repeating it keeps the first time.
pub(crate) async fn confirm(db: &Db, request_id: &PayoutRequestId) -> anyhow::Result<PayoutJob> {
    sqlx::query("UPDATE payout_jobs SET confirmed_at_ms = ? WHERE request_id = ? AND method = 'ecash' AND operation_id IS NOT NULL AND confirmed_at_ms IS NULL")
        .bind(now_ms()).bind(request_id.as_str()).execute(db.pool()).await?;
    let job = get(db, request_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payout request {request_id} does not exist"))?;
    anyhow::ensure!(
        job.method == PayoutMethod::Ecash,
        "only an ecash payout can be confirmed"
    );
    anyhow::ensure!(
        job.operation
            .as_ref()
            .is_some_and(|op| op.confirmed_at_ms.is_some()),
        "ecash payout has no issued token to confirm"
    );
    Ok(job)
}

/// Write the Lightning payment about to be paid, replacing an attempt whose
/// payment never started. The caller holds the wallet's payout fence and has
/// found no native operation for the attempt it replaces.
pub(crate) async fn attempt(
    db: &Db,
    request_id: &PayoutRequestId,
    attempt: &LightningAttempt,
) -> anyhow::Result<PayoutJob> {
    let (cap_maximum, cap_remaining) = match attempt.capped {
        Some(cap) => (
            Some(to_sql(cap.maximum_msat)?),
            Some(to_sql(cap.remaining_msat)?),
        ),
        None => (None, None),
    };
    sqlx::query("UPDATE payout_jobs SET lightning_rail = ?, invoice = ?, cap_maximum_msat = ?, cap_remaining_msat = ? WHERE request_id = ? AND operation_id IS NULL")
        .bind(rail_name(attempt.rail)).bind(attempt.invoice.to_string()).bind(cap_maximum).bind(cap_remaining).bind(request_id.as_str()).execute(db.pool()).await?;
    let job = get(db, request_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payout request {request_id} does not exist"))?;
    anyhow::ensure!(
        job.operation.is_none() && job.attempt.as_ref() == Some(attempt),
        "payout request already has a committed operation"
    );
    Ok(job)
}

pub(crate) async fn commit(
    db: &Db,
    request_id: &PayoutRequestId,
    payout: &Payout,
) -> anyhow::Result<PayoutJob> {
    sqlx::query("UPDATE payout_jobs SET operation_id = ?, amount_msat = ?, committed_at_ms = ? WHERE request_id = ? AND operation_id IS NULL")
        .bind(payout.operation_id.as_str()).bind(to_sql(payout.amount_msat)?).bind(now_ms()).bind(request_id.as_str()).execute(db.pool()).await?;
    let job = get(db, request_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payout request {request_id} does not exist"))?;
    anyhow::ensure!(
        job.operation.as_ref().is_some_and(
            |op| op.operation_id == payout.operation_id && op.amount_msat == payout.amount_msat
        ),
        "payout request is already bound to a different native operation"
    );
    Ok(job)
}

fn to_sql(msat: u64) -> anyhow::Result<i64> {
    i64::try_from(msat).map_err(|_| {
        anyhow::anyhow!("payout amount does not fit the durable SQLite representation")
    })
}

fn rail_name(rail: OutgoingRail) -> &'static str {
    match rail {
        OutgoingRail::Lnv1 => "lnv1",
        OutgoingRail::Lnv2 => "lnv2",
    }
}

#[derive(FromRow)]
struct Row {
    request_id: String,
    scope_kind: String,
    federation_id: String,
    seat_id: Option<String>,
    invite_code: Option<String>,
    method: String,
    destination: Option<String>,
    lightning_rail: Option<String>,
    invoice: Option<String>,
    operation_id: Option<String>,
    amount_msat: Option<i64>,
    cap_maximum_msat: Option<i64>,
    cap_remaining_msat: Option<i64>,
    created_at_ms: i64,
    committed_at_ms: Option<i64>,
    confirmed_at_ms: Option<i64>,
}
fn parse(r: Row) -> anyhow::Result<PayoutJob> {
    let request_id = PayoutRequestId::parse(&r.request_id)?;
    let federation_id = FederationId(r.federation_id);
    let scope = match (r.scope_kind.as_str(), r.seat_id, r.invite_code) {
        ("payment_federation", None, None) => PayoutScope::PaymentFederation { federation_id },
        ("guardian_fee", Some(seat), Some(invite)) => PayoutScope::GuardianFee {
            federation_id,
            seat_id: SeatId::new(seat)?,
            invite_code: invite.parse()?,
        },
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: invalid scope shape"),
    };
    let method = match (r.method.as_str(), r.destination) {
        ("lightning", Some(destination)) => PayoutMethod::Lightning { destination },
        ("ecash", None) => PayoutMethod::Ecash,
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: invalid method shape"),
    };
    let confirmed_at_ms = r.confirmed_at_ms.map(u64::try_from).transpose()?;
    let capped = match (r.cap_maximum_msat, r.cap_remaining_msat) {
        (None, None) => None,
        (Some(maximum), Some(remaining)) => Some(DestinationCap {
            maximum_msat: u64::try_from(maximum)?,
            remaining_msat: u64::try_from(remaining)?,
        }),
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: partial destination cap"),
    };
    let attempt = match (r.lightning_rail.as_deref(), r.invoice) {
        (None, None) => None,
        (Some(rail), Some(invoice)) => Some(LightningAttempt {
            rail: match rail {
                "lnv1" => OutgoingRail::Lnv1,
                "lnv2" => OutgoingRail::Lnv2,
                _ => anyhow::bail!("corrupt payout_jobs row {request_id}: unknown rail"),
            },
            invoice: invoice.parse().map_err(|error| {
                anyhow::anyhow!("corrupt payout_jobs row {request_id}: invoice: {error}")
            })?,
            capped,
        }),
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: partial Lightning attempt"),
    };
    let operation = match (r.operation_id, r.amount_msat, r.committed_at_ms) {
        (None, None, None) => {
            anyhow::ensure!(
                (capped.is_none() || attempt.is_some()) && confirmed_at_ms.is_none(),
                "corrupt payout_jobs row {request_id}: operation facts without an operation"
            );
            None
        }
        (Some(id), Some(amount), Some(at)) => Some(PayoutJobOperation {
            operation_id: PayoutOperationId::parse(&id)?,
            amount_msat: u64::try_from(amount)?,
            capped,
            committed_at_ms: u64::try_from(at)?,
            confirmed_at_ms,
        }),
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: partial operation commit"),
    };
    Ok(PayoutJob {
        request_id,
        scope,
        method,
        attempt,
        operation,
        created_at_ms: u64::try_from(r.created_at_ms)?,
    })
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lightning(destination: &str) -> PayoutMethod {
        PayoutMethod::Lightning {
            destination: destination.to_owned(),
        }
    }

    fn native(byte: &str, amount_msat: u64) -> Payout {
        Payout {
            operation_id: PayoutOperationId::parse(&byte.repeat(32)).unwrap(),
            amount_msat,
        }
    }

    fn lightning_attempt(
        rail: OutgoingRail,
        seed: u8,
        capped: Option<DestinationCap>,
    ) -> LightningAttempt {
        use bitcoin::secp256k1::{SECP256K1, SecretKey};
        use bitcoin_hashes::{Hash as _, sha256};
        use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

        LightningAttempt {
            rail,
            invoice: InvoiceBuilder::new(Currency::Regtest)
                .description(String::new())
                .payment_hash(sha256::Hash::hash(&[seed; 32]))
                .current_timestamp()
                .min_final_cltv_expiry_delta(0)
                .payment_secret(PaymentSecret([2; 32]))
                .amount_milli_satoshis(42)
                .build_signed(|message| {
                    SECP256K1
                        .sign_ecdsa_recoverable(message, &SecretKey::from_slice(&[3; 32]).unwrap())
                })
                .unwrap(),
            capped,
        }
    }

    #[tokio::test]
    async fn ecash_jobs_bind_their_method_and_confirm_once_after_commit() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let request = PayoutRequestId::parse("ecash-request").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("fixture-federation".into()),
        };
        let pending = create(&db, &request, &scope, &PayoutMethod::Ecash)
            .await
            .unwrap();
        assert_eq!(pending.method, PayoutMethod::Ecash);
        // The same id cannot become a Lightning payout, or the reverse.
        assert!(
            create(&db, &request, &scope, &lightning("operator@example.com"))
                .await
                .is_err()
        );
        let lightning_request = PayoutRequestId::parse("lightning-request").unwrap();
        create(
            &db,
            &lightning_request,
            &scope,
            &lightning("operator@example.com"),
        )
        .await
        .unwrap();
        assert!(
            create(&db, &lightning_request, &scope, &PayoutMethod::Ecash)
                .await
                .is_err()
        );

        // An ecash payout pays no invoice.
        assert!(
            attempt(
                &db,
                &request,
                &lightning_attempt(OutgoingRail::Lnv2, 1, None)
            )
            .await
            .is_err()
        );
        // Nothing was issued yet, so there is nothing to confirm.
        assert!(confirm(&db, &request).await.is_err());
        assert_eq!(unconfirmed_ecash(&db).await.unwrap(), vec![pending.clone()]);

        let committed = commit(&db, &request, &native("ab", 5_000)).await.unwrap();
        let confirmed = confirm(&db, &request).await.unwrap();
        let at = confirmed.operation.as_ref().unwrap().confirmed_at_ms;
        assert!(at.is_some());
        assert_eq!(
            confirmed.operation.as_ref().unwrap().operation_id,
            committed.operation.unwrap().operation_id
        );
        // Repeating keeps the first confirmation.
        assert_eq!(confirm(&db, &request).await.unwrap(), confirmed);
        assert!(unconfirmed_ecash(&db).await.unwrap().is_empty());
        sqlx::query("UPDATE payout_jobs SET confirmed_at_ms = NULL WHERE request_id = ?")
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects undoing a confirmation");
        sqlx::query("UPDATE payout_jobs SET method = 'lightning' WHERE request_id = ?")
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects changing a job's method");

        // A Lightning job is never confirmed, even once it has an operation.
        commit(&db, &lightning_request, &native("cd", 7_000))
            .await
            .unwrap();
        assert!(confirm(&db, &lightning_request).await.is_err());
        sqlx::query("UPDATE payout_jobs SET confirmed_at_ms = 1 WHERE request_id = ?")
            .bind(lightning_request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema refuses a confirmation on a Lightning job");
        sqlx::query(
            "INSERT INTO payout_jobs (request_id, scope_kind, federation_id, method, destination, created_at_ms) VALUES ('bad', 'payment_federation', 'f', 'ecash', 'operator@example.com', 0)",
        )
        .execute(db.pool())
        .await
        .expect_err("the schema refuses an ecash job with a destination");
    }
    #[tokio::test]
    async fn payout_jobs_pin_inputs_and_committed_operations_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_owned();
        let db = Db::open(&path).await.unwrap();
        let request = PayoutRequestId::parse("caller-request-1").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("payment-fed".into()),
        };
        let pending = create(&db, &request, &scope, &lightning("operator@example.com"))
            .await
            .unwrap();
        assert!(pending.operation.is_none());
        assert_eq!(
            create(&db, &request, &scope, &lightning("operator@example.com"))
                .await
                .unwrap(),
            pending
        );
        assert!(
            create(&db, &request, &scope, &lightning("other@example.com"))
                .await
                .is_err()
        );
        sqlx::query("UPDATE payout_jobs SET destination = ? WHERE request_id = ?")
            .bind("retargeted@example.com")
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects direct payout identity mutation");
        let cap = Some(DestinationCap {
            maximum_msat: 42,
            remaining_msat: 58,
        });
        // A first attempt whose payment never started is replaced.
        attempt(
            &db,
            &request,
            &lightning_attempt(OutgoingRail::Lnv1, 4, None),
        )
        .await
        .unwrap();
        let paid = lightning_attempt(OutgoingRail::Lnv2, 5, cap);
        let attempted = attempt(&db, &request, &paid).await.unwrap();
        assert_eq!(attempted.attempt.as_ref(), Some(&paid));
        assert!(attempted.operation.is_none());
        let payout = Payout {
            operation_id: paid.operation_id(),
            amount_msat: 42,
        };
        let committed = commit(&db, &request, &payout).await.unwrap();
        assert_eq!(
            committed.operation.as_ref().unwrap().operation_id,
            payout.operation_id
        );
        assert_eq!(committed.operation.as_ref().unwrap().amount_msat, 42);
        assert_eq!(committed.operation.as_ref().unwrap().capped, cap);
        assert_eq!(committed.attempt.as_ref(), Some(&paid));
        assert_eq!(commit(&db, &request, &payout).await.unwrap(), committed);
        let other = Payout {
            operation_id: PayoutOperationId::parse(&"cd".repeat(32)).unwrap(),
            amount_msat: 43,
        };
        assert!(commit(&db, &request, &other).await.is_err());
        // Once committed, the attempt is the payment's and stays.
        assert!(
            attempt(
                &db,
                &request,
                &lightning_attempt(OutgoingRail::Lnv2, 6, None)
            )
            .await
            .is_err()
        );
        assert_eq!(get(&db, &request).await.unwrap().unwrap(), committed);
        for column in ["invoice", "lightning_rail"] {
            sqlx::query(&format!(
                "UPDATE payout_jobs SET {column} = NULL WHERE request_id = ?"
            ))
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects replacing a committed attempt");
        }
        sqlx::query("UPDATE payout_jobs SET operation_id = ? WHERE request_id = ?")
            .bind("ef".repeat(32))
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects direct payout operation replacement");
        sqlx::query("UPDATE payout_jobs SET cap_remaining_msat = 0 WHERE request_id = ?")
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects direct payout cap replacement");
        drop(db);
        let reopened = Db::open(&path).await.unwrap();
        assert_eq!(get(&reopened, &request).await.unwrap(), Some(committed));
        sqlx::query("DELETE FROM payout_jobs WHERE request_id = ?")
            .bind(request.as_str())
            .execute(reopened.pool())
            .await
            .expect_err("committed payout jobs remain discoverable");
    }

    #[tokio::test]
    async fn lost_native_response_reconciles_without_rebinding_the_request() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let request = PayoutRequestId::parse("lost-response").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("fixture-federation".into()),
        };
        create(&db, &request, &scope, &lightning("first@example.com"))
            .await
            .unwrap();
        let native = Payout {
            operation_id: PayoutOperationId::parse(&"cd".repeat(32)).unwrap(),
            amount_msat: 1_000,
        };
        let reconciled = commit(&db, &request, &native).await.unwrap();
        assert_eq!(
            reconciled.operation.unwrap().operation_id,
            native.operation_id
        );
        assert!(
            create(&db, &request, &scope, &lightning("changed@example.com"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn concurrent_retries_commit_one_native_operation_identity() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let request = PayoutRequestId::parse("concurrent-request").unwrap();
        create(
            &db,
            &request,
            &PayoutScope::PaymentFederation {
                federation_id: FederationId("fixture-federation".into()),
            },
            &lightning("operator@example.com"),
        )
        .await
        .unwrap();
        let payout = Payout {
            operation_id: PayoutOperationId::parse(&"ef".repeat(32)).unwrap(),
            amount_msat: 2_000,
        };
        let (left, right) = tokio::join!(
            commit(&db, &request, &payout),
            commit(&db, &request, &payout)
        );
        assert_eq!(left.unwrap(), right.unwrap());
    }

    #[tokio::test]
    async fn pending_status_read_does_not_create_an_operation() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let request = PayoutRequestId::parse("status-only").unwrap();
        create(
            &db,
            &request,
            &PayoutScope::PaymentFederation {
                federation_id: FederationId("fixture-federation".into()),
            },
            &lightning("operator@example.com"),
        )
        .await
        .unwrap();
        assert!(
            get(&db, &request)
                .await
                .unwrap()
                .unwrap()
                .operation
                .is_none()
        );
    }
}
