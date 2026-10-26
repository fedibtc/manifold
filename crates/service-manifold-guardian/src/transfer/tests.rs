use secp256k1::{Keypair, SECP256K1};

use super::*;
use crate::signing::AuthError;
use crate::{MgSignedRequest, QuoteId, SignedRequest};

const ISSUED: u64 = 1_800_000_000;
const DAY: u64 = 24 * 60 * 60;

fn fa_key() -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[7; 32]).unwrap()
}

fn locator() -> Locator {
    Locator::new(
        fedi_iroh_rpc::iroh::EndpointAddr::new(
            fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[5_u8; 32]).public(),
        ),
        Keypair::from_seckey_slice(SECP256K1, &[6; 32])
            .unwrap()
            .x_only_public_key()
            .0,
    )
}

fn grant(expires_after: u64) -> SeatTransferGrant {
    let key = fa_key();
    SeatTransferGrant {
        ts: Timestamp(ISSUED),
        fa_id: FaId(key.x_only_public_key().0),
        federation_id: FederationId(
            "15db8cb4f1ec8e484d73b889372bec94812580f929e8148b7437d359af422cd3".to_owned(),
        ),
        source: locator(),
        source_seat_id: SeatId::from(QuoteId([1; 32])),
        destination_service_pubkey: Keypair::from_seckey_slice(SECP256K1, &[9; 32])
            .unwrap()
            .x_only_public_key()
            .0,
        destination_seat_id: SeatId::from(QuoteId([2; 32])),
        expires_at: Timestamp(ISSUED + expires_after),
    }
}

fn verify_at(grant: &SeatTransferGrant, now: u64) -> Result<SeatTransferGrant, AuthError> {
    SignedRequest::create(grant, &fa_key())
        .unwrap()
        .verify(Timestamp(now))
        .map(|verified| verified.into_inner())
}

#[test]
fn grant_outlives_the_one_hour_request_window_until_expiry() {
    let grant = grant(14 * DAY);

    // An ordinary request would be stale here; a grant is still valid.
    assert_eq!(verify_at(&grant, ISSUED + 2 * 60 * 60).unwrap(), grant);
    assert_eq!(verify_at(&grant, ISSUED + 14 * DAY).unwrap(), grant);
    assert!(matches!(
        verify_at(&grant, ISSUED + 14 * DAY + 1),
        Err(AuthError::Stale)
    ));
}

#[test]
fn grant_keeps_the_usual_clock_allowance_before_issue() {
    let grant = grant(DAY);

    assert!(verify_at(&grant, ISSUED - FA_REQUEST_FRESHNESS_WINDOW_SECS).is_ok());
    assert!(matches!(
        verify_at(&grant, ISSUED - FA_REQUEST_FRESHNESS_WINDOW_SECS - 1),
        Err(AuthError::Stale)
    ));
}

#[test]
fn grant_longer_than_the_maximum_lifetime_is_never_valid() {
    assert!(verify_at(&grant(SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS), ISSUED).is_ok());
    assert!(matches!(
        verify_at(&grant(SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS + 1), ISSUED),
        Err(AuthError::Stale)
    ));
}

#[test]
fn grant_signature_does_not_verify_as_another_verb() {
    let envelope = SignedRequest::create(&grant(DAY), &fa_key()).unwrap();
    let as_remove: SignedRequest<crate::RemoveSeatRequest> =
        serde_json::from_value(serde_json::to_value(&envelope).unwrap()).unwrap();

    assert!(matches!(
        as_remove.verify(Timestamp(ISSUED)),
        Err(AuthError::BadSignature)
    ));
}

fn destination_key() -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[9; 32]).unwrap()
}

fn transfer_request(key: &Keypair) -> TransferSeatRequest {
    TransferSeatRequest {
        ts: Timestamp(ISSUED),
        destination_service_pubkey: key.x_only_public_key().0,
        grant: SignedRequest::create(&grant(DAY), &fa_key()).unwrap(),
    }
}

#[test]
fn transfer_request_verifies_only_under_the_destination_key() {
    let request = transfer_request(&destination_key());
    let envelope = MgSignedRequest::create(&request, &destination_key()).unwrap();
    assert_eq!(envelope.verify(Timestamp(ISSUED + 60)).unwrap(), request);
    assert!(matches!(
        envelope.verify(Timestamp(ISSUED + FA_REQUEST_FRESHNESS_WINDOW_SECS + 1)),
        Err(AuthError::Stale)
    ));

    // The envelope signer is the key the payload names, so another MG cannot
    // sign a payload naming the destination.
    let forged = MgSignedRequest::create(&request, &fa_key()).unwrap();
    assert!(matches!(
        forged.verify(Timestamp(ISSUED)),
        Err(AuthError::BadSignature)
    ));
    let other = transfer_request(&fa_key());
    let mismatched: MgSignedRequest<TransferSeatRequest> = serde_json::from_value(
        serde_json::to_value(MgSignedRequest::create(&other, &fa_key()).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(mismatched.verify(Timestamp(ISSUED)).unwrap(), other);
}

#[test]
fn guardian_material_is_bounded_and_redacted() {
    let material = GuardianMaterial {
        private_json: "a".repeat(SEAT_TRANSFER_MATERIAL_MAX_BYTES - 64 - 2),
        local_json: "c".to_owned(),
        consensus_json: "d".to_owned(),
        iroh_api_sk: [1; 32],
        iroh_p2p_sk: [2; 32],
    };
    assert!(material.check_size().is_ok());
    assert_eq!(format!("{material:?}"), "GuardianMaterial(redacted)");
    let oversized = GuardianMaterial {
        local_json: "cc".to_owned(),
        ..material
    };
    assert!(oversized.check_size().is_err());
}
