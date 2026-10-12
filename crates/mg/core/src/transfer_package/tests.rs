use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationId, Locator, QuoteId, SeatId, Timestamp,
};

use super::*;

fn key(byte: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[byte; 32]).unwrap()
}

fn grant(destination: &Keypair) -> SeatTransferGrant {
    SeatTransferGrant {
        ts: Timestamp(1_800_000_000),
        fa_id: FaId(key(7).x_only_public_key().0),
        federation_id: FederationId("ab".repeat(32)),
        source: Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[5; 32]).public(),
            ),
            key(6).x_only_public_key().0,
        ),
        source_seat_id: SeatId::from(QuoteId([1; 32])),
        destination_service_pubkey: destination.x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([2; 32])),
        expires_at: Timestamp(1_800_000_100),
    }
}

fn material() -> TransferMaterial {
    TransferMaterial {
        archive: GuardianArchive {
            private_encrypt: "ciphertext-placeholder".into(),
            private_salt: "salt-placeholder".into(),
            local_json: "{\"local\":\"test\"}".into(),
            consensus_json: "{\"consensus\":\"test\"}".into(),
        },
        api_auth: "secret-test-password".into(),
    }
}

#[test]
fn roundtrip_both_recipient_and_ephemeral_parities() {
    for parity in [Parity::Even, Parity::Odd] {
        let destination = (1..=255)
            .map(key)
            .find(|key| key.x_only_public_key().1 == parity)
            .unwrap();
        let grant = grant(&destination);
        let material = material();
        let mut seen = [false; 2];
        // Fresh ephemeral points cover both parities with overwhelming
        // probability; stop once both are exercised, not a fixed lucky pair.
        for _ in 0..128 {
            let package = seal(&material, &grant, &key(6)).unwrap();
            let ephemeral = PublicKey::from_slice(&package.as_bytes()[8..41]).unwrap();
            seen[usize::from(ephemeral.serialize()[0] == 3)] = true;
            let opened = open(&package, &grant, &destination).unwrap();
            assert_eq!(opened.archive, material.archive);
            assert_eq!(opened.api_auth, material.api_auth);
            assert_eq!(format!("{opened:?}"), "TransferMaterial(redacted)");
            if seen == [true; 2] {
                break;
            }
        }
        assert_eq!(seen, [true; 2]);
    }
}

#[test]
fn fresh_seals_are_distinct_and_bound_to_every_grant_field() {
    let destination = key(9);
    let grant = grant(&destination);
    let first = seal(&material(), &grant, &key(6)).unwrap();
    let second = seal(&material(), &grant, &key(6)).unwrap();
    assert_ne!(first, second);
    let mut changed = Vec::new();
    let mut next = grant.clone();
    next.ts.0 += 1;
    changed.push(next);
    let mut next = grant.clone();
    next.expires_at.0 += 1;
    changed.push(next);
    let mut next = grant.clone();
    next.fa_id = FaId(key(8).x_only_public_key().0);
    changed.push(next);
    let mut next = grant.clone();
    next.federation_id = FederationId("cd".repeat(32));
    changed.push(next);
    let mut next = grant.clone();
    next.source.service_pubkey = key(8).x_only_public_key().0;
    changed.push(next);
    let mut next = grant.clone();
    next.source = Locator::new(
        fedi_iroh_rpc::iroh::EndpointAddr::new(
            fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[8; 32]).public(),
        ),
        next.source.service_pubkey,
    );
    changed.push(next);
    let mut next = grant.clone();
    next.source_seat_id = SeatId::from(QuoteId([3; 32]));
    changed.push(next);
    let mut next = grant.clone();
    next.destination_seat_id = SeatId::from(QuoteId([3; 32]));
    changed.push(next);
    let mut next = grant.clone();
    next.destination_service_pubkey = key(8).x_only_public_key().0;
    changed.push(next);
    for next in changed {
        assert_eq!(
            open(&first, &next, &destination).unwrap_err(),
            PackageError::Binding
        );
    }
    assert_eq!(
        open(&first, &grant, &key(8)).unwrap_err(),
        PackageError::Binding
    );
    assert_eq!(
        seal(&material(), &grant, &key(8)).unwrap_err(),
        PackageError::Binding
    );
}

