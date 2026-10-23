use fedi_decentralized_service_manifold_guardian::{
    FEDERATION_SIZES_0_1, FEDIMINTD_VERSION_0_1, Plan,
};
use nostr::{JsonUtil as _, Keys};
use peerbadge_protocol::{
    Credential, CredentialDigest, CredentialProof, HolderAuthorization,
    HolderAuthorizationStatement, HolderId, IssuerId, ProtocolV1 as SdkProtocolV1,
    SignedCredential, SubjectPubkey, Timestamp as SdkTimestamp,
};

use super::*;

/// The BIP-340 test-vector x-only pubkey for secret `0x...03`, doubling as a
/// syntactically valid commitment-signing service pubkey.
const SERVICE_PUBKEY_HEX: &str = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";

fn payload(keys: &Keys) -> AdvertisementPayload {
    AdvertisementPayload {
        version: ProtocolV1,
        mg_id_pubkey: keys.public_key().to_string(),
        service_pubkey: SERVICE_PUBKEY_HEX.to_owned(),
        issued_at: 1_730_000_000,
        expires_at: 1_730_007_200,
        api_endpoints: vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}endpoint"),
        }],
        availability: Availability {
            fedimintd_version: FEDIMINTD_VERSION_0_1
                .parse()
                .expect("the bundled fedimintd version is valid SemVer"),
            federation_sizes: FEDERATION_SIZES_0_1.to_vec(),
        },
        plans: vec![
            fedi_decentralized_service_manifold_guardian::Plan::InfiniteBestEffort {
                price_msats: 250_000,
            },
        ],
        holder_authorizations: vec![],
    }
}

#[test]
fn advertisement_signature_round_trips_and_pins_the_payload() {
    let keys = Keys::generate();
    let document = sign_advertisement(payload(&keys), &keys).unwrap();
    assert!(
        document.payload.availability.federation_sizes.contains(&8),
        "advertisements include representative custom sizes"
    );
    assert!(
        document.payload.availability.federation_sizes.contains(&20),
        "advertisements include the inclusive custom-size ceiling"
    );
    verify_advertisement_self_signature(&document).unwrap();
    let value = serde_json::to_value(&document).unwrap();
    assert_eq!(
        parse_and_verify_advertisement(&value.to_string()).unwrap(),
        document
    );
    let proof = serde_json::to_value(&document.proof).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "payload": {
                "version": 1,
                "fman_id_pubkey": keys.public_key().to_string(),
                "service_pubkey": SERVICE_PUBKEY_HEX,
                "issued_at": 1_730_000_000_u64,
                "expires_at": 1_730_007_200_u64,
                "api_endpoints": [{
                    "transport": "iroh",
                    "url": "iroh://endpoint",
                }],
                "availability": {
                    "fedimintd_version": FEDIMINTD_VERSION_0_1,
                    "federation_sizes": FEDERATION_SIZES_0_1,
                },
                "plans": [{"InfiniteBestEffort": {"price_msats": 250_000}}],
                "holder_authorizations": [],
            },
            "proof": proof,
        }),
        "kind 37701 must retain its exact payload shape without a per-MG payment list",
    );

    // Any payload change invalidates the proof.
    let mut tampered =
        serde_json::from_str::<AdvertisementDocument>(&serde_json::to_string(&document).unwrap())
            .unwrap();
    tampered.payload.issued_at += 1;
    assert_eq!(
        verify_advertisement_self_signature(&tampered).unwrap_err(),
        AdvertisementDocumentError::InvalidProof
    );

    // Signing under a mismatched pubkey is refused outright.
    assert_eq!(
        sign_advertisement(payload(&Keys::generate()), &keys).unwrap_err(),
        AdvertisementDocumentError::SigningKeyMismatch
    );
}

#[test]
fn advertisement_verification_rejects_a_malformed_pubkey() {
    let keys = Keys::generate();
    let mut document = sign_advertisement(payload(&keys), &keys).unwrap();
    document.payload.mg_id_pubkey = "not-a-pubkey".to_owned();
    assert_eq!(
        verify_advertisement_self_signature(&document).unwrap_err(),
        AdvertisementDocumentError::MalformedMgIdPubkey
    );
}

