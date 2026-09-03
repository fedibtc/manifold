//! Tests for FMan directory and public trust-material types.

use std::sync::LazyLock;

use super::*;
use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, IssuerSecretKeys, PendingIssuance,
};
use nostr::{
    Keys, SecretKey,
    secp256k1::{Message, schnorr::Signature as SchnorrSignature},
};

static ISSUER_SECRET_KEYS: LazyLock<IssuerSecretKeys> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../trust_score/issuer-secret-keys.json"))
        .expect("fixed test issuer keys deserialize")
});

fn holder_envelope(subject: &Pubkey) -> HolderAuthorizationEnvelope {
    let issuer = IssuerContext::import_secret_key(&ISSUER_SECRET_KEYS).expect("import issuer");
    let authority = issuer.issuer_authority(vec![]).expect("issuer authority");
    let holder = HolderContext::generate();
    let info = crate::trust_score_info_v1(6).expect("legal trust level");
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        serde_json::json!(holder.public_key().to_string()),
    )
    .expect("create issuance request");
    let issued = issuer
        .issue_credential(info, &request)
        .expect("issue credential");
    let credential = pending
        .finalize(&authority.issuer.issuance_key, &issued)
        .expect("finalize credential");
    let holder_authorization = holder
        .authorize_credential_use_at_time(
            HolderAuthorizationRequest {
                subject_pubkey: subject.0.parse().expect("subject pubkey"),
            },
            &credential,
            1_000,
        )
        .expect("authorize credential use");
    HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential: credential,
    }
}

fn example_request() -> GetFmanTrustMaterialRequest {
    GetFmanTrustMaterialRequest {
        version: ProtocolV1,
    }
}

fn example_material(keys: &Keys) -> FmanTrustMaterial {
    FmanTrustMaterial {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        holder_authorizations: vec![],
    }
}

fn signed_response(keys: &Keys, material: FmanTrustMaterial) -> GetFmanTrustMaterialResponse {
    let message = Message::from_digest(material.digest().expect("material digests"));
    GetFmanTrustMaterialResponse {
        version: ProtocolV1,
        material,
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
fn fman_trust_material_canonical_payload_is_typed() {
    let material = FmanTrustMaterial {
        fman_pubkey: Pubkey("fman".to_owned()),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        holder_authorizations: vec![],
    };
    assert_eq!(
        String::from_utf8(material.canonical_bytes().expect("material canonicalizes")).unwrap(),
        "{\"material\":{\"expires_at\":20,\"fman_pubkey\":\"fman\",\"holder_authorizations\":[],\"issued_at\":10,\"public_api_urls\":[\"iroh://node-a\"]},\"type\":\"fedi.fman.trust-material\",\"version\":1}"
    );
}

#[test]
fn fman_trust_material_digest_is_stable() {
    let material = FmanTrustMaterial {
        fman_pubkey: Pubkey("fman".to_owned()),
        issued_at: Timestamp(10),
        expires_at: Timestamp(20),
        public_api_urls: vec![Url("iroh://node-a".to_owned())],
        holder_authorizations: vec![],
    };
    assert_eq!(
        material.digest().expect("material digests"),
        [
            181, 197, 231, 84, 87, 214, 0, 126, 17, 117, 73, 30, 49, 27, 217, 38, 31, 146, 51, 168,
            63, 202, 32, 140, 113, 26, 249, 167, 214, 178, 103, 66
        ]
    );
}

#[test]
fn fman_trust_material_response_verifies_for_expected_identity() {
    let keys = Keys::generate();
    let response = signed_response(&keys, example_material(&keys));
    let expected = Pubkey(keys.public_key().to_string());
    assert_eq!(
        response
            .verify_for_fman(&expected, Timestamp(12), 60)
            .expect("response verifies"),
        response.material
    );
}

#[test]
fn fman_trust_material_response_rejects_unexpected_identity() {
    let keys = Keys::generate();
    let response = signed_response(&keys, example_material(&keys));
    let unexpected = Pubkey(Keys::generate().public_key().to_string());
    assert_eq!(
        response
            .verify_for_fman(&unexpected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::UnexpectedFman
    );
}

#[test]
fn fman_trust_material_response_rejects_wrong_signature() {
    let keys = Keys::generate();
    let wrong_keys = Keys::generate();
    let material = example_material(&keys);
    let expected = material.fman_pubkey.clone();
    let response = GetFmanTrustMaterialResponse {
        version: ProtocolV1,
        material,
        proof: SchnorrSignatureProof {
            signature: wrong_keys.sign_schnorr(&Message::from_digest([7_u8; 32])),
        },
    };
    assert_eq!(
        response
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidSignature
    );
}

#[test]
fn fman_trust_material_response_wire_shape_includes_proof() {
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
                "issued_at": 10,
                "expires_at": 20,
                "public_api_urls": ["iroh://node-a"],
                "holder_authorizations": [],
            },
            "proof": {
                "signature": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ",
            }
        })
    );
}