#[test]
fn reject_each_modified_byte_and_every_truncation() {
    let destination = key(9);
    let grant = grant(&destination);
    let package = seal(&material(), &grant, &key(6)).unwrap();
    for index in 0..package.as_bytes().len() {
        let mut modified = package.as_bytes().to_vec();
        modified[index] ^= 1;
        assert!(
            open(
                &SealedSeatPackage::new(modified).unwrap(),
                &grant,
                &destination
            )
            .is_err()
        );
        let truncated = SealedSeatPackage::new(package.as_bytes()[..index].to_vec()).unwrap();
        assert!(open(&truncated, &grant, &destination).is_err());
    }
}

#[test]
fn authenticated_malformed_ephemeral_point_is_rejected() {
    let destination = key(9);
    let grant = grant(&destination);
    let package = seal(&material(), &grant, &key(6)).unwrap();
    let mut modified = package.as_bytes().to_vec();
    modified[8..41].fill(0);
    let signed_end = modified.len() - SIGNATURE_LEN;
    let signature = SECP256K1.sign_schnorr(&signature_hash(&modified[..signed_end]), &key(6));
    modified[signed_end..].copy_from_slice(signature.as_ref());
    assert_eq!(
        open(
            &SealedSeatPackage::new(modified).unwrap(),
            &grant,
            &destination
        )
        .unwrap_err(),
        PackageError::Invalid
    );
}

#[test]
fn source_signature_does_not_replace_aead_authentication() {
    let destination = key(9);
    let grant = grant(&destination);
    let package = seal(&material(), &grant, &key(6)).unwrap();
    let signed_end = package.as_bytes().len() - SIGNATURE_LEN;
    // A source signature authenticates the sender, not AEAD correctness.
    // Re-sign independently corrupted nonce, ciphertext and tag bytes so
    // each test reaches the decryption/authentication boundary.
    for index in [HEADER_LEN, HEADER_LEN + 12, signed_end - 1] {
        let mut modified = package.as_bytes().to_vec();
        modified[index] ^= 1;
        let signature = SECP256K1.sign_schnorr(&signature_hash(&modified[..signed_end]), &key(6));
        modified[signed_end..].copy_from_slice(signature.as_ref());
        assert_eq!(
            open(
                &SealedSeatPackage::new(modified).unwrap(),
                &grant,
                &destination
            )
            .unwrap_err(),
            PackageError::Authentication
        );
    }
}

#[test]
fn enforce_exact_wire_budget_before_encrypting() {
    let destination = key(9);
    let grant = grant(&destination);
    let mut material = material();
    let current = material.encode().unwrap().len();
    material
        .archive
        .consensus_json
        .push_str(&"x".repeat(MAX_PLAINTEXT - current));
    let package = seal(&material, &grant, &key(6)).unwrap();
    assert_eq!(package.as_bytes().len(), SEALED_SEAT_PACKAGE_MAX_BYTES);
    assert!(open(&package, &grant, &destination).is_ok());
    material.archive.consensus_json.push('x');
    assert_eq!(
        seal(&material, &grant, &key(6)).unwrap_err(),
        PackageError::TooLarge
    );
}

#[test]
fn malformed_plaintext_cannot_allocate_declared_lengths_or_ignore_trailing_bytes() {
    let encoded = material().encode().unwrap();
    for end in 0..encoded.len() {
        assert!(TransferMaterial::decode(&encoded[..end]).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(TransferMaterial::decode(&trailing).is_err());
    let mut overlong = encoded.clone();
    overlong[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(TransferMaterial::decode(&overlong).is_err());
    let mut invalid_utf8 = encoded;
    invalid_utf8[4] = 0xff;
    assert!(TransferMaterial::decode(&invalid_utf8).is_err());
    let mut empty_auth = material();
    empty_auth.api_auth.clear();
    assert!(empty_auth.encode().is_err());
    empty_auth.api_auth = "x".repeat(MAX_API_AUTH + 1);
    assert!(empty_auth.encode().is_err());
}
