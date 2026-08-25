mod test_support;

use std::path::Path;

use anyhow::Context;
use fedi_decentralized_liquidity_manager_daemon::Database;
use fedi_decentralized_service_liquidity_manager::{
    AllocationItemTarget, GatewayId, GatewayName, ItemId, Sats,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use test_support::{ADMIN_TOKEN, DaemonProcess, TestDataDir, TestPorts, wait_for_admin_ready};

const ARCHIVED_TOKEN: &str = "flip-archived-admin-token";
const DISPLACED_TOKEN: &str = "flip-displaced-admin-token";

/// A live restore replaces the complete installation credential boundary, not
/// just business rows. This scenario proves that the token captured by the
/// archive comes back and that the token from the displaced generation does
/// not survive the runtime swap.
#[tokio::test(flavor = "multi_thread")]
async fn live_restore_reinstates_the_archived_admin_credential() -> anyhow::Result<()> {
    let ports = TestPorts::allocate()?;
    let root = TestDataDir::new("flip-e2e-restore-token")?;
    let data_dir = root.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();
    let mut daemon = DaemonProcess::start(&data_dir, &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;

    rotate_token(&client, &admin_url, ADMIN_TOKEN, ARCHIVED_TOKEN).await?;
    let backup = admin_post(
        &client,
        &admin_url,
        ARCHIVED_TOKEN,
        "create_backup",
        &json!({}),
    )
    .await?;
    let archive = backup["archive"]
        .as_str()
        .context("create_backup response omitted archive")?;

    rotate_token(&client, &admin_url, ARCHIVED_TOKEN, DISPLACED_TOKEN).await?;
    assert_eq!(
        admin_status(
            &client,
            &admin_url,
            ARCHIVED_TOKEN,
            "get_health",
            &json!({})
        )
        .await?,
        StatusCode::UNAUTHORIZED,
        "rotation must retire the token stored in the archive before restore"
    );

    let restored = admin_post(
        &client,
        &admin_url,
        DISPLACED_TOKEN,
        "restore_backup",
        &json!({ "archive": archive }),
    )
    .await?;
    assert_eq!(restored["status"], "not_configured");

    wait_for_token(&client, &admin_url, ARCHIVED_TOKEN, &mut daemon).await?;
    assert_eq!(
        admin_status(
            &client,
            &admin_url,
            DISPLACED_TOKEN,
            "get_health",
            &json!({})
        )
        .await?,
        StatusCode::UNAUTHORIZED,
        "the displaced generation's credential must not survive restore"
    );
    assert_eq!(
        admin_status(&client, &admin_url, ADMIN_TOKEN, "get_health", &json!({})).await?,
        StatusCode::UNAUTHORIZED,
        "restoring a rotated token must not resurrect the bootstrap token"
    );

    daemon.stop()?;
    Ok(())
}

async fn rotate_token(
    client: &Client,
    admin_url: &str,
    current_token: &str,
    new_token: &str,
) -> anyhow::Result<()> {
    let response = admin_post(
        client,
        admin_url,
        current_token,
        "rotate_admin_token",
        &json!({ "new_token": new_token }),
    )
    .await?;
    assert_eq!(response["bootstrap_token_accepted"], false);
    Ok(())
}

async fn wait_for_token(
    client: &Client,
    admin_url: &str,
    token: &str,
    daemon: &mut DaemonProcess,
) -> anyhow::Result<()> {
    for _ in 0..100 {
        if let Some(status) = daemon.child.try_wait()? {
            anyhow::bail!("daemon exited during restore: {status}");
        }
        if admin_status(client, admin_url, token, "get_health", &json!({})).await? == StatusCode::OK
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    anyhow::bail!("archived admin token did not become active after restore")
}

async fn admin_post(
    client: &Client,
    admin_url: &str,
    token: &str,
    method: &str,
    body: &Value,
) -> anyhow::Result<Value> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/{method}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .with_context(|| format!("send {method}"))?
        .error_for_status()
        .with_context(|| format!("{method} response"))?
        .json()
        .await?)
}

async fn admin_status(
    client: &Client,
    admin_url: &str,
    token: &str,
    method: &str,
    body: &Value,
) -> anyhow::Result<StatusCode> {
    Ok(client
        .post(format!("{admin_url}/admin/v1/{method}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await?
        .status())
}
/// Exercises the authenticated operator recovery boundary against a real daemon:
/// safe pre-submission work can be retried or cancelled, ambiguous sends stay
/// fenced, reviewed sends require an explicit conclusion, and target-client
/// value can only be abandoned with an audited reason.
#[tokio::test(flavor = "multi_thread")]
async fn operator_remediation_preserves_send_once_guards_and_releases_abandoned_capacity()
-> anyhow::Result<()> {
    let ports = TestPorts::allocate()?;
    let data_dir = TestDataDir::new("flip-e2e-operator-remediation")?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();
    let mut daemon = DaemonProcess::start(data_dir.path(), &ports)?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;
    seed_operator_remediation(data_dir.path()).await?;

    let retried = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "retry_funding_step",
        &json!({
            "federation_id": "federation-retryable",
            "item_id": "item-retryable",
            "operation_id": "operation-retryable"
        }),
    )
    .await?;
    assert_eq!(retried["status"], "accepted");

    let ambiguous_retry = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "retry_funding_step",
        &json!({
            "federation_id": "federation-ambiguous",
            "item_id": "item-ambiguous",
            "operation_id": "operation-ambiguous"
        }),
    )
    .await?;
    assert_eq!(ambiguous_retry["status"], "rejected");
    let ambiguous_cancel = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "cancel_allocation",
        &json!({
            "federation_id": "federation-ambiguous",
            "reason": "the send outcome is still ambiguous"
        }),
    )
    .await?;
    assert_eq!(ambiguous_cancel["status"], "rejected");

    let cancelled = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "cancel_allocation",
        &json!({
            "federation_id": "federation-cancellable",
            "reason": "withdrawn before submission"
        }),
    )
    .await?;
    assert_eq!(cancelled["status"], "accepted");
    assert_eq!(
        cancelled["allocation_status"]["item_statuses"][0]["status"],
        "cancelled"
    );
    let cancelled_again = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "cancel_allocation",
        &json!({ "federation_id": "federation-cancellable", "reason": null }),
    )
    .await?;
    assert_eq!(cancelled_again["status"], "already_applied");

    let invalid_resolution = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "resolve_manual_review",
        &json!({
            "operation_id": "operation-review",
            "resolution": "completed",
            "txid": null,
            "reason": "reconciled externally"
        }),
    )
    .await?;
    assert_eq!(invalid_resolution["status"], "rejected");
    let resolved = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "resolve_manual_review",
        &json!({
            "operation_id": "operation-review",
            "resolution": "failed",
            "txid": null,
            "reason": "external reconciliation found no payment"
        }),
    )
    .await?;
    assert_eq!(resolved["status"], "accepted");
    assert_eq!(resolved["operation"]["status"], "failed");
    // "Conclude a review as completed" is split across two verbs.
    // `resolve_manual_review` requires exact chain evidence — the observer must
    // return the named transaction *and* an output paying this operation's
    // address for its amount — and refuses every case where FLIP cannot obtain
    // it. This daemon has no chain observer that knows
    // `reviewed-completed-txid`, which is the situation reviewed operations
    // usually arise in, so the refusal is the designed behaviour rather than a
    // failure of the scenario.
    let completed_request = json!({
        "operation_id": "operation-review-completed",
        "resolution": "completed",
        "txid": "reviewed-completed-txid",
        "reason": "external reconciliation found the payment"
    });
    let refused = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "resolve_manual_review",
        &completed_request,
    )
    .await?;
    assert_eq!(refused["status"], "rejected");
    let refusal_detail = refused["detail"].as_str().unwrap_or_default();
    assert!(
        refusal_detail.contains("complete_review_without_evidence"),
        "a refusal must name the operator's route through it: {refusal_detail}"
    );

    // That route. It completes on the operator's assertion and records in the
    // audit log that no evidence existed, which is the whole point of splitting
    // them: an unverified completion cannot arrive through the verb that looks
    // verified.
    let completed = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "complete_review_without_evidence",
        &json!({
            "operation_id": "operation-review-completed",
            "txid": "reviewed-completed-txid",
            "reason": "external reconciliation found the payment"
        }),
    )
    .await?;
    assert_eq!(completed["status"], "accepted");

    // The response carries only a status and a detail, so the durable outcome is
    // read back rather than assumed.
    let reviewed = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "get_wallet_operation",
        &json!({ "operation_id": "operation-review-completed" }),
    )
    .await?;
    assert_eq!(reviewed["operation"]["status"], "completed");
    assert_eq!(reviewed["operation"]["txid"], "reviewed-completed-txid");

    // Repeating it is refused rather than reported as already applied: the
    // operation has left manual review, and this verb only ever acts on an
    // operation under review.
    let completed_again = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "complete_review_without_evidence",
        &json!({
            "operation_id": "operation-review-completed",
            "txid": "reviewed-completed-txid",
            "reason": "external reconciliation found the payment"
        }),
    )
    .await?;
    assert_eq!(completed_again["status"], "rejected");
    assert!(
        completed_again["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not under manual review"),
        "{}",
        completed_again["detail"]
    );

    let retry_request = json!({
        "operation_id": "operation-review-retry",
        "resolution": "safe_to_retry",
        "txid": null,
        "reason": "external reconciliation proved no payment was submitted"
    });
    let safe_to_retry = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "resolve_manual_review",
        &retry_request,
    )
    .await?;
    assert_eq!(safe_to_retry["status"], "accepted");
    assert_eq!(safe_to_retry["operation"]["status"], "pending");
    assert!(safe_to_retry["operation"]["txid"].is_null());
    assert!(safe_to_retry["operation"]["submitted_at"].is_null());
    assert!(safe_to_retry["operation"]["failure"].is_null());
    let retry_again = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "resolve_manual_review",
        &retry_request,
    )
    .await?;
    assert_eq!(retry_again["status"], "already_applied");

    let missing_reason = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "abandon_target_client_value",
        &json!({ "federation_id": "federation-abandon", "reason": "   " }),
    )
    .await?;
    assert_eq!(missing_reason["status"], "rejected");
    let abandoned = admin_post(
        &client,
        &admin_url,
        ADMIN_TOKEN,
        "abandon_target_client_value",
        &json!({
            "federation_id": "federation-abandon",
            "reason": "the target pool permanently refuses the deposit"
        }),
    )
    .await?;
    assert_eq!(abandoned["status"], "accepted");
    assert_eq!(abandoned["abandoned_amount"], 10_000);
    assert!(
        abandoned["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("outside FLIP"))
    );

    let database = Database::connect(data_dir.path().join("flip.sqlite")).await?;
    let cancelled_item: String = sqlx::query_scalar(
        "SELECT status FROM allocation_items WHERE item_id = 'item-cancellable'",
    )
    .fetch_one(database.pool())
    .await?;
    let cancelled_operation: String = sqlx::query_scalar(
        "SELECT status FROM wallet_operations WHERE operation_id = 'operation-cancellable'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(cancelled_item, "cancelled");
    assert_eq!(cancelled_operation, "cancelled");
    let completed_watermark: Option<i64> = sqlx::query_scalar(
        "SELECT settled_tick FROM wallet_operations WHERE operation_id = 'operation-review-completed'",
    )
    .fetch_one(database.pool())
    .await?;
    assert!(completed_watermark.is_some());
    let retry_row: (String, Option<String>, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT status, txid, submitted_at, failure_json FROM wallet_operations WHERE operation_id = 'operation-review-retry'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(retry_row, ("pending".to_owned(), None, None, None));
    let abandoned_status: String =
        sqlx::query_scalar("SELECT status FROM allocation_items WHERE item_id = 'item-abandon'")
            .fetch_one(database.pool())
            .await?;
    assert_eq!(abandoned_status, "failed");
    let accepted_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE detail_json LIKE '%\"outcome\":\"accepted\"%'",
    )
    .fetch_one(database.pool())
    .await?;
    assert!(
        accepted_audits >= 4,
        "each accepted operator decision must be audited"
    );
    database.pool().close().await;

    daemon.stop()?;
    Ok(())
}

async fn seed_operator_remediation(data_dir: &Path) -> anyhow::Result<()> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    seed_gateway_allocation(
        &database,
        "federation-retryable",
        "item-retryable",
        "action_required",
        Some(("operation-retryable", "failed")),
    )
    .await?;
    seed_gateway_allocation(
        &database,
        "federation-ambiguous",
        "item-ambiguous",
        "action_required",
        Some(("operation-ambiguous", "in_doubt")),
    )
    .await?;
    seed_gateway_allocation(
        &database,
        "federation-cancellable",
        "item-cancellable",
        "pending",
        Some(("operation-cancellable", "pending")),
    )
    .await?;
    seed_gateway_allocation(
        &database,
        "federation-review",
        "item-review",
        "action_required",
        Some(("operation-review", "manual_review_required")),
    )
    .await?;
    seed_gateway_allocation(
        &database,
        "federation-review-completed",
        "item-review-completed",
        "action_required",
        Some(("operation-review-completed", "manual_review_required")),
    )
    .await?;
    seed_gateway_allocation(
        &database,
        "federation-review-retry",
        "item-review-retry",
        "action_required",
        Some(("operation-review-retry", "manual_review_required")),
    )
    .await?;
    sqlx::query(
        r#"UPDATE wallet_operations SET submitted_at = unixepoch(), failure_json = '{"code":"ambiguous","message":"review required","occurred_at":0}' WHERE operation_id IN ('operation-review-completed', 'operation-review-retry')"#,
    )
    .execute(database.pool())
    .await?;
    seed_stability_abandonment(&database).await?;
    database.pool().close().await;
    Ok(())
}

