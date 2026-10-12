//! Byte-for-byte interoperability fixtures for the TypeScript client.
//!
//! Regenerate intentionally with:
//! `PEERBADGE_UPDATE_GOLDEN=1 cargo test -p fedi-decentralized-service-peerbadge-signing --test golden`
//! Then run without the variable and review the fixture diff. SDK JSON strings
//! below are opaque codec samples, not cryptographically valid SDK documents.

use std::fmt::{Debug, Write as _};
use std::path::PathBuf;

use ciborium::Value as CborValue;
use fedi_decentralized_service_peerbadge_signing::{
    ChallengeRequest, ChallengeResponse, OpenSessionRequest, OpenSessionResponse,
    RedeemSessionRequest, RedeemSessionResponse, SessionState, SessionStatusRequest,
    SessionStatusResponse, SigningError, challenge_digest,
};
use fedi_iroh_rpc::{RequestFrame, ResponseFrame, RpcError, WIRE_VERSION};
use serde::{Serialize, de::DeserializeOwned};

const EXPIRES_AT: u64 = 1_700_000_060;

fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).expect("encode fixed fixture");
    bytes
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> T {
    ciborium::from_reader(bytes).expect("decode fixed fixture")
}

fn golden_bytes(name: &str, bytes: &[u8]) {
    let mut expected = String::with_capacity(bytes.len() * 2 + 1);
    for byte in bytes {
        write!(&mut expected, "{byte:02x}").expect("write hex to string");
    }
    expected.push('\n');
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.hex"));
    if std::env::var_os("PEERBADGE_UPDATE_GOLDEN").as_deref() == Some(std::ffi::OsStr::new("1")) {
        std::fs::write(&path, &expected).expect("write golden fixture");
    }
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing fixture {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "{} differs\ncommitted hex: {actual}\nencoded hex: {expected}\n\
         Diagnose the encoding difference before regenerating fixtures",
        path.display(),
    );
}

fn request<T>(method: &str, value: T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    // These are the production frames, including their serde_bytes attributes;
    // a duplicate local frame definition would miss transport encoding drift.
    let frame = RequestFrame::new(method, encode(&value));
    let bytes = encode(&frame);
    golden_bytes(&format!("request_{method}"), &bytes);
    let decoded: RequestFrame = decode(&bytes);
    assert_eq!(decoded.version, WIRE_VERSION);
    assert_eq!(decoded.method, method);
    assert_eq!(decode::<T>(&decoded.body), value);
    assert_eq!(encode(&decoded), bytes);
}

fn response<T>(name: &str, value: Result<T, SigningError>)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let bytes = encode(&ResponseFrame::Service(encode(&value)));
    golden_bytes(name, &bytes);
    let decoded: ResponseFrame = decode(&bytes);
    let ResponseFrame::Service(body) = &decoded else {
        panic!("service response decoded as transport failure");
    };
    assert_eq!(decode::<Result<T, SigningError>>(body), value);
    assert_eq!(encode(&decoded), bytes);
}

#[test]
fn request_frames() {
    request("challenge", ChallengeRequest {});
    request(
        "open_session",
        OpenSessionRequest {
            signer_pubkey: "11".repeat(32),
            level: 9,
            nonce: (0..32).collect(),
            signature: (64..128).collect(),
        },
    );
    request(
        "redeem_session",
        RedeemSessionRequest {
            session_id: "33".repeat(32),
            request: r#"{"fixture":"issuance-request"}"#.to_owned(),
        },
    );
    request(
        "session_status",
        SessionStatusRequest {
            session_id: "33".repeat(32),
        },
    );
}

