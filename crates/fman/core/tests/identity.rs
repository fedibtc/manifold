use super::*;
use fedi_decentralized_service_fleet_manager::QuoteId;

const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[test]
fn derives_golden_v1_secrets_from_bip39_seed() {
    let root = RootMnemonic::parse(TEST_MNEMONIC).unwrap();

    assert_eq!(
        hex::encode(root.derive_service_nostr_secret_key().secret_bytes()),
        "8cc84640c84215bebe31ea99bf0a387783d38a12e43e7f42c6cb31533d277897"
    );
    assert_eq!(
        root.derive_service_nostr_pubkey().to_string(),
        "9384d5bef4d90f491d316a2b786cd1477ea1dde563ae10d42f8e3e270f249b85"
    );
    assert_eq!(
        hex::encode(root.derive_iroh_secret_key().to_bytes()),
        "7a26e5f5702b457f5d03966e6e18b1fc165b1fe7cc457a9fe6ae6bba7527c262"
    );
    assert_eq!(
        hex::encode(root.derive_service_signing_key().secret_bytes()),
        "1db26692c08a6803a1bce8b01264dd77aa6170e0cc1ebe46234ff2989a3420c1"
    );
    assert_eq!(
        root.derive_service_pubkey().to_string(),
        "90d85ac4b88d2400df6735085c24aed5afe5dcdcafc9e820ceef65652347c909"
    );

    // The backup identity is the only way a recovered install finds and
    // decrypts its own Nostr backup documents, so a silent change to any of
    // its three keys — author, coordinate blinding, sealing — strands every
    // existing backup (SPEC-nostr-backup-restore).
    assert_eq!(
        root.derive_nostr_backup_keys().public_key().to_string(),
        "3c2d94cf620249d4fc935429a5a17eef4eccb8e0b5f2f7ff8301e0dc5b7e5196"
    );
    assert_eq!(
        hex::encode(root.derive_nostr_backup_tag_key()),
        "83169f3611c24258d4bc37be6c2d2132be62783b58adb7d9762db354d05da77b"
    );
    assert_eq!(
        hex::encode(root.derive_nostr_backup_encryption_key()),
        "48d25f552b274c4e80f4c714cb16bc56a92606cae0d51eae243b78c7c492ac13"
    );

    // The wallet root feeds fman-fedimint; a silent change here
    // loses funds recoverability, so it is pinned like every other key.
    assert_eq!(
        hex::encode(root.derive_wallet_secret().0),
        "411277a94e2c8377267503acef2b467ab9eec19174450e0538dbf45291bab8b4496e6b6553bb63238d99464c07114164a4a1bbcebb2a5fb14e9c67018b4e1217"
    );

    let seat = root.derive_seat_keys(&SeatId::from(QuoteId([0x0a; 32])));
    assert_eq!(
        hex::encode(seat.iroh_api.to_bytes()),
        "8639a633c1066e7ecf331633c96e995901c5390b90c4082881179fda839ad055"
    );
    assert_eq!(
        hex::encode(seat.iroh_p2p.to_bytes()),
        "84cd6373f21045dc743fde16deb7a79a9dedeb0c9ba3b4377b480db62e81590e"
    );
    assert_eq!(
        seat.api_auth,
        "98baf5a6e856ebb0ddf5fa91d43b599898784dc34285d42fa7e95a86ce33994b"
    );
}

#[test]
fn purpose_labels_are_separated() {
    let root = RootMnemonic::parse(TEST_MNEMONIC).unwrap();

    assert_ne!(
        root.derive_service_nostr_secret_key().secret_bytes(),
        root.derive_iroh_secret_key().to_bytes()
    );
    // Both service keys are secp256k1; this pair colliding is exactly
    // what the two-label decision exists to prevent
    // (ARCH-fleet-manager-identity *two service keys*).
    assert_ne!(
        root.derive_service_signing_key().secret_bytes(),
        root.derive_service_nostr_secret_key().secret_bytes()
    );
    assert_ne!(
        root.derive_service_signing_key().secret_bytes(),
        root.derive_iroh_secret_key().to_bytes()
    );
    let seat_a = root.derive_seat_keys(&SeatId::from(QuoteId([0x0a; 32])));
    let seat_b = root.derive_seat_keys(&SeatId::from(QuoteId([0x0b; 32])));
    assert_ne!(seat_a.iroh_api.to_bytes(), seat_b.iroh_api.to_bytes());
    assert_ne!(seat_a.iroh_api.to_bytes(), seat_a.iroh_p2p.to_bytes());
    assert_ne!(seat_a.api_auth, seat_b.api_auth);
}

/// The guardian-fee clients' root is what makes collected fee ecash
/// recoverable from the mnemonic alone, so it is pinned like the wallet root.
/// It must also stay distinct from that root: a guardian and a payment client
/// of one federation open two databases, and one root across both would derive
/// the same mint note secrets in each.
#[test]
fn guardian_fee_root_is_pinned_and_separate_from_the_wallet() {
    let root = RootMnemonic::parse(TEST_MNEMONIC).unwrap();

    assert_eq!(
        hex::encode(root.derive_guardian_fee_secret().0),
        "ddbe7ec42c66f6a1dc138bc2c8b28d5d49f364a00e4bb6623c089d4f46570a7a3d9c6e203f8eb4b92f8516bdd0e7e9959df4548766a23e5db0c5397caa1eccc9"
    );
    assert_ne!(
        root.derive_guardian_fee_secret().0,
        root.derive_wallet_secret().0
    );
}

/// The remittance account is committed to before the federation exists, so
/// changing this derivation strands money payers were told to send to the old
/// account. Pinned for that reason, and pinned per seat: two seats of one FMan
/// must never share an account, or their shares become indistinguishable.
#[test]
fn guardian_fee_accounts_are_pinned_and_per_seat() {
    let root = RootMnemonic::parse(TEST_MNEMONIC).unwrap();
    let account = |byte: u8| {
        root.derive_guardian_fee_account_key(&SeatId::from(QuoteId([byte; 32])))
            .account()
            .id()
            .to_string()
    };

    assert_eq!(
        account(0x0a),
        "spd1y79galyhhqh3l4kc7eeh58wxmzh5uktzmfg8uvrv68t4t6vadkrszngrqq"
    );
    assert_ne!(account(0x0a), account(0x0b));
}

#[test]
fn telemetry_capability_is_generation_scoped_and_redacted() {
    let root = RootMnemonic::parse(TEST_MNEMONIC).unwrap();
    let capability = root.derive_telemetry_capability(0);

    assert_eq!(
        capability,
        root.derive_telemetry_capability(0),
        "the mnemonic and durable generation recover telemetry access"
    );
    assert_ne!(capability, root.derive_telemetry_capability(1));
    assert_ne!(
        capability.as_bytes(),
        &root.derive_iroh_secret_key().to_bytes()
    );
    assert_eq!(format!("{capability:?}"), "TelemetryCapability([REDACTED])");
}