#[test]
fn unsupported_advertisement_version_is_rejected() {
    let keys = Keys::generate();
    let mut value = serde_json::to_value(payload(&keys)).unwrap();
    value["version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<AdvertisementPayload>(value).is_err());
}

#[test]
fn advertisement_requires_one_typed_fedimintd_version() {
    let keys = Keys::generate();
    let mut value = serde_json::to_value(payload(&keys)).unwrap();
    value["availability"]["fedimintd_version"] = serde_json::json!("not-semver");
    assert!(serde_json::from_value::<AdvertisementPayload>(value).is_err());

    let mut old_shape = serde_json::to_value(payload(&keys)).unwrap();
    old_shape["availability"]
        .as_object_mut()
        .unwrap()
        .remove("fedimintd_version");
    old_shape["availability"]["fedimintd_versions"] = serde_json::json!(["0.11.1+fedi"]);
    assert!(serde_json::from_value::<AdvertisementPayload>(old_shape).is_err());
}

#[test]
fn advertisement_requires_service_pubkey() {
    let keys = Keys::generate();
    let mut value = serde_json::to_value(payload(&keys)).unwrap();
    value.as_object_mut().unwrap().remove("service_pubkey");
    assert!(serde_json::from_value::<AdvertisementPayload>(value).is_err());
}

/// Pinned cross-program fixture (`testing.md`): a fully signed v1 kind-37701
/// advertisement document captured as literal JSON from hardcoded keys
/// (Nostr identity secret `0x...03`, advertised service pubkey from secret
/// `0x...05`). `verify_advertisement_self_signature` must keep accepting
/// these exact bytes; a failure here means the signing convention or the v1
/// payload schema changed in a way that breaks external producers and
/// verifiers.
#[test]
fn pinned_signed_advertisement_fixture_stays_accepted() {
    const FIXTURE: &str = r#"{"payload":{"version":1,"fman_id_pubkey":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","service_pubkey":"2f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4","issued_at":1730000000,"expires_at":1730007200,"api_endpoints":[{"transport":"iroh","url":"iroh://fixture-endpoint"}],"availability":{"fedimintd_version":"0.8.1+fedi","federation_sizes":[7,13]},"plans":[{"InfiniteBestEffort":{"price_msats":250000}}],"holder_authorizations":[]},"proof":{"signature":"3FMOmhFQ0sHinyiy0bnyGzp8xFRYL4lcewDve5fn8bjlS8AMS-RUrVIxMmkIyB_4buZfOC9stXfANFyJGBghew"}}"#;

    let document =
        serde_json::from_str::<AdvertisementDocument>(FIXTURE).expect("pinned fixture parses");
    verify_advertisement_self_signature(&document).expect("pinned fixture stays verifiable");
    assert_eq!(parse_and_verify_advertisement(FIXTURE).unwrap(), document);

    // Literal expectations, not production constants, so a constant change
    // cannot silently move implementation and expectation together.
    assert_eq!(document.payload.version, ProtocolV1);
    assert_eq!(
        document.payload.mg_id_pubkey,
        "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
    );
    assert_eq!(
        document.payload.service_pubkey,
        "2f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4",
    );
    assert_eq!(document.payload.issued_at, 1_730_000_000);
    assert_eq!(document.payload.expires_at, 1_730_007_200);

    // The pinned signature binds the advertised service key.
    let mut tampered_key = document;
    tampered_key.payload.service_pubkey =
        "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9".to_owned();
    assert_eq!(
        verify_advertisement_self_signature(&tampered_key).unwrap_err(),
        AdvertisementDocumentError::InvalidProof,
        "swapping the advertised service pubkey must invalidate the signature",
    );
}

// Literal canonical payload, independently ordered and encoded rather than
// reconstructed through AdvertisementPayload or the production digest helper.
// Includes extensions at the root and in every protocol-owned nested object.
const EXTENDED_CANONICAL_PAYLOAD: &str = r#"{"api_endpoints":[{"future":{"hints":[null,true,"x"]},"transport":"iroh","url":"iroh://endpoint"}],"availability":{"federation_sizes":[7,13],"fedimintd_version":"0.8.1+fedi","future":false},"expires_at":1730007200,"fman_id_pubkey":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","future":[1,0,1e+30,0.002,9007199254740992],"holder_authorizations":[],"issued_at":1730000000,"plans":[{"InfiniteBestEffort":{"future":{"a":null},"price_msats":250000}}],"service_pubkey":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","version":1}"#;

fn extended_wire_document() -> serde_json::Value {
    let keys = Keys::parse(&format!("{:064x}", 3)).unwrap();
    let digest = Sha256::new()
        .chain_update(b"fedi-fman-advertisement/v1\0")
        .chain_update(EXTENDED_CANONICAL_PAYLOAD)
        .finalize();
    // Independently calculated with Python hashlib over the literal above.
    assert_eq!(
        format!("{digest:x}"),
        "6f2b407ab3ae14ac42e2141178b3b21fb1de8ea149b0baf0eb3fb8e32c6e5e84"
    );
    let signature = keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(digest.into()));
    serde_json::json!({
        "payload": serde_json::from_str::<serde_json::Value>(EXTENDED_CANONICAL_PAYLOAD).unwrap(),
        "proof": SchnorrSignatureProof { signature },
    })
}