#[test]
fn service_response_frames() {
    response(
        "response_challenge",
        Ok(ChallengeResponse {
            nonce: (0..32).collect(),
            expires_at: EXPIRES_AT,
            issuer_id_pubkey: "22".repeat(32),
        }),
    );
    response(
        "response_open_session",
        Ok(OpenSessionResponse {
            session_id: "33".repeat(32),
            // Reverse insertion order must still match the committed wire bytes.
            info: serde_json::from_str(r#"{"trust_level":9,"schema":"fedi-trust-score-v1.0"}"#)
                .unwrap(),
            issuer_authority: r#"{"fixture":"issuer-authority"}"#.to_owned(),
            expires_at: EXPIRES_AT + 540,
        }),
    );
    response(
        "response_redeem_session",
        Ok(RedeemSessionResponse {
            response: r#"{"fixture":"issuance-response"}"#.to_owned(),
        }),
    );
    for (name, state) in [
        ("response_session_status_open", SessionState::Open),
        ("response_session_status_redeemed", SessionState::Redeemed),
        ("response_session_status_expired", SessionState::Expired),
    ] {
        response(name, Ok(SessionStatusResponse { state }));
    }
}

fn info_response(info: serde_json::Value) -> OpenSessionResponse {
    OpenSessionResponse {
        session_id: "33".repeat(32),
        info,
        issuer_authority: r#"{"fixture":"issuer-authority"}"#.to_owned(),
        expires_at: EXPIRES_AT + 540,
    }
}

fn assert_info_wire(info: serde_json::Value, expected: CborValue) -> Vec<u8> {
    let response = info_response(info);
    let bytes = encode(&response);
    let decoded: OpenSessionResponse = decode(&bytes);
    assert_eq!(decoded, response);
    assert_eq!(encode(&decoded), bytes);
    let CborValue::Map(fields) = decode(&bytes) else {
        panic!("response must be a CBOR map");
    };
    let (_, actual) = fields
        .into_iter()
        .find(|(key, _)| key == &CborValue::Text("info".to_owned()))
        .expect("response info field");
    assert_eq!(actual, expected);
    bytes
}

#[test]
fn info_uses_native_cbor_values() {
    for (json, expected) in [
        ("null", CborValue::Null),
        ("true", CborValue::Bool(true)),
        ("false", CborValue::Bool(false)),
        (r#""text""#, CborValue::Text("text".to_owned())),
        ("0", CborValue::Integer(0.into())),
        ("9", CborValue::Integer(9.into())),
        ("-1", CborValue::Integer((-1).into())),
        ("-9223372036854775808", CborValue::Integer(i64::MIN.into())),
        ("9223372036854775807", CborValue::Integer(i64::MAX.into())),
        ("18446744073709551615", CborValue::Integer(u64::MAX.into())),
        ("1.5", CborValue::Float(1.5)),
        ("[]", CborValue::Array(vec![])),
        ("{}", CborValue::Map(vec![])),
        (
            "[9,-1,1.5,true,null]",
            CborValue::Array(vec![
                CborValue::Integer(9.into()),
                CborValue::Integer((-1).into()),
                CborValue::Float(1.5),
                CborValue::Bool(true),
                CborValue::Null,
            ]),
        ),
    ] {
        assert_info_wire(serde_json::from_str(json).unwrap(), expected);
    }
}

#[test]
fn info_sorts_nested_object_keys_independent_of_insertion_order() {
    let nested = CborValue::Map(vec![
        (
            CborValue::Text("aa".to_owned()),
            CborValue::Integer(1.into()),
        ),
        (
            CborValue::Text("b".to_owned()),
            CborValue::Integer(2.into()),
        ),
    ]);
    let expected = CborValue::Map(vec![
        (CborValue::Text("aa".to_owned()), nested.clone()),
        (
            CborValue::Text("b".to_owned()),
            CborValue::Array(vec![nested]),
        ),
    ]);
    // "aa" precedes "b" lexically, not by encoded CBOR key length.
    let reversed = serde_json::from_str(r#"{"b":[{"b":2,"aa":1}],"aa":{"b":2,"aa":1}}"#).unwrap();
    let sorted = serde_json::from_str(r#"{"aa":{"aa":1,"b":2},"b":[{"aa":1,"b":2}]}"#).unwrap();
    assert_eq!(
        assert_info_wire(reversed, expected.clone()),
        assert_info_wire(sorted, expected),
    );
}

#[test]
fn info_rejects_integers_outside_native_range_without_rounding() {
    // These values only exist when arbitrary_precision is feature-unified in.
    for number in [
        serde_json::Number::from_i128(i128::from(i64::MIN) - 1),
        serde_json::Number::from_u128(u128::from(u64::MAX) + 1),
    ]
    .into_iter()
    .flatten()
    {
        let response = info_response(serde_json::json!({"nested": [number]}));
        let error = ciborium::into_writer(&response, Vec::new())
            .expect_err("out-of-range integers must not silently become floats");
        assert!(error.to_string().contains("unsupported info number"));
    }
}

#[test]
fn transport_response_frame() {
    let frame = ResponseFrame::Transport("request too large".to_owned());
    let bytes = encode(&frame);
    golden_bytes("response_transport", &bytes);
    let decoded: ResponseFrame = decode(&bytes);
    assert!(matches!(
        &decoded,
        ResponseFrame::Transport(message) if message == "request too large"
    ));
    assert_eq!(encode(&decoded), bytes);
}

#[test]
fn every_signing_error_result() {
    for (name, error) in [
        ("unauthorized", SigningError::Unauthorized),
        (
            "level_not_allowed",
            SigningError::LevelNotAllowed { max: 5 },
        ),
        ("bad_challenge", SigningError::BadChallenge),
        ("rate_limited", SigningError::RateLimited),
        ("session_not_found", SigningError::SessionNotFound),
        ("session_expired", SigningError::SessionExpired),
        (
            "session_already_redeemed",
            SigningError::SessionAlreadyRedeemed,
        ),
        (
            "invalid_request",
            SigningError::InvalidRequest("invalid issuance request".to_owned()),
        ),
        (
            "transport",
            SigningError::Transport("iroh transport failure".to_owned()),
        ),
    ] {
        let result: Result<ChallengeResponse, SigningError> = Err(error.clone());
        golden_bytes(&format!("result_error_{name}"), &encode(&result));
        response::<ChallengeResponse>(&format!("response_error_{name}"), Err(error));
    }
}

#[test]
fn challenge_digest_cross_language_vector() {
    let nonce = std::array::from_fn(|index| index as u8);
    let issuer = [0x22; 32];
    let digest = challenge_digest(&nonce, 9, &issuer);
    golden_bytes("challenge_digest", &digest);

    let mut changed_nonce = nonce;
    changed_nonce[0] ^= 1;
    assert_ne!(challenge_digest(&changed_nonce, 9, &issuer), digest);
    assert_ne!(challenge_digest(&nonce, 8, &issuer), digest);
    assert_ne!(challenge_digest(&nonce, 9, &[0x23; 32]), digest);
}

#[test]
fn rpc_errors_do_not_expose_untrusted_diagnostics() {
    for error in [
        RpcError::RequestTooLarge,
        RpcError::RequestTimedOut,
        RpcError::ResponseTooLarge,
        RpcError::UnknownMethod("private request content".to_owned()),
        RpcError::Encode("private request content".to_owned()),
        RpcError::Decode("private request content".to_owned()),
        RpcError::Remote("private request content".to_owned()),
        RpcError::Iroh("private request content".to_owned()),
    ] {
        let error = SigningError::from(error);
        let SigningError::Transport(message) = &error else {
            panic!("RPC error must map to Transport");
        };
        assert!(!message.contains("private request content"));
        assert!(!error.to_string().contains("private request content"));
    }
    for error in [
        SigningError::InvalidRequest("private request content".to_owned()),
        SigningError::Transport("private request content".to_owned()),
    ] {
        assert!(!error.to_string().contains("private request content"));
    }
    let request = RedeemSessionRequest {
        session_id: "bearer session".to_owned(),
        request: "private request content".to_owned(),
    };
    let diagnostic = format!("{request:?}");
    assert!(!diagnostic.contains(&request.request));
    assert!(!diagnostic.contains(&request.session_id));
}
