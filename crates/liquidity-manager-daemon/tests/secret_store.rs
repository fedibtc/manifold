use super::*;

#[test]
fn encrypt_decrypt_round_trip() -> anyhow::Result<()> {
    let store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    let record = store.encrypt("gateway.admin_credential", "gateway-secret")?;
    let decrypted = store.decrypt("gateway.admin_credential", &record)?;

    assert_eq!(decrypted, "gateway-secret");
    assert_ne!(record.ciphertext, b"gateway-secret");
    Ok(())
}

#[test]
fn encryption_uses_fresh_nonce() -> anyhow::Result<()> {
    let store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;

    let first = store.encrypt("gateway.admin_credential", "gateway-secret")?;
    let second = store.encrypt("gateway.admin_credential", "gateway-secret")?;

    assert_ne!(first.nonce, second.nonce);
    assert_ne!(first.ciphertext, second.ciphertext);
    Ok(())
}

#[test]
fn decrypt_fails_with_wrong_name_or_key() -> anyhow::Result<()> {
    let store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    let wrong_store = SecretStore::from_hex_key(&SecretStore::generate_hex_key())?;
    let record = store.encrypt("gateway.admin_credential", "gateway-secret")?;

    assert!(
        store
            .decrypt("chain_observer.bitcoind.password", &record)
            .is_err()
    );
    assert!(
        wrong_store
            .decrypt("gateway.admin_credential", &record)
            .is_err()
    );
    Ok(())
}
