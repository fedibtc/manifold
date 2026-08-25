use super::*;
use nostr_sdk::Keys;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn production_identity_import_persists_reloads_and_rejects_conflicts() -> anyhow::Result<()> {
    let data_dir = test_data_dir("production-identity");
    let sqlite_path = data_dir.join("flip.sqlite");
    let database = Database::connect(&sqlite_path).await?;
    let store_key = SecretStore::generate_hex_key();
    let secret_store =
        SecretStore::load_or_create(data_dir.join("secret-store.key"), Some(&store_key))?;

    let keys = Keys::generate();
    let secret = keys.secret_key().to_secret_hex();
    let imported =
        load_or_import_production_provider_identity(&database, &secret_store, Some(&secret))
            .await?
            .expect("imported identity is returned");

    assert_eq!(imported.provider_pubkey.0, keys.public_key().to_hex());
    assert_eq!(imported.nostr_secret_key_hex, secret);
    assert_provider_identity_row(&database, &imported.provider_pubkey).await?;
    assert_secret_record_is_encrypted(&database, &secret).await?;

    let reopened_database = Database::connect(&sqlite_path).await?;
    let reopened_store =
        SecretStore::load_or_create(data_dir.join("secret-store.key"), Some(&store_key))?;
    let reloaded =
        load_or_import_production_provider_identity(&reopened_database, &reopened_store, None)
            .await?
            .expect("stored identity reloads without import env");
    assert_eq!(reloaded, imported);

    let idempotent = load_or_import_production_provider_identity(
        &reopened_database,
        &reopened_store,
        Some(&secret),
    )
    .await?
    .expect("same imported identity remains valid");
    assert_eq!(idempotent, imported);

    let other_secret = Keys::generate().secret_key().to_secret_hex();
    let error = load_or_import_production_provider_identity(
        &reopened_database,
        &reopened_store,
        Some(&other_secret),
    )
    .await
    .expect_err("conflicting production provider key must fail closed");
    assert!(
        error
            .to_string()
            .contains("does not match existing production provider identity"),
        "unexpected error: {error:#}"
    );

    let after_conflict =
        load_or_import_production_provider_identity(&reopened_database, &reopened_store, None)
            .await?
            .expect("stored identity remains after rejected conflict");
    assert_eq!(after_conflict, imported);

    Ok(())
}

async fn assert_provider_identity_row(
    database: &Database,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<()> {
    let stored_pubkey: String =
        sqlx::query_scalar("SELECT provider_pubkey FROM provider_identity WHERE id = ?")
            .bind(PROVIDER_IDENTITY_ID)
            .fetch_one(database.pool())
            .await?;
    assert_eq!(stored_pubkey, provider_pubkey.0);
    Ok(())
}

async fn assert_secret_record_is_encrypted(
    database: &Database,
    secret: &str,
) -> anyhow::Result<()> {
    let row = sqlx::query("SELECT ciphertext FROM secret_records WHERE name = ?")
        .bind(PROVIDER_NOSTR_SECRET)
        .fetch_one(database.pool())
        .await?;
    let ciphertext: Vec<u8> = row.get("ciphertext");
    assert_ne!(ciphertext, secret.as_bytes());
    assert!(!String::from_utf8_lossy(&ciphertext).contains(secret));
    Ok(())
}

/// Pins the derivation. The node id built from this key is the address FLIP
/// publishes, so changing the construction silently would orphan every
/// advertisement already on a relay: this vector has to be edited
/// deliberately, alongside a migration story.
#[test]
fn iroh_secret_key_derivation_is_pinned() {
    let identity = production_identity_from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .expect("well-formed test secret parses");

    let derived = identity
        .derive_iroh_secret_key()
        .expect("derivation succeeds for a valid identity");

    assert_eq!(
        hex::encode(derived.to_bytes()),
        "da2f10fd7ec05194be7608888d4dcf849ff8ca91f1f9c938fc0fa32aa8e8f634"
    );
}

#[test]
fn iroh_secret_key_is_stable_and_identity_specific() {
    let first = production_identity_from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .expect("first test secret parses");
    let second = production_identity_from_secret(
        "0000000000000000000000000000000000000000000000000000000000000002",
    )
    .expect("second test secret parses");

    // Stability across calls is what makes the advertised endpoint survive
    // a restart; the daemon derives rather than stores this key.
    assert_eq!(
        first.derive_iroh_secret_key().expect("derives").to_bytes(),
        first.derive_iroh_secret_key().expect("derives").to_bytes()
    );
    assert_ne!(
        first.derive_iroh_secret_key().expect("derives").to_bytes(),
        second.derive_iroh_secret_key().expect("derives").to_bytes()
    );
}

#[tokio::test]
async fn installing_provider_identity_is_install_only() -> anyhow::Result<()> {
    let data_dir = test_data_dir("install-identity");
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let secret_store = SecretStore::load_or_create(
        data_dir.join("secret-store.key"),
        Some(&SecretStore::generate_hex_key()),
    )?;

    let keys = Keys::generate();
    let secret = keys.secret_key().to_secret_hex();

    let (identity, installed) =
        install_production_provider_identity(&database, &secret_store, &secret).await?;
    assert!(installed, "the first install reports that it installed");
    assert_eq!(identity.provider_pubkey.0, keys.public_key().to_hex());

    let (again, installed_again) =
        install_production_provider_identity(&database, &secret_store, &secret).await?;
    assert!(
        !installed_again,
        "re-installing the same key is a no-op, not an error"
    );
    assert_eq!(again, identity);

    // Rotation is out of scope, so a conflicting key must be refused
    // rather than quietly orphaning published advertisements.
    let other = Keys::generate().secret_key().to_secret_hex();
    let error = install_production_provider_identity(&database, &secret_store, &other)
        .await
        .expect_err("a different provider key must be refused");
    assert_eq!(
        error.code(),
        fedi_decentralized_service_liquidity_manager::ServiceErrorCode::FailedPrecondition
    );

    let (unchanged, _) =
        install_production_provider_identity(&database, &secret_store, &secret).await?;
    assert_eq!(
        unchanged, identity,
        "the refused key left the installed identity intact"
    );
    Ok(())
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("flip-identity-{name}-{nanos}"))
}
