use fedi_decentralized_service_liquidity_manager::ServiceErrorCode;

use super::*;
use crate::test_support::test_sqlite_path;

#[tokio::test]
async fn no_rotation_means_no_stored_token() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("admin-token-absent")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    // `None` is what keeps the boot bootstrap token acceptable, so a fresh
    // deployment is not locked out of its own Admin API.
    assert_eq!(load(&database, &secret_store).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_rotated_token_is_stored_encrypted_and_reloads() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("admin-token-rotate")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    let token = "rotated-admin-token-value";

    rotate(&database, &secret_store, token).await?;
    assert_eq!(
        load(&database, &secret_store).await?.as_deref(),
        Some(token)
    );

    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM secret_records WHERE name = ?")
            .bind(ADMIN_TOKEN_SECRET)
            .fetch_one(database.pool())
            .await?;
    assert!(
        !String::from_utf8_lossy(&ciphertext).contains(token),
        "the admin token must not be recoverable from the row"
    );

    // Rotating again replaces rather than accumulates.
    let replacement = "second-rotated-admin-token";
    rotate(&database, &secret_store, replacement).await?;
    assert_eq!(
        load(&database, &secret_store).await?.as_deref(),
        Some(replacement)
    );
    Ok(())
}

#[tokio::test]
async fn rotation_refuses_tokens_that_would_lock_the_operator_out() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("admin-token-invalid")).await?;
    let secret_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    for candidate in ["short", "  padded-admin-token-value  "] {
        let error = rotate(&database, &secret_store, candidate)
            .await
            .expect_err("weak or whitespace-padded tokens are refused");
        assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    }

    assert_eq!(
        load(&database, &secret_store).await?,
        None,
        "a refused rotation must leave the bootstrap token in force"
    );
    Ok(())
}
