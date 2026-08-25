use std::path::PathBuf;
#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use super::*;
use crate::BitcoindInfo;
use crate::{
    ApiError, ApiErrorKind, NostrRelayInfo, PushGatewayInfo, Request, ResourceDescriptor,
    ResourceHandleId, ResourceLease, ResourceRequest, Response,
};

#[test]
fn request_round_trip() {
    let request = Request::Allocate(ResourceRequest::NostrRelay(
        crate::NostrRelayRequest::shared(),
    ));

    let frame = encode_frame(&request).expect("encode request");
    let decoded: Request = decode_frame(&frame).expect("decode request");

    assert_eq!(decoded, request);
}

#[test]
fn response_round_trip() {
    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(42),
        descriptor: ResourceDescriptor::NostrRelay(NostrRelayInfo {
            url: "ws://127.0.0.1:7777".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 7777,
            data_dir: PathBuf::from("/tmp/defe/relay/db"),
        }),
    });

    let frame = encode_frame(&response).expect("encode response");
    let decoded: Response = decode_frame(&frame).expect("decode response");

    assert_eq!(decoded, response);
}

#[test]
fn error_round_trip() {
    let response = Response::Error(ApiError::new(
        ApiErrorKind::ProtocolDecodeError,
        "malformed request frame",
    ));

    let frame = encode_frame(&response).expect("encode error response");
    let decoded: Response = decode_frame(&frame).expect("decode error response");

    assert_eq!(decoded, response);
}

#[test]
fn request_wire_format_matches_golden_bytes() {
    let request = Request::Allocate(ResourceRequest::NostrRelay(
        crate::NostrRelayRequest::shared(),
    ));

    let frame = encode_frame(&request).expect("encode request");

    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0x26, 0xa1, 0x68, 0x41, 0x6c, 0x6c, 0x6f, 0x63, 0x61, 0x74, 0x65,
            0xa1, 0x6a, 0x4e, 0x6f, 0x73, 0x74, 0x72, 0x52, 0x65, 0x6c, 0x61, 0x79, 0xa1, 0x67,
            0x73, 0x68, 0x61, 0x72, 0x69, 0x6e, 0x67, 0x66, 0x53, 0x68, 0x61, 0x72, 0x65, 0x64,
        ]
    );
}

#[test]
fn release_request_wire_format_matches_golden_bytes() {
    let request = Request::Release(ResourceHandleId(42));

    let frame = encode_frame(&request).expect("encode release request");

    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0x0b, 0xa1, 0x67, 0x52, 0x65, 0x6c, 0x65, 0x61, 0x73, 0x65, 0x18,
            0x2a,
        ]
    );
}

#[test]
fn resource_response_wire_format_matches_golden_bytes() {
    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(42),
        descriptor: ResourceDescriptor::NostrRelay(NostrRelayInfo {
            url: "ws://127.0.0.1:7777".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 7777,
            data_dir: PathBuf::from("/tmp/defe/relay/db"),
        }),
    });

    let frame = encode_frame(&response).expect("encode resource response");

    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0x7a, 0xa1, 0x68, 0x52, 0x65, 0x73, 0x6f, 0x75, 0x72, 0x63, 0x65,
            0xa2, 0x69, 0x68, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x5f, 0x69, 0x64, 0x18, 0x2a, 0x6a,
            0x64, 0x65, 0x73, 0x63, 0x72, 0x69, 0x70, 0x74, 0x6f, 0x72, 0xa1, 0x6a, 0x4e, 0x6f,
            0x73, 0x74, 0x72, 0x52, 0x65, 0x6c, 0x61, 0x79, 0xa4, 0x63, 0x75, 0x72, 0x6c, 0x73,
            0x77, 0x73, 0x3a, 0x2f, 0x2f, 0x31, 0x32, 0x37, 0x2e, 0x30, 0x2e, 0x30, 0x2e, 0x31,
            0x3a, 0x37, 0x37, 0x37, 0x37, 0x64, 0x68, 0x6f, 0x73, 0x74, 0x69, 0x31, 0x32, 0x37,
            0x2e, 0x30, 0x2e, 0x30, 0x2e, 0x31, 0x64, 0x70, 0x6f, 0x72, 0x74, 0x19, 0x1e, 0x61,
            0x68, 0x64, 0x61, 0x74, 0x61, 0x5f, 0x64, 0x69, 0x72, 0x72, 0x2f, 0x74, 0x6d, 0x70,
            0x2f, 0x64, 0x65, 0x66, 0x65, 0x2f, 0x72, 0x65, 0x6c, 0x61, 0x79, 0x2f, 0x64, 0x62,
        ]
    );
}