async fn seed_gateway_allocation(
    database: &Database,
    federation_id: &str,
    item_id: &str,
    item_status: &str,
    operation: Option<(&str, &str)>,
) -> anyhow::Result<()> {
    seed_allocation(database, federation_id, 10_000).await?;
    let item_json = serde_json::to_string(&AllocationItemTarget::Gateway {
        item_id: ItemId(item_id.to_owned()),
        gateway_id: GatewayId("gateway-1".to_owned()),
        gateway_name: GatewayName("Gateway".to_owned()),
        amount: Sats(10_000),
    })?;
    sqlx::query(
        "INSERT INTO allocation_items
         (item_id, federation_id, source_type, status, committed_amount_sats,
          reserved_amount_sats, item_json, created_at, updated_at)
         VALUES (?, ?, 'gateway', ?, 10000, 10000, ?, unixepoch(), unixepoch())",
    )
    .bind(item_id)
    .bind(federation_id)
    .bind(item_status)
    .bind(item_json)
    .execute(database.pool())
    .await?;
    if let Some((operation_id, operation_status)) = operation {
        sqlx::query(
            "INSERT INTO wallet_operations
             (operation_id, operation_type, status, federation_id, item_id, amount_sats,
              created_at, updated_at)
             VALUES (?, 'gateway_funding', ?, ?, ?, 10000, unixepoch(), unixepoch())",
        )
        .bind(operation_id)
        .bind(operation_status)
        .bind(federation_id)
        .bind(item_id)
        .execute(database.pool())
        .await?;
    }
    Ok(())
}