#[test]
fn complete_extended_payload_is_authenticated_before_projection() {
    let wire = extended_wire_document();
    assert_eq!(
        serde_json_canonicalizer::to_string(&wire["payload"]).unwrap(),
        EXTENDED_CANONICAL_PAYLOAD
    );
    let document = parse_and_verify_advertisement(&wire.to_string()).unwrap();
    assert_eq!(
        document.payload,
        serde_json::from_value::<AdvertisementPayload>(wire["payload"].clone()).unwrap()
    );
    // This fixture would fail with the old lossy verification path.
    assert_eq!(
        verify_advertisement_self_signature(&document),
        Err(AdvertisementDocumentError::InvalidProof)
    );
    for pointer in [
        "/payload/future",
        "/payload/api_endpoints/0/future",
        "/payload/availability/future",
        "/payload/plans/0/InfiniteBestEffort/future",
        "/payload/service_pubkey",
    ] {
        let mut tampered = wire.clone();
        *tampered.pointer_mut(pointer).unwrap() = serde_json::json!("changed");
        assert_eq!(
            parse_and_verify_advertisement(&tampered.to_string()),
            Err(AdvertisementDocumentError::InvalidProof),
            "{pointer}"
        );
    }
    let mut removed = wire.clone();
    removed["payload"].as_object_mut().unwrap().remove("future");
    let mut added = wire.clone();
    added["payload"]["another_extension"] = serde_json::json!(null);
    for tampered in [removed, added] {
        assert_eq!(
            parse_and_verify_advertisement(&tampered.to_string()),
            Err(AdvertisementDocumentError::InvalidProof)
        );
    }

    // Same JCS value despite order, whitespace, escaped names, and equivalent
    // number spellings, including the pinned canonicalizer's f64 rounding.
    let equivalent_payload = EXTENDED_CANONICAL_PAYLOAD
        .replace(r#""version":1"#, r#""\u0076ersion":1"#)
        .replace(
            "[1,0,1e+30,0.002,9007199254740992]",
            "[1.0,-0.0,1E30,2e-3,9007199254740993]",
        );
    let equivalent = format!(
        " {{ \"proof\": {}, \"payload\": {equivalent_payload} }} ",
        wire["proof"]
    );
    assert_eq!(
        parse_and_verify_advertisement(&equivalent).unwrap(),
        document
    );
}

#[test]
fn wire_parser_rejects_ambiguous_or_unbounded_json() {
    let wire = extended_wire_document().to_string();
    let bad_inputs = [
        wire.replace(r#""version":1"#, r#""version":1,"version":1"#),
        wire.replace(r#""future":false"#, r#""future":false,"future":false"#),
        wire.replace(r#""future":false"#, r#""future":false,"\u0066uture":false"#),
        wire.replace(r#""a":null"#, r#""a":null,"a":null"#),
        wire.replace(r#""payload":"#, r#""payload":{},"payload":"#),
        wire.replace(r#""proof":"#, r#""proof":{},"proof":"#),
        wire.replace(r#""future":false"#, r#""future":1e9999"#),
        wire.replace(r#""future":false"#, r#""future":NaN"#),
        wire.replace(
            r#""future":false"#,
            &format!(r#""future":{}0{}"#, "[".repeat(128), "]".repeat(128)),
        ),
        format!("{wire} null"),
        format!("{wire}{}", " ".repeat(256 * 1024)),
        "null".to_owned(),
        "[]".to_owned(),
        r#"{"payload":[],"proof":{}}"#.to_owned(),
        r#"{"payload":{},"proof":[]}"#.to_owned(),
        "{".to_owned(),
    ];
    for input in bad_inputs {
        assert_eq!(
            parse_and_verify_advertisement(&input),
            Err(AdvertisementDocumentError::InvalidDocument)
        );
    }
}

#[test]
fn wire_parser_preserves_real_serde_json_marker_objects() {
    let keys = Keys::parse(&format!("{:064x}", 3)).unwrap();
    for marker in [
        "$serde_json::private::Number",
        "$serde_json::private::RawValue",
    ] {
        // Do not construct this through from_str<Value>: under unified serde
        // features that parser itself interprets private-marker objects.
        let canonical = EXTENDED_CANONICAL_PAYLOAD.replace(
            r#""future":false"#,
            &format!(r#""future":{{"{marker}":"1"}}"#),
        );
        let digest = Sha256::new()
            .chain_update(b"fedi-fman-advertisement/v1\0")
            .chain_update(&canonical)
            .finalize()
            .into();
        let proof = serde_json::to_string(&SchnorrSignatureProof {
            signature: keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(digest)),
        })
        .unwrap();
        let wire = format!(r#"{{"payload":{canonical},"proof":{proof}}}"#);
        parse_and_verify_advertisement(&wire).unwrap();
        let tampered = wire.replace(&format!(r#"{{"{marker}":"1"}}"#), "1");
        assert_eq!(
            parse_and_verify_advertisement(&tampered),
            Err(AdvertisementDocumentError::InvalidProof)
        );
    }
}

#[test]
fn wire_parser_retains_the_default_json_container_depth_limit() {
    for depth in [126, 127, 128, 129] {
        let wire = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let ordinary = serde_json::from_str::<serde_json::Value>(&wire);
        let unique = serde_json::from_str::<wire_json::UniqueValue>(&wire);
        assert_eq!(unique.is_ok(), ordinary.is_ok(), "depth {depth}");
        assert_eq!(unique.is_ok(), depth < 128, "depth {depth}");
    }
}

#[test]
fn signed_invalid_known_fields_remain_schema_errors() {
    let keys = Keys::parse(&format!("{:064x}", 3)).unwrap();
    let original = extended_wire_document();
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("issued_at", serde_json::json!("yesterday")),
        ("api_endpoints", serde_json::json!({})),
        ("plans", serde_json::json!([{"Unknown": {}}])),
        ("service_pubkey", serde_json::Value::Null),
    ] {
        let mut wire = original.clone();
        if value.is_null() {
            wire["payload"].as_object_mut().unwrap().remove(field);
        } else {
            wire["payload"][field] = value;
        }
        // Sign the complete invalid payload, not a typed projection.
        let canonical = serde_json_canonicalizer::to_vec(&wire["payload"]).unwrap();
        let digest = Sha256::new()
            .chain_update(b"fedi-fman-advertisement/v1\0")
            .chain_update(canonical)
            .finalize()
            .into();
        wire["proof"] = serde_json::to_value(SchnorrSignatureProof {
            signature: keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(digest)),
        })
        .unwrap();
        assert_eq!(
            parse_and_verify_advertisement(&wire.to_string()),
            Err(AdvertisementDocumentError::InvalidDocument),
            "{field}"
        );
    }
    for proof in [
        serde_json::json!({}),
        serde_json::json!({"signature": "bad"}),
    ] {
        let mut wire = original.clone();
        wire["proof"] = proof;
        assert_eq!(
            parse_and_verify_advertisement(&wire.to_string()),
            Err(AdvertisementDocumentError::InvalidDocument)
        );
    }
}

/// One structurally complete holder-authorization envelope at realistic
/// worst-case size: real Schnorr signatures and digests, plus an RSA
/// credential-proof filler matching the SDK's 2048-bit issuer modulus
/// (`ISSUER_MODULUS_BITS`), whose real PBRSA signature is 256 bytes.
fn worst_case_envelope(
    holder: &Keys,
    issuer: &Keys,
    subject: nostr::PublicKey,
) -> HolderAuthorizationEnvelope {
    let credential = Credential {
        issuer_id_pubkey: IssuerId(issuer.public_key()),
        info: serde_json::json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": 10,
        }),
        blind_msg: serde_json::json!(holder.public_key().to_string()),
    };
    let statement = HolderAuthorizationStatement {
        holder_id_pubkey: HolderId(holder.public_key()),
        subject_pubkey: SubjectPubkey(subject),
        credential_digest: CredentialDigest(
            credential.digest().expect("worst-case credential digests"),
        ),
        issued_at: SdkTimestamp(1_730_000_000),
    };
    let signature = holder.sign_schnorr(&nostr::secp256k1::Message::from_digest(
        statement
            .digest()
            .expect("worst-case statement digests")
            .into(),
    ));
    HolderAuthorizationEnvelope {
        holder_authorization: HolderAuthorization {
            version: SdkProtocolV1,
            authorization: statement,
            proof: SchnorrSignatureProof { signature },
        },
        signed_credential: SignedCredential {
            version: SdkProtocolV1,
            credential,
            proof: CredentialProof {
                signature: blind_rsa_signatures::Signature(vec![0xAB; 256]),
            },
        },
    }
}

/// Measure a worst-case *legitimate* kind-37701 advertisement event and pin
/// it under the shared 256 KiB per-event relay cap.
///
/// Every field is filled to a generous realistic maximum: 8 long iroh
/// endpoint URLs, one long fedimintd version, 64 federation sizes,
/// 4 plans with verbose prices, and 4 complete holder-authorization
/// envelopes (`MG_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS` examines at
/// most 4). Measured size at the time of writing: 7299 bytes (~7.1 KiB) —
/// roughly 3% of the 256 KiB cap. The consumers' per-event cap
/// (`ROLE_FETCHED_EVENT_MAX_BYTES` in `crates/nostr-clients`) should be
/// tightened toward this measurement, with margin, in a follow-up; see the
/// TODO beside `MG_ADVERTISEMENTS_RETAINED_MAX_BYTES` in
/// `crates/nostr-clients/src/fa.rs`.
#[test]
fn worst_case_advertisement_event_fits_the_per_event_cap() {
    // Restated literally: `ROLE_FETCHED_EVENT_MAX_BYTES` lives in
    // `crates/nostr-clients`, which depends on this crate, so importing it
    // here would invert the dependency.
    const ROLE_FETCHED_EVENT_MAX_BYTES: usize = 256 * 1024;

    let mg = Keys::generate();
    let holder = Keys::generate();
    let issuer = Keys::generate();
    let payload = AdvertisementPayload {
        version: ProtocolV1,
        mg_id_pubkey: mg.public_key().to_string(),
        service_pubkey: SERVICE_PUBKEY_HEX.to_owned(),
        issued_at: 1_730_000_000,
        expires_at: 1_730_007_200,
        api_endpoints: (0..8)
            .map(|index| ApiEndpoint {
                transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
                url: format!(
                    "{IROH_API_ENDPOINT_URL_SCHEME}{index:064x}?relay=https://use1-1.relay.example.iroh.network/&alpn=fedi-fman-fi-setup/1"
                ),
            })
            .collect(),
        availability: Availability {
            fedimintd_version: "0.11.1-fedi17+fedi"
                .parse()
                .expect("worst-case version is valid SemVer"),
            federation_sizes: (1..=64).collect(),
        },
        plans: vec![
            Plan::InfiniteBestEffort {
                price_msats: 18446744073709551615,
            },
            Plan::SubscriptionBased {
                initial_price_msats: 18446744073709551615,
                renewal_price_msats: 18446744073709551615,
                period: "every-30-days".to_owned(),
            },
            Plan::InfiniteBestEffort {
                price_msats: 21000000,
            },
        ],
        holder_authorizations: (0..4)
            .map(|_| worst_case_envelope(&holder, &issuer, mg.public_key()))
            .collect(),
    };
    let document = sign_advertisement(payload, &mg).expect("worst-case advertisement signs");
    let content = serde_json::to_string(&document).expect("worst-case advertisement serializes");
    assert_eq!(parse_and_verify_advertisement(&content).unwrap(), document);
    let event = nostr::EventBuilder::new(nostr::Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND), content)
        .tag(nostr::Tag::identifier(MG_ADVERTISEMENT_D_TAG))
        .tag(nostr::Tag::hashtag(MG_ADVERTISEMENT_HASHTAG))
        .sign_with_keys(&mg)
        .expect("worst-case advertisement event signs");

    let event_bytes = event.as_json().len();
    println!("worst-case legitimate advertisement event: {event_bytes} bytes");
    assert!(
        event_bytes < ROLE_FETCHED_EVENT_MAX_BYTES,
        "a worst-case legitimate advertisement event ({event_bytes} bytes) must fit \
         the shared 256 KiB per-event relay cap",
    );
}
