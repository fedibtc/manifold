//! Tests for FMan federation-directory and public trust-material types.

use super::*;
use crate::FmanPeerAttestationStatement;
use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey as BitcoinSecretKey};
use nostr::{
    Keys, SecretKey,
    secp256k1::{Message, schnorr::Signature as SchnorrSignature},
};
use stability_pool_common::{Account, AccountType};

fn guardian_fee_account(byte: u8) -> Account {
    Account::single(
        PublicKey::from_secret_key(
            SECP256K1,
            &BitcoinSecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
}

fn example_request() -> GetFederationTrustMaterialRequest {
    GetFederationTrustMaterialRequest {
        version: ProtocolV1,
        federation_id: FederationId("fed".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        peer_ids: vec![],
    }
}

fn example_material(keys: &Keys) -> FmanFederationTrustMaterial {
    FmanFederationTrustMaterial {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        federation_id: FederationId("fed".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        peer_attestations: vec![],
        holder_authorizations: vec![],
    }
}

fn signed_response(
    keys: &Keys,
    material: FmanFederationTrustMaterial,
) -> GetFederationTrustMaterialResponse {
    let message = Message::from_digest(material.digest().expect("material digests"));
    GetFederationTrustMaterialResponse {
        version: ProtocolV1,
        material,
        proof: SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

fn signed_peer_attestation(keys: &Keys, peer_id: &str) -> FmanPeerAttestation {
    let statement = FmanPeerAttestationStatement {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        federation_id: FederationId("fed".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        peer_id: PeerId(peer_id.to_owned()),
        guardian_identity: crate::GuardianIdentity(format!("guardian-{peer_id}")),
        guardian_fee_account: guardian_fee_account(1),
        issued_at: Timestamp(11),
    };
    let message = Message::from_digest(statement.digest().expect("attestation digests"));

    FmanPeerAttestation {
        version: ProtocolV1,
        attestation: statement,
        proof: SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

#[test]
fn fman_api_urls_metadata_sorts_dedupes_and_canonicalizes() {
    let metadata = FmanApiUrlsMetadata::new([
        Url("iroh://node-b".to_owned()),
        Url("iroh://node-a".to_owned()),
        Url("iroh://node-a".to_owned()),
    ])
    .expect("metadata validates");

    assert_eq!(
        metadata.fman_api_urls(),
        [
            Url("iroh://node-a".to_owned()),
            Url("iroh://node-b".to_owned())
        ]
    );
    assert_eq!(
        metadata.canonical_string().expect("metadata canonicalizes"),
        "{\"fman_api_urls\":[\"iroh://node-a\",\"iroh://node-b\"],\"version\":1}"
    );
}

#[test]
fn fman_api_urls_parse_requires_exact_canonical_value() {
    let canonical = "{\"fman_api_urls\":[\"iroh://node-a\",\"iroh://node-b\"],\"version\":1}";
    let parsed = FmanApiUrlsMetadata::parse_canonical(canonical).expect("canonical parses");

    assert_eq!(
        parsed.fman_api_urls(),
        [
            Url("iroh://node-a".to_owned()),
            Url("iroh://node-b".to_owned())
        ]
    );
    assert_eq!(
        FmanApiUrlsMetadata::parse_canonical(
            "{\"version\":1,\"fman_api_urls\":[\"iroh://node-b\",\"iroh://node-a\"]}"
        )
        .unwrap_err(),
        FmanApiUrlsMetadataError::NonCanonical
    );
}

#[test]
fn fman_api_urls_metadata_serde_rejects_noncanonical_wire_lists() {
    assert!(
        serde_json::from_str::<FmanApiUrlsMetadata>(
            "{\"fman_api_urls\":[\"iroh://node-b\",\"iroh://node-a\"],\"version\":1}"
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<FmanApiUrlsMetadata>(
            "{\"fman_api_urls\":[\"iroh://node-a\",\"iroh://node-a\"],\"version\":1}"
        )
        .is_err()
    );
}

#[test]
fn fman_api_urls_validator_rejects_non_iroh_discovery_data() {
    assert_eq!(
        validate_fman_api_url(&Url("https://fman.example.invalid".to_owned())).unwrap_err(),
        FmanApiUrlsMetadataError::UnsupportedUrlScheme
    );
    assert_eq!(
        validate_fman_api_url(&Url("iroh://".to_owned())).unwrap_err(),
        FmanApiUrlsMetadataError::MissingEndpoint
    );
    assert_eq!(
        validate_fman_api_url(&Url("iroh://node\n".to_owned())).unwrap_err(),
        FmanApiUrlsMetadataError::UrlContainsControlCharacter
    );
}

#[test]
fn fman_api_urls_validator_rejects_long_url() {
    let url = Url(format!("iroh://{}", "a".repeat(FMAN_API_URL_MAX_BYTES)));

    assert_eq!(
        validate_fman_api_url(&url).unwrap_err(),
        FmanApiUrlsMetadataError::UrlTooLong
    );
}

#[test]
fn fman_api_urls_metadata_enforces_count_limit() {
    let urls = (0..=FMAN_API_URLS_MAX_COUNT)
        .map(|idx| Url(format!("iroh://node-{idx}")))
        .collect::<Vec<_>>();

    assert_eq!(
        FmanApiUrlsMetadata::new(urls).unwrap_err(),
        FmanApiUrlsMetadataError::TooManyUrls
    );
}

#[test]
fn fman_api_urls_metadata_enforces_canonical_value_size_limit() {
    let urls = (0..FMAN_API_URLS_MAX_COUNT)
        .map(|idx| Url(format!("iroh://{}-{idx}", "a".repeat(120))))
        .collect::<Vec<_>>();

    assert_eq!(
        FmanApiUrlsMetadata::new(urls).unwrap_err(),
        FmanApiUrlsMetadataError::ValueTooLarge
    );
}

#[test]
fn federation_trust_material_canonical_payload_is_typed() {
    let material = FmanFederationTrustMaterial {
        fman_pubkey: Pubkey("fman".to_owned()),
        federation_id: FederationId("fed".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        peer_attestations: vec![],
        holder_authorizations: vec![],
    };

    assert_eq!(
        String::from_utf8(material.canonical_bytes().expect("material canonicalizes")).unwrap(),
        "{\"material\":{\"expires_at\":20,\"federation_config_hash\":[1,2,3],\"federation_id\":\"fed\",\"fman_pubkey\":\"fman\",\"holder_authorizations\":[],\"issued_at\":10,\"peer_attestations\":[],\"public_api_urls\":[\"iroh://node-a\"]},\"type\":\"fedi.fman.federation-trust-material\",\"version\":1}"
    );
}

#[test]
fn federation_trust_material_digest_is_stable() {
    let material = FmanFederationTrustMaterial {
        fman_pubkey: Pubkey("fman".to_owned()),
        federation_id: FederationId("fed".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        peer_attestations: vec![],
        holder_authorizations: vec![],
    };

    // Repinned on 2026-08-03 when `signed_credentials` was folded into
    // `holder_authorizations` as envelopes. The digest is what the FMan signs
    // and what every verifier recomputes, so a change here is a wire break, not
    // a refactor — safe only because the verb had no producer at the time.
    assert_eq!(
        material.digest().expect("material digests"),
        [
            59, 194, 16, 177, 241, 189, 99, 231, 206, 31, 215, 120, 186, 255, 22, 104, 234, 75, 38,
            226, 118, 174, 54, 186, 2, 10, 62, 247, 209, 161, 122, 163,
        ]
    );
}

#[test]
fn federation_trust_material_response_verifies_signature() {
    let keys = Keys::generate();
    let response = signed_response(&keys, example_material(&keys));

    assert_eq!(
        response
            .verify_envelope_signature()
            .expect("response verifies"),
        response.material
    );
}

#[test]
fn federation_trust_material_response_rejects_wrong_signature() {
    let keys = Keys::generate();
    let wrong_keys = Keys::generate();
    let material = example_material(&keys);
    let message = Message::from_digest([7_u8; 32]);
    let response = GetFederationTrustMaterialResponse {
        version: ProtocolV1,
        material,
        proof: SchnorrSignatureProof {
            signature: wrong_keys.sign_schnorr(&message),
        },
    };

    assert_eq!(
        response.verify_envelope_signature().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidSignature
    );
}

#[test]
fn federation_trust_material_response_wire_shape_includes_proof() {
    let secret =
        SecretKey::parse("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("test secret parses");
    let keys = Keys::new(secret);
    let mut response = signed_response(&keys, example_material(&keys));
    response.proof.signature =
        SchnorrSignature::from_slice(&[1_u8; 64]).expect("test signature parses");

    assert_eq!(
        serde_json::to_value(&response).expect("response serializes"),
        serde_json::json!({
            "version": 1,
            "material": {
                "fman_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "federation_id": "fed",
                "federation_config_hash": [1, 2, 3],
                "issued_at": 10,
                "expires_at": 20,
                "public_api_urls": ["iroh://node-a"],
                "peer_attestations": [],
                "holder_authorizations": [],
            },
            "proof": {
                "signature": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ",
            }
        })
    );
}

#[test]
fn federation_trust_material_response_verifies_for_request() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.peer_attestations = vec![signed_peer_attestation(&keys, "0")];
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .expect("response verifies"),
        response.material
    );
}

#[test]
fn federation_trust_material_response_rejects_config_hash_mismatch() {
    let keys = Keys::generate();
    let response = signed_response(&keys, example_material(&keys));
    let mut request = example_request();
    request.federation_config_hash = HashBytes(vec![9]);

    assert_eq!(
        response
            .verify_for_request(&request, Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::ConfigHashMismatch
    );
}

#[test]
fn federation_trust_material_request_rejects_oversized_fields() {
    let mut request = example_request();
    request.federation_id =
        FederationId("a".repeat(FMAN_TRUST_MATERIAL_FEDERATION_ID_MAX_BYTES + 1));
    assert_eq!(
        request.validate().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidFederationId
    );

    let mut request = example_request();
    request.federation_config_hash =
        HashBytes(vec![1; FMAN_TRUST_MATERIAL_CONFIG_HASH_MAX_BYTES + 1]);
    assert_eq!(
        request.validate().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidConfigHash
    );

    let mut request = example_request();
    request.peer_ids = vec![PeerId(
        "a".repeat(FMAN_TRUST_MATERIAL_PEER_ID_MAX_BYTES + 1),
    )];
    assert_eq!(
        request.validate().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidPeerId
    );
}

#[test]
fn federation_trust_material_request_rejects_oversized_request() {
    let mut request = example_request();
    request.peer_ids = (0..FMAN_TRUST_MATERIAL_PEER_FILTER_MAX_COUNT)
        .map(|idx| PeerId(format!("peer-{idx}-{}", "a".repeat(55))))
        .collect();

    assert_eq!(
        request.validate().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::RequestTooLarge
    );
}

#[test]
fn federation_trust_material_response_rejects_invalid_time_windows() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.expires_at = Timestamp(10);
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidFreshnessWindow
    );

    let mut material = example_material(&keys);
    material.issued_at = Timestamp(5000);
    material.expires_at = Timestamp(5010);
    let response = signed_response(&keys, material);
    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::IssuedInFuture
    );
}

#[test]
fn federation_trust_material_response_rejects_invalid_public_urls() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.public_api_urls = vec![Url("https://node-a".to_owned())];
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::UnsupportedUrlScheme
        )
    );
}

#[test]
fn federation_trust_material_response_rejects_noncanonical_public_urls() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.public_api_urls = vec![
        Url("iroh://node-b".to_owned()),
        Url("iroh://node-a".to_owned()),
    ];
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::NonCanonical
        )
    );
}

#[test]
fn federation_trust_material_response_rejects_duplicate_public_urls() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.public_api_urls = vec![
        Url("iroh://node-a".to_owned()),
        Url("iroh://node-a".to_owned()),
    ];
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::NonCanonical
        )
    );
}

