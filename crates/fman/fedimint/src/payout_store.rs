use fedi_decentralized_service_fleet_manager::{FederationId, SeatId};
use fman_core::db::Db;
use sqlx::FromRow;

use crate::payout_job::{Payout, PayoutJob, PayoutJobOperation, PayoutRequestId, PayoutScope};
use crate::payout_operation_id::PayoutOperationId;

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
    destination: &str,
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
    sqlx::query("INSERT INTO payout_jobs (request_id, scope_kind, federation_id, seat_id, invite_code, destination, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(request_id) DO NOTHING")
        .bind(request_id.as_str()).bind(kind).bind(federation_id).bind(seat_id).bind(invite_code).bind(destination).bind(now_ms()).execute(db.pool()).await?;
    let job = get(db, request_id)
        .await?
        .expect("inserted payout row is readable");
    anyhow::ensure!(
        job.scope == *scope && job.destination == destination,
        "payout request id is already bound to different inputs"
    );
    Ok(job)
}

pub(crate) async fn get(
    db: &Db,
    request_id: &PayoutRequestId,
) -> anyhow::Result<Option<PayoutJob>> {
    let row: Option<Row> = sqlx::query_as("SELECT request_id, scope_kind, federation_id, seat_id, invite_code, destination, operation_id, amount_msat, created_at_ms, committed_at_ms FROM payout_jobs WHERE request_id = ?")
        .bind(request_id.as_str()).fetch_optional(db.pool()).await?;
    row.map(parse).transpose()
}

pub(crate) async fn commit(
    db: &Db,
    request_id: &PayoutRequestId,
    payout: &Payout,
) -> anyhow::Result<PayoutJob> {
    let amount = i64::try_from(payout.amount_msat).map_err(|_| {
        anyhow::anyhow!("payout amount does not fit the durable SQLite representation")
    })?;
    sqlx::query("UPDATE payout_jobs SET operation_id = ?, amount_msat = ?, committed_at_ms = ? WHERE request_id = ? AND operation_id IS NULL")
        .bind(payout.operation_id.as_str()).bind(amount).bind(now_ms()).bind(request_id.as_str()).execute(db.pool()).await?;
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

#[derive(FromRow)]
struct Row {
    request_id: String,
    scope_kind: String,
    federation_id: String,
    seat_id: Option<String>,
    invite_code: Option<String>,
    destination: String,
    operation_id: Option<String>,
    amount_msat: Option<i64>,
    created_at_ms: i64,
    committed_at_ms: Option<i64>,
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
    let operation = match (r.operation_id, r.amount_msat, r.committed_at_ms) {
        (None, None, None) => None,
        (Some(id), Some(amount), Some(at)) => Some(PayoutJobOperation {
            operation_id: PayoutOperationId::parse(&id)?,
            amount_msat: u64::try_from(amount)?,
            committed_at_ms: u64::try_from(at)?,
        }),
        _ => anyhow::bail!("corrupt payout_jobs row {request_id}: partial operation commit"),
    };
    Ok(PayoutJob {
        request_id,
        scope,
        destination: r.destination,
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
    #[tokio::test]
    async fn payout_jobs_pin_inputs_and_committed_operations_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_owned();
        let db = Db::open(&path).await.unwrap();
        let request = PayoutRequestId::parse("caller-request-1").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("payment-fed".into()),
        };
        let pending = create(&db, &request, &scope, "operator@example.com")
            .await
            .unwrap();
        assert!(pending.operation.is_none());
        assert_eq!(
            create(&db, &request, &scope, "operator@example.com")
                .await
                .unwrap(),
            pending
        );
        assert!(
            create(&db, &request, &scope, "other@example.com")
                .await
                .is_err()
        );
        sqlx::query("UPDATE payout_jobs SET destination = ? WHERE request_id = ?")
            .bind("retargeted@example.com")
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects direct payout identity mutation");
        let payout = Payout {
            operation_id: PayoutOperationId::parse(&"ab".repeat(32)).unwrap(),
            amount_msat: 42,
        };
        let committed = commit(&db, &request, &payout).await.unwrap();
        assert_eq!(
            committed.operation.as_ref().unwrap().operation_id,
            payout.operation_id
        );
        assert_eq!(committed.operation.as_ref().unwrap().amount_msat, 42);
        assert_eq!(commit(&db, &request, &payout).await.unwrap(), committed);
        let other = Payout {
            operation_id: PayoutOperationId::parse(&"cd".repeat(32)).unwrap(),
            amount_msat: 43,
        };
        assert!(commit(&db, &request, &other).await.is_err());
        sqlx::query("UPDATE payout_jobs SET operation_id = ? WHERE request_id = ?")
            .bind("ef".repeat(32))
            .bind(request.as_str())
            .execute(db.pool())
            .await
            .expect_err("the schema rejects direct payout operation replacement");

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
        create(&db, &request, &scope, "first@example.com")
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
            create(&db, &request, &scope, "changed@example.com")
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
            "operator@example.com",
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
            "operator@example.com",
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