#[test]
fn fman_trust_material_response_rejects_invalid_time_windows() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let mut material = example_material(&keys);
    material.expires_at = Timestamp(10);
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidFreshnessWindow
    );

    let mut material = example_material(&keys);
    material.issued_at = Timestamp(5000);
    material.expires_at = Timestamp(5010);
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::IssuedInFuture
    );
}

#[test]
fn fman_trust_material_response_rejects_invalid_or_noncanonical_public_urls() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let mut material = example_material(&keys);
    material.public_api_urls = vec![Url("https://node-a".to_owned())];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::UnsupportedUrlScheme
        )
    );

    let mut material = example_material(&keys);
    material.public_api_urls = vec![
        Url("iroh://node-b".to_owned()),
        Url("iroh://node-a".to_owned()),
    ];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::NonCanonical
        )
    );
}

#[test]
fn fman_trust_material_response_rejects_noncanonical_fman_pubkey() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.fman_pubkey = Pubkey(keys.public_key().to_string().to_uppercase());
    let response = signed_response(&keys, material);
    assert_eq!(
        response.verify_envelope_signature().unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidFmanPubkey
    );
}

#[test]
fn fman_trust_material_response_rejects_expired_and_overlong_validity() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let mut material = example_material(&keys);
    material.issued_at = Timestamp(1);
    material.expires_at = Timestamp(10);
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(10), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::Expired
    );

    let mut material = example_material(&keys);
    material.expires_at = Timestamp(100);
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::ValidityWindowTooLarge
    );
}

#[test]
fn fman_trust_material_response_enforces_response_and_holder_bounds() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let mut material = example_material(&keys);
    material.public_api_urls = vec![Url(format!(
        "iroh://{}",
        "a".repeat(FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES)
    ))];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::ResponseTooLarge
    );

    let mut material = example_material(&keys);
    let envelope = holder_envelope(&expected);
    material.holder_authorizations =
        vec![envelope; FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS + 1];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::TooManyHolderAuthorizations
    );
}

#[test]
fn fman_trust_material_response_rejects_wrong_holder_subject() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let other = Pubkey(Keys::generate().public_key().to_string());
    let mut material = example_material(&keys);
    material.holder_authorizations = vec![holder_envelope(&other)];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::HolderAuthorizationSubjectMismatch
    );
}

#[test]
fn fman_trust_material_response_rejects_duplicate_public_urls() {
    let keys = Keys::generate();
    let expected = Pubkey(keys.public_key().to_string());
    let mut material = example_material(&keys);
    material.public_api_urls = vec![
        Url("iroh://node-a".to_owned()),
        Url("iroh://node-a".to_owned()),
    ];
    assert_eq!(
        signed_response(&keys, material)
            .verify_for_fman(&expected, Timestamp(12), 60)
            .unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidPublicApiUrls(
            FmanApiUrlsMetadataError::NonCanonical
        )
    );
}

#[test]
fn fman_trust_material_response_rejects_malformed_fman_pubkey() {
    let keys = Keys::generate();
    let mut material = example_material(&keys);
    material.fman_pubkey = Pubkey("not-a-nostr-key".to_owned());
    let response = signed_response(&keys, material);
    assert_eq!(
        response.verify_envelope_signature().unwrap_err(),
        FmanTrustMaterialVerificationError::InvalidFmanPubkey
    );
}

#[test]
fn fman_trust_material_request_wire_shape_is_stable() {
    assert_eq!(
        serde_json::to_value(example_request()).expect("request serializes"),
        serde_json::json!({"version": 1})
    );
    assert!(
        serde_json::from_value::<GetFmanTrustMaterialRequest>(serde_json::json!({
            "version": 1,
            "federation_id": "old-shape-is-not-accepted"
        }))
        .is_err()
    );
}

#[test]
fn fman_trust_material_wire_rejects_unknown_response_fields() {
    let keys = Keys::generate();
    let response = signed_response(&keys, example_material(&keys));
    let mut value = serde_json::to_value(&response).expect("response serializes");
    value["material"]["federation_id"] = serde_json::json!("old-field");
    assert!(serde_json::from_value::<GetFmanTrustMaterialResponse>(value).is_err());

    let mut value = serde_json::to_value(response).expect("response serializes");
    value["peer_attestations"] = serde_json::json!([]);
    assert!(serde_json::from_value::<GetFmanTrustMaterialResponse>(value).is_err());
}
