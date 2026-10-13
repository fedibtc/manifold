use bitcoin::secp256k1::{SECP256K1, SecretKey};

use super::*;

fn metadata() -> RemittanceMetadata {
    RemittanceMetadata {
        version: 1,
        total_msats: 123_456,
        breakdown: vec![
            RemittanceBreakdownItem {
                module: "lightning".to_owned(),
                direction: "send".to_owned(),
                amount_msats: 100_000,
            },
            RemittanceBreakdownItem {
                module: "stabilityPoolV2".to_owned(),
                direction: "receive".to_owned(),
                amount_msats: 23_456,
            },
        ],
        remitted_at_unix: 1_753_000_000,
    }
}

fn account_key(byte: u8) -> secp256k1::Keypair {
    secp256k1::Keypair::from_secret_key(
        SECP256K1,
        &SecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
    )
}

#[test]
fn recipient_reads_what_the_payer_sealed() {
    let key = account_key(0x11);
    let sealed = encrypt(&key.public_key(), &metadata()).expect("seal");
    assert_eq!(decrypt(&key, &sealed).expect("open"), metadata());
}

#[test]
fn another_key_cannot_read_it() {
    let sealed = encrypt(&account_key(0x11).public_key(), &metadata()).expect("seal");
    let err = decrypt(&account_key(0x22), &sealed).expect_err("wrong key must not open");
    assert!(matches!(err, RemittanceMetadataError::Decrypt(_)), "{err}");
}

/// The payer picks a fresh ephemeral key per remittance, so two identical
/// breakdowns must not produce identical envelopes on chain.
#[test]
fn each_remittance_gets_a_fresh_envelope() {
    let recipient = account_key(0x11).public_key();
    let first = encrypt(&recipient, &metadata()).expect("seal");
    let second = encrypt(&recipient, &metadata()).expect("seal");
    assert_ne!(first, second);
}

#[test]
fn future_envelope_versions_are_refused_rather_than_guessed() {
    let key = account_key(0x11);
    let sealed = encrypt(&key.public_key(), &metadata()).expect("seal");
    let mut envelope: serde_json::Value = serde_json::from_slice(&sealed).expect("envelope json");
    envelope["version"] = serde_json::json!(2);
    let bumped = serde_json::to_vec(&envelope).expect("re-encode");
    assert!(matches!(
        decrypt(&key, &bumped).expect_err("version 2 is not this format"),
        RemittanceMetadataError::EnvelopeVersion(2)
    ));
}
/// The payer is a different codebase, so the round-trip above only proves we
/// agree with ourselves. This fixture pins the wire form we must keep reading:
/// envelope field names, hex ciphertext, the ECDH domain string, and the
/// plaintext field names. A change that breaks it breaks live remittances.
#[test]
fn a_fixed_envelope_still_opens() {
    const SEALED: &str = concat!(
        r#"{"version":1,"#,
        r#""ephemeral_pubkey":"020f5266b113823985c171228179901c0d18289b962fc618b1830a2f3726e6a5a3","#,
        r#""ciphertext_hex":"f4b37b38bcafbf0c1b2bea6afa2856c2504270ad49baf5704be2c0c081cbabf0"#,
        r#"14bc09cc3938d66e6d5d15d3377c93784c1834262229f54acfc99fc5542d66887db32a60632ce07485"#,
        r#"2b11f274eddc58095d00be206cce7c53c124e854304f536cadbdca2c501a4992c038d321b5e1ca4b16"#,
        r#"3ce0dcb6bfb88e1af87e5f25240a7c71ddf01f5462a1ea7ca78352b174903e23605a0350fb85373a32"#,
        r#"547e0f66b73c142bde7f85e759cc145fedfb49d07f89c10adb976a33d66ef91932e66a04463ba29176"#,
        r#"87f2316a5f66a05133168884e38f39d0608a9adf1052b6f4806b0ca2fc75af6b4447dba40c64e4795d"#,
        r#"6363193275"}"#,
    );
    assert_eq!(
        decrypt(&account_key(0x11), SEALED.as_bytes()).expect("open fixture"),
        metadata()
    );
}
