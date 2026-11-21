use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_iroh_rpc::iroh::{EndpointAddr, SecretKey};
use secp256k1::{Keypair, SECP256K1};

use super::*;
use crate::{GetQuoteResponse, manager_response_signing_digest};

const ENV: ManifoldEnvironment = ManifoldEnvironment::Staging;

fn mg_key() -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[3; 32]).unwrap()
}

fn locator(key: &Keypair) -> Locator {
    let endpoint = SecretKey::from_bytes(&[4; 32]).public();
    Locator::new(EndpointAddr::new(endpoint), key.x_only_public_key().0)
}

fn grant(label: Option<&str>) -> SeatGrant {
    SeatGrant {
        grant_nonce: SeatGrantNonce::from_bytes([0xa5; 32]),
        issued_at: Timestamp(1_790_000_000),
        label: label.map(|label| SeatGrantLabel::new(label).unwrap()),
    }
}

fn code(grant: &SeatGrant) -> SeatGrantCode {
    let key = mg_key();
    let signed = SignedResponse::create(grant, &key).unwrap();
    SeatGrantCode::new(ENV, locator(&key), signed).unwrap()
}

/// The URI's query values by name, for tampering with one of them.
fn query(uri: &str) -> Vec<(String, String)> {
    url::Url::parse(uri)
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn with_param(uri: &str, name: &str, value: &str) -> String {
    let mut url = url::Url::parse(uri).unwrap();
    let pairs = query(uri);
    url.query_pairs_mut().clear().extend_pairs(
        pairs
            .iter()
            .map(|(key, old)| (key.as_str(), if key == name { value } else { old.as_str() })),
    );
    url.into()
}

fn param(uri: &str, name: &str) -> String {
    query(uri)
        .into_iter()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
}

#[test]
fn code_round_trips_through_its_uri() {
    for grant in [grant(None), grant(Some("Kitchen node"))] {
        let code = code(&grant);
        let uri = code.to_uri();
        assert!(
            uri.starts_with("manifold:seat-grant?v=1&env=staging&node="),
            "{uri}"
        );
        let parsed = SeatGrantCode::parse(&uri, ENV).unwrap();
        assert_eq!(parsed, code);
        assert_eq!(parsed.grant(), &grant);
        assert_eq!(parsed.locator(), &locator(&mg_key()));
        assert_eq!(parsed.environment(), ENV);
        // The presented grant is the exact signed bytes from the URI.
        let (payload, _) = parsed.signed_grant().as_parts();
        assert_eq!(URL_SAFE_NO_PAD.encode(payload), param(&uri, "grant"));
        assert!(!param(&uri, "grant").contains('='));
    }
}

#[test]
fn uri_parameters_use_the_specified_encodings() {
    let key = mg_key();
    let uri = code(&grant(None)).to_uri();
    assert_eq!(
        param(&uri, "key"),
        hex::encode(key.x_only_public_key().0.serialize())
    );
    assert_eq!(
        param(&uri, "node"),
        hex::encode(SecretKey::from_bytes(&[4; 32]).public().as_bytes())
    );
    let payload = URL_SAFE_NO_PAD.decode(param(&uri, "grant")).unwrap();
    assert_eq!(
        String::from_utf8(payload.clone()).unwrap(),
        format!(
            r#"{{"grant_nonce":"{}","issued_at":1790000000}}"#,
            "a5".repeat(32)
        )
    );
    let sig = hex::decode(param(&uri, "sig")).unwrap();
    let sig = secp256k1::schnorr::Signature::from_slice(&sig).unwrap();
    SECP256K1
        .verify_schnorr(
            &sig,
            &manager_response_signing_digest("seat_grant", &payload),
            &key.x_only_public_key().0,
        )
        .unwrap();
}

#[test]
fn tampered_payload_signature_or_key_is_refused() {
    let uri = code(&grant(Some("a"))).to_uri();

    // A payload re-encoded with one field changed no longer matches the signature.
    let payload = URL_SAFE_NO_PAD.decode(param(&uri, "grant")).unwrap();
    let tampered = String::from_utf8(payload)
        .unwrap()
        .replace("1790000000", "1790000001");
    let tampered = with_param(&uri, "grant", &URL_SAFE_NO_PAD.encode(tampered));
    assert_eq!(
        SeatGrantCode::parse(&tampered, ENV),
        Err(InvalidSeatGrantCode::BadSignature)
    );

    // A valid signature by the same key over another grant.
    let other = code(&grant(Some("b"))).to_uri();
    let tampered = with_param(&uri, "sig", &param(&other, "sig"));
    assert_eq!(
        SeatGrantCode::parse(&tampered, ENV),
        Err(InvalidSeatGrantCode::BadSignature)
    );

    // Another MG's key.
    let other_key = Keypair::from_seckey_slice(SECP256K1, &[5; 32]).unwrap();
    let tampered = with_param(&uri, "key", &other_key.x_only_public_key().0.to_string());
    assert_eq!(
        SeatGrantCode::parse(&tampered, ENV),
        Err(InvalidSeatGrantCode::BadSignature)
    );
}

#[test]
fn quote_signature_over_the_same_bytes_is_not_a_grant() {
    let key = mg_key();
    let payload = serde_json::to_vec(&grant(None)).unwrap();
    let signature = SECP256K1.sign_schnorr(
        &manager_response_signing_digest("get_quote", &payload),
        &key,
    );
    let signed = SignedResponse::<SeatGrant>::from_parts(payload, ManagerSignature(signature));
    assert_eq!(
        SeatGrantCode::new(ENV, locator(&key), signed),
        Err(InvalidSeatGrantCode::BadSignature)
    );
    assert_eq!(
        <GetQuoteResponse as crate::ManagerSignedResponse>::LABEL,
        "get_quote"
    );
}

#[test]
fn code_for_another_environment_is_refused() {
    let uri = code(&grant(None)).to_uri();
    assert_eq!(
        SeatGrantCode::parse(&uri, ManifoldEnvironment::Production),
        Err(InvalidSeatGrantCode::WrongEnvironment {
            found: ManifoldEnvironment::Staging,
            expected: ManifoldEnvironment::Production,
        })
    );
    let relabeled = with_param(&uri, "env", "production");
    assert_eq!(
        SeatGrantCode::parse(&relabeled, ENV),
        Err(InvalidSeatGrantCode::WrongEnvironment {
            found: ManifoldEnvironment::Production,
            expected: ENV,
        })
    );
    for env in ["", "mainnet"] {
        assert_eq!(
            SeatGrantCode::parse(&with_param(&uri, "env", env), ENV),
            Err(InvalidSeatGrantCode::Malformed)
        );
    }
}

#[test]
fn only_a_well_formed_version_1_uri_parses() {
    let uri = code(&grant(None)).to_uri();
    let malformed = [
        with_param(&uri, "v", "2"),
        with_param(&uri, "v", ""),
        uri.replacen("v=1&", "", 1),
        format!("{uri}&v=1"),
        format!("{uri}&extra=1"),
        uri.replacen("seat-grant", "guardian-link", 1),
        uri.replacen("manifold:", "manifold://", 1),
        uri.replacen("manifold:", "fedi:", 1),
        // The payload is unpadded base64url, not standard base64.
        with_param(&uri, "grant", &format!("{}=", param(&uri, "grant"))),
        with_param(&uri, "sig", "00"),
        with_param(&uri, "node", "zz"),
    ];
    for uri in malformed {
        assert_eq!(
            SeatGrantCode::parse(&uri, ENV),
            Err(InvalidSeatGrantCode::Malformed),
            "{uri}"
        );
    }
}

#[test]
fn label_is_bounded_in_characters() {
    // 32 two-byte characters: 64 bytes but within the character bound.
    let longest = "é".repeat(SEAT_GRANT_LABEL_MAX_CHARS);
    assert_eq!(
        SeatGrantLabel::new(longest.clone()).unwrap().as_str(),
        longest
    );
    assert_eq!(
        SeatGrantLabel::new("x".repeat(SEAT_GRANT_LABEL_MAX_CHARS + 1)),
        Err(InvalidSeatGrantLabel)
    );
    assert_eq!(SeatGrantLabel::new(""), Err(InvalidSeatGrantLabel));

    // A signed payload carrying an overlong label does not parse as a grant.
    let key = mg_key();
    let payload = format!(
        r#"{{"grant_nonce":"{}","issued_at":1,"label":"{}"}}"#,
        "00".repeat(32),
        "x".repeat(SEAT_GRANT_LABEL_MAX_CHARS + 1)
    )
    .into_bytes();
    let signature = SECP256K1.sign_schnorr(
        &manager_response_signing_digest("seat_grant", &payload),
        &key,
    );
    let signed = SignedResponse::<SeatGrant>::from_parts(payload, ManagerSignature(signature));
    assert_eq!(
        SeatGrantCode::new(ENV, locator(&key), signed),
        Err(InvalidSeatGrantCode::Malformed)
    );
}