#[test]
fn federation_trust_material_response_rejects_nested_fman_mismatch() {
    let keys = Keys::generate();
    let other_keys = Keys::generate();
    let mut material = example_material(&keys);
    material.peer_attestations = vec![signed_peer_attestation(&other_keys, "0")];
    let response = signed_response(&keys, material);

    assert_eq!(
        response
            .verify_for_request(&example_request(), Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::NestedFmanMismatch
    );
}

#[test]
fn federation_trust_material_response_rejects_peer_filter_mismatch() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.peer_attestations = vec![signed_peer_attestation(&keys, "1")];
    let response = signed_response(&keys, material);
    let mut request = example_request();
    request.peer_ids = vec![PeerId("0".to_owned())];

    assert_eq!(
        response
            .verify_for_request(&request, Timestamp(12), 60)
            .unwrap_err(),
        FmanFederationTrustMaterialVerificationError::PeerFilterMismatch
    );
}

#[test]
fn federation_trust_material_response_rejects_noncanonical_fman_pubkey() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.fman_pubkey = Pubkey(keys.public_key().to_string().to_uppercase());
    let response = signed_response(&keys, material);

    assert_eq!(
        response.verify_envelope_signature().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidFmanPubkey
    );
}

#[test]
fn federation_trust_material_response_rejects_malformed_fman_pubkey() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.fman_pubkey = Pubkey("not-a-nostr-key".to_owned());
    let response = signed_response(&keys, material);

    assert_eq!(
        response.verify_envelope_signature().unwrap_err(),
        FmanFederationTrustMaterialVerificationError::InvalidFmanPubkey
    );
}

#[test]
fn federation_trust_material_request_wire_shape_is_stable() {
    let request = example_request();

    assert_eq!(
        serde_json::to_value(request).expect("request serializes"),
        serde_json::json!({
            "version": 1,
            "federation_id": "fed",
            "federation_config_hash": [1, 2, 3],
            "peer_ids": [],
        })
    );
}