#[test]
fn error_response_wire_format_matches_golden_bytes() {
    let response = Response::Error(ApiError::new(
        ApiErrorKind::ProtocolDecodeError,
        "malformed request frame",
    ));

    let frame = encode_frame(&response).expect("encode error response");

    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0x41, 0xa1, 0x65, 0x45, 0x72, 0x72, 0x6f, 0x72, 0xa2, 0x64, 0x6b,
            0x69, 0x6e, 0x64, 0x73, 0x50, 0x72, 0x6f, 0x74, 0x6f, 0x63, 0x6f, 0x6c, 0x44, 0x65,
            0x63, 0x6f, 0x64, 0x65, 0x45, 0x72, 0x72, 0x6f, 0x72, 0x67, 0x6d, 0x65, 0x73, 0x73,
            0x61, 0x67, 0x65, 0x77, 0x6d, 0x61, 0x6c, 0x66, 0x6f, 0x72, 0x6d, 0x65, 0x64, 0x20,
            0x72, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74, 0x20, 0x66, 0x72, 0x61, 0x6d, 0x65,
        ]
    );
}

#[cfg(unix)]
#[test]
fn nostr_relay_data_dir_requires_utf8_for_serialization() {
    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(42),
        descriptor: ResourceDescriptor::NostrRelay(NostrRelayInfo {
            url: "ws://127.0.0.1:7777".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 7777,
            data_dir: PathBuf::from(OsString::from_vec(b"/tmp/defe/relay/\xff".to_vec())),
        }),
    });

    let err = encode_frame(&response).expect_err("non-UTF-8 paths do not serialize");

    assert!(matches!(err, FrameError::Encode(_)));
}

#[test]
fn rejects_incomplete_length_prefix() {
    let err = decode_frame::<Request>(&[0, 0, 0]).expect_err("frame is incomplete");

    assert!(matches!(
        err,
        FrameError::IncompleteLengthPrefix { size: 3 }
    ));
}

#[test]
fn rejects_incomplete_payload() {
    let err = decode_frame::<Request>(&[0, 0, 0, 8, 0xaa]).expect_err("payload is incomplete");

    assert!(matches!(
        err,
        FrameError::IncompletePayload {
            expected: 8,
            actual: 1,
        }
    ));
}

#[test]
fn rejects_malformed_cbor() {
    let err = decode_frame::<Request>(&[0, 0, 0, 1, 0xff]).expect_err("CBOR is malformed");

    assert!(matches!(err, FrameError::Decode(_)));
}

#[test]
fn rejects_oversize_frame_before_reading_payload() {
    let len = (MAX_FRAME_SIZE + 1) as u32;
    let frame = len.to_be_bytes();

    let err = decode_frame::<Request>(&frame).expect_err("frame is too large");

    assert!(matches!(
        err,
        FrameError::FrameTooLarge {
            size,
            max: MAX_FRAME_SIZE,
        } if size == MAX_FRAME_SIZE + 1
    ));
}

#[test]
fn rejects_trailing_frame_bytes() {
    let mut frame = encode_frame(&Request::Ping).expect("encode request");
    frame.push(0);

    let err = decode_frame::<Request>(&frame).expect_err("trailing bytes are rejected");

    assert!(matches!(err, FrameError::TrailingBytes { trailing: 1 }));
}