async fn seed_stability_abandonment(database: &Database) -> anyhow::Result<()> {
    seed_allocation(database, "federation-abandon", 10_000).await?;
    let item_json = serde_json::to_string(&AllocationItemTarget::StabilityPool {
        item_id: ItemId("item-abandon".to_owned()),
        amount: Sats(10_000),
    })?;
    sqlx::query(
        "INSERT INTO allocation_items
         (item_id, federation_id, source_type, status, committed_amount_sats,
          reserved_amount_sats, item_json, step_json, created_at, updated_at)
         VALUES ('item-abandon', 'federation-abandon', 'stability_pool', 'action_required',
                 10000, 10000, ?, '{\"peg_in_status\":\"claimed\",\"peg_in_amount\":10000}',
                 unixepoch(), unixepoch())",
    )
    .bind(item_json)
    .execute(database.pool())
    .await?;
    Ok(())
}

async fn seed_allocation(
    database: &Database,
    federation_id: &str,
    amount: i64,
) -> anyhow::Result<()> {
    let mut details_hash = [0u8; 32];
    let bytes = federation_id.as_bytes();
    let length = bytes.len().min(details_hash.len());
    details_hash[..length].copy_from_slice(&bytes[..length]);
    let target = json!({
        "federation_id": federation_id,
        "federation_name": "Federation",
        "invite_code": "invite-code",
        "federation_config_hash": "01020304"
    });
    sqlx::query(
        "INSERT INTO allocations
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash,
          request_json, verification_json, target_json, committed_amount_sats,
          reserved_amount_sats)
         VALUES (?, 'requester', 'provider', 'regtest', ?, '{}', '{}', ?, ?, ?)",
    )
    .bind(federation_id)
    .bind(details_hash.to_vec())
    .bind(target.to_string())
    .bind(amount)
    .bind(amount)
    .execute(database.pool())
    .await?;
    Ok(())
}