#[test]
fn push_gateway_request_wire_format_matches_golden_bytes() {
    let request = Request::Allocate(ResourceRequest::PushGateway(
        crate::PushGatewayRequest::exclusive(),
    ));

    let frame = encode_frame(&request).expect("encode push gateway request");
    let decoded: Request = decode_frame(&frame).expect("decode push gateway request");

    assert_eq!(decoded, request);
    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0x2a, 0xa1, 0x68, 0x41, 0x6c, 0x6c, 0x6f, 0x63, 0x61, 0x74, 0x65,
            0xa1, 0x6b, 0x50, 0x75, 0x73, 0x68, 0x47, 0x61, 0x74, 0x65, 0x77, 0x61, 0x79, 0xa1,
            0x67, 0x73, 0x68, 0x61, 0x72, 0x69, 0x6e, 0x67, 0x69, 0x45, 0x78, 0x63, 0x6c, 0x75,
            0x73, 0x69, 0x76, 0x65,
        ]
    );
}

#[test]
fn push_gateway_resource_response_wire_format_matches_golden_bytes() {
    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(43),
        descriptor: ResourceDescriptor::PushGateway(PushGatewayInfo {
            url: "http://127.0.0.1:8888".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 8888,
            app_id: "test-app".to_owned(),
            database_path: PathBuf::from("/tmp/defe/push/push-gateway.sqlite"),
        }),
    });

    let frame = encode_frame(&response).expect("encode push gateway resource response");
    let decoded: Response = decode_frame(&frame).expect("decode push gateway resource response");

    assert_eq!(decoded, response);
    assert_eq!(
        frame,
        [
            0x00, 0x00, 0x00, 0xa3, 0xa1, 0x68, 0x52, 0x65, 0x73, 0x6f, 0x75, 0x72, 0x63, 0x65,
            0xa2, 0x69, 0x68, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x5f, 0x69, 0x64, 0x18, 0x2b, 0x6a,
            0x64, 0x65, 0x73, 0x63, 0x72, 0x69, 0x70, 0x74, 0x6f, 0x72, 0xa1, 0x6b, 0x50, 0x75,
            0x73, 0x68, 0x47, 0x61, 0x74, 0x65, 0x77, 0x61, 0x79, 0xa5, 0x63, 0x75, 0x72, 0x6c,
            0x75, 0x68, 0x74, 0x74, 0x70, 0x3a, 0x2f, 0x2f, 0x31, 0x32, 0x37, 0x2e, 0x30, 0x2e,
            0x30, 0x2e, 0x31, 0x3a, 0x38, 0x38, 0x38, 0x38, 0x64, 0x68, 0x6f, 0x73, 0x74, 0x69,
            0x31, 0x32, 0x37, 0x2e, 0x30, 0x2e, 0x30, 0x2e, 0x31, 0x64, 0x70, 0x6f, 0x72, 0x74,
            0x19, 0x22, 0xb8, 0x66, 0x61, 0x70, 0x70, 0x5f, 0x69, 0x64, 0x68, 0x74, 0x65, 0x73,
            0x74, 0x2d, 0x61, 0x70, 0x70, 0x6d, 0x64, 0x61, 0x74, 0x61, 0x62, 0x61, 0x73, 0x65,
            0x5f, 0x70, 0x61, 0x74, 0x68, 0x78, 0x22, 0x2f, 0x74, 0x6d, 0x70, 0x2f, 0x64, 0x65,
            0x66, 0x65, 0x2f, 0x70, 0x75, 0x73, 0x68, 0x2f, 0x70, 0x75, 0x73, 0x68, 0x2d, 0x67,
            0x61, 0x74, 0x65, 0x77, 0x61, 0x79, 0x2e, 0x73, 0x71, 0x6c, 0x69, 0x74, 0x65,
        ]
    );
}

#[cfg(unix)]
#[test]
fn push_gateway_database_path_requires_utf8_for_serialization() {
    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(42),
        descriptor: ResourceDescriptor::PushGateway(PushGatewayInfo {
            url: "http://127.0.0.1:8888".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 8888,
            app_id: "test-app".to_owned(),
            database_path: PathBuf::from(OsString::from_vec(b"/tmp/defe/push/\xff".to_vec())),
        }),
    });

    let err = encode_frame(&response).expect_err("non-UTF-8 paths do not serialize");

    assert!(matches!(err, FrameError::Encode(_)));
}

#[test]
fn bitcoind_request_and_response_round_trip() {
    let request = Request::Allocate(ResourceRequest::Bitcoind(crate::BitcoindRequest::shared()));
    let frame = encode_frame(&request).expect("encode bitcoind request");
    let decoded: Request = decode_frame(&frame).expect("decode bitcoind request");
    assert_eq!(decoded, request);

    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(44),
        descriptor: ResourceDescriptor::Bitcoind(BitcoindInfo {
            rpc_url: "http://127.0.0.1:18443".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "user".to_owned(),
            rpc_password: "password".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/bitcoind"),
        }),
    });
    let frame = encode_frame(&response).expect("encode bitcoind response");
    let decoded: Response = decode_frame(&frame).expect("decode bitcoind response");
    assert_eq!(decoded, response);
}

#[test]
fn fman_request_and_response_round_trip() {
    let request = Request::Allocate(ResourceRequest::Fman(crate::FmanRequest {
        sharing: crate::SharingMode::Exclusive,
        bitcoind: BitcoindInfo {
            rpc_url: "http://127.0.0.1:18443".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin".to_owned(),
            rpc_password: "bitcoin".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/bitcoind"),
        },
        nostr_relay_url: "ws://127.0.0.1:7777".to_owned(),
        first_port_base: 34000,
        iroh_connect_overrides: "routes".to_owned(),
    }));
    let decoded: Request = decode_frame(&encode_frame(&request).expect("encode FMan request"))
        .expect("decode FMan request");
    assert_eq!(decoded, request);

    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(45),
        descriptor: ResourceDescriptor::Fman(crate::FmanInfo {
            locator: "locator".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/fman"),
            iroh_connect_overrides: "routes".to_owned(),
            admin_url: "http://127.0.0.1:10015".to_owned(),
            admin_password: "operator-password".to_owned(),
        }),
    });
    let decoded: Response = decode_frame(&encode_frame(&response).expect("encode FMan response"))
        .expect("decode FMan response");
    assert_eq!(decoded, response);
}

#[test]
fn flip_request_and_response_round_trip() {
    let request = Request::Allocate(ResourceRequest::Flip(crate::FlipRequest {
        sharing: crate::SharingMode::Exclusive,
        iroh_connect_overrides: Some("routes".to_owned()),
    }));
    let decoded: Request = decode_frame(&encode_frame(&request).expect("encode FLIP request"))
        .expect("decode FLIP request");
    assert_eq!(decoded, request);

    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(46),
        descriptor: ResourceDescriptor::Flip(crate::FlipInfo {
            admin_url: "http://127.0.0.1:8173".to_owned(),
            admin_token: "token".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/flip"),
            trust_fixtures_dir: PathBuf::from("/tmp/defe/flip/trust-fixtures"),
            provider_pubkey_hex: "provider".to_owned(),
        }),
    });
    let decoded: Response = decode_frame(&encode_frame(&response).expect("encode FLIP response"))
        .expect("decode FLIP response");
    assert_eq!(decoded, response);
}

#[test]
fn gatewayd_request_and_response_round_trip() {
    let request = Request::Allocate(ResourceRequest::Gatewayd(crate::GatewaydRequest {
        sharing: crate::SharingMode::Shared,
        bitcoind: BitcoindInfo {
            rpc_url: "http://127.0.0.1:18443".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin".to_owned(),
            rpc_password: "bitcoin".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/bitcoind"),
        },
        iroh_connect_overrides: None,
    }));
    let decoded: Request = decode_frame(&encode_frame(&request).expect("encode gatewayd request"))
        .expect("decode gatewayd request");
    assert_eq!(decoded, request);

    let response = Response::Resource(ResourceLease {
        handle_id: ResourceHandleId(47),
        descriptor: ResourceDescriptor::Gatewayd(crate::GatewaydInfo {
            api_url: "http://127.0.0.1:8173".to_owned(),
            password: "password".to_owned(),
        }),
    });
    let decoded: Response =
        decode_frame(&encode_frame(&response).expect("encode gatewayd response"))
            .expect("decode gatewayd response");
    assert_eq!(decoded, response);
}
