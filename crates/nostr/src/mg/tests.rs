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
/// it under the shared 64 KiB per-event relay cap.
///
/// Every field is filled to a generous realistic maximum: 8 long iroh
/// endpoint URLs, one long fedimintd version, 64 federation sizes,
/// 4 plans with verbose prices, and 4 complete holder-authorization
/// envelopes (`MG_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS` examines at
/// most 4). Measured size at the time of writing: 7299 bytes (~7.1 KiB) —
/// roughly 11% of the 64 KiB cap. The consumers' per-event cap
/// (`ROLE_FETCHED_EVENT_MAX_BYTES` in `crates/nostr-clients`) should be
/// tightened toward this measurement, with margin, in a follow-up; see the
/// TODO beside `MG_ADVERTISEMENTS_RETAINED_MAX_BYTES` in
/// `crates/nostr-clients/src/fa.rs`.
#[test]
fn worst_case_advertisement_event_fits_the_per_event_cap() {
    // Restated literally: `ROLE_FETCHED_EVENT_MAX_BYTES` lives in
    // `crates/nostr-clients`, which depends on this crate, so importing it
    // here would invert the dependency.
    const ROLE_FETCHED_EVENT_MAX_BYTES: usize = 64 * 1024;

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
         the shared 64 KiB per-event relay cap",
    );
}

fn v2_payload() -> AdvertisementV2Payload {
    let mut payload = AdvertisementV2Payload::from(payload(&Keys::generate()));
    payload.uptime_permille = Some(AdvertisedUptime {
        day: Permille::try_from(998).unwrap(),
        week: Permille::try_from(991).unwrap(),
        month: Permille::try_from(964).unwrap(),
    });
    payload
}

/// Sign `payload` exactly as written, computing the signature here rather
/// than through the code under test: `SHA256(tag || payload)`.
fn hand_signed_v2_content(payload: &str, keys: &Keys, tag: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::new()
        .chain_update(tag)
        .chain_update(payload.as_bytes())
        .finalize()
        .into();
    let signature = keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(digest));
    serde_json::json!({
        "signer": keys.public_key().to_hex(),
        "payload": payload,
        "signature": signature.to_string(),
    })
    .to_string()
}

#[test]
fn advertisement_v2_round_trips_and_binds_the_event_author() {
    let keys = Keys::generate();
    let payload = v2_payload();
    let content = sign_advertisement_v2(&payload, &keys).unwrap().to_json();
    assert_eq!(
        verify_advertisement_v2(&content, keys.public_key()).unwrap(),
        payload
    );

    // A valid envelope by another key is not this author's advertisement.
    assert_eq!(
        verify_advertisement_v2(&content, Keys::generate().public_key()).unwrap_err(),
        AdvertisementV2Error::AuthorMismatch,
    );

    // The same bytes under another purpose tag fail the signature.
    let text = serde_json::to_string(&payload).unwrap();
    let other_tag = hand_signed_v2_content(&text, &keys, b"fedi-signed-json/other/v1\0");
    assert_eq!(
        verify_advertisement_v2(&other_tag, keys.public_key()).unwrap_err(),
        AdvertisementV2Error::InvalidSignature,
    );
    let right_tag = hand_signed_v2_content(&text, &keys, b"fedi-signed-json/mg-advertisement/v1\0");
    assert_eq!(
        verify_advertisement_v2(&right_tag, keys.public_key()).unwrap(),
        payload,
        "the literal registered tag verifies",
    );
}

#[test]
fn advertisement_v2_has_the_spec_payload_shape() {
    let value = serde_json::to_value(v2_payload()).unwrap();
    let mut keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "api_endpoints",
            "availability",
            "expires_at",
            "holder_authorizations",
            "issued_at",
            "plans",
            "service_pubkey",
            "uptime_permille",
        ],
    );
    assert_eq!(
        value["uptime_permille"],
        serde_json::json!({"1d": 998, "7d": 991, "30d": 964})
    );

    let mut unreported = v2_payload();
    unreported.uptime_permille = None;
    assert!(
        serde_json::to_value(unreported)
            .unwrap()
            .get("uptime_permille")
            .is_none(),
        "an unreported uptime is absent, not null or zero",
    );
}

/// Unknown members are skipped by syntax wherever they appear, including
/// numbers no `f64` can hold, so a later payload revision cannot break this
/// reader. A buffering serde shape would reject these payloads.
#[test]
fn advertisement_v2_skips_unknown_numbers_in_nested_objects() {
    let keys = Keys::generate();
    let mut value = serde_json::to_value(v2_payload()).unwrap();
    value["holder_authorizations"] = serde_json::json!([serde_json::to_value(
        worst_case_envelope(&Keys::generate(), &Keys::generate(), keys.public_key())
    )
    .unwrap()]);
    let base = serde_json::to_string(&value).unwrap();
    let marker = 0.5;
    for (place, needle) in [
        ("top level", r#""issued_at":"#),
        ("a plan", r#""price_msats":"#),
        ("a holder authorization", r#""holder_authorization":"#),
        ("an availability", r#""federation_sizes":"#),
    ] {
        assert!(base.contains(needle), "{place}: fixture has {needle}");
        let text = base.replacen(
            needle,
            &format!(r#""future":{{"n":1e400,"m":[{marker}]}},{needle}"#),
            1,
        );
        let content =
            hand_signed_v2_content(&text, &keys, b"fedi-signed-json/mg-advertisement/v1\0");
        let parsed = verify_advertisement_v2(&content, keys.public_key())
            .unwrap_or_else(|error| panic!("{place}: {error}"));
        assert_eq!(
            serde_json::to_value(parsed).unwrap(),
            value,
            "{place}: the unknown member is ignored",
        );
    }
}

#[test]
fn advertisement_v2_uptime_is_bounded_to_one_thousand() {
    let keys = Keys::generate();
    for (value, accepted) in [(1000, true), (1001, false)] {
        let mut payload = serde_json::to_value(v2_payload()).unwrap();
        payload["uptime_permille"]["30d"] = serde_json::json!(value);
        let content = hand_signed_v2_content(
            &payload.to_string(),
            &keys,
            b"fedi-signed-json/mg-advertisement/v1\0",
        );
        let result = verify_advertisement_v2(&content, keys.public_key());
        if accepted {
            assert_eq!(result.unwrap().uptime_permille.unwrap().month.get(), 1000);
        } else {
            assert_eq!(result.unwrap_err(), AdvertisementV2Error::Unparsable);
        }
    }
}

/// The legacy rendering of a v2 offer is the payload a v1-only MG signs: the
/// same fields, its own identity, and nothing of the uptime report.
#[test]
fn legacy_rendering_of_a_v2_offer_drops_only_the_uptime() {
    let keys = Keys::generate();
    let v1 = payload(&keys);
    let mut v2 = AdvertisementV2Payload::from(v1.clone());
    assert_eq!(v2.to_legacy(keys.public_key()), v1);
    v2.uptime_permille = v2_payload().uptime_permille;
    assert_eq!(v2.to_legacy(keys.public_key()), v1);
}

/// The v2 counterpart of the worst-case kind-37701 measurement: the same
/// worst-case offer, plus a full uptime report, in an envelope whose payload
/// is an escaped JSON string.
#[test]
fn worst_case_advertisement_v2_event_fits_the_per_event_cap() {
    const ROLE_FETCHED_EVENT_MAX_BYTES: usize = 64 * 1024;

    let mg = Keys::generate();
    let holder = Keys::generate();
    let issuer = Keys::generate();
    let mut payload = v2_payload();
    payload.api_endpoints = (0..8)
        .map(|index| ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!(
                "{IROH_API_ENDPOINT_URL_SCHEME}{index:064x}?relay=https://use1-1.relay.example.iroh.network/&alpn=fedi-fman-fi-setup/1"
            ),
        })
        .collect();
    payload.availability = Availability {
        fedimintd_version: "0.11.1-fedi17+fedi".parse().unwrap(),
        federation_sizes: (1..=64).collect(),
    };
    payload.plans = vec![
        Plan::InfiniteBestEffort {
            price_msats: u64::MAX,
        },
        Plan::SubscriptionBased {
            initial_price_msats: u64::MAX,
            renewal_price_msats: u64::MAX,
            period: "every-30-days".to_owned(),
        },
        Plan::InfiniteBestEffort {
            price_msats: 21_000_000,
        },
    ];
    payload.holder_authorizations = (0..4)
        .map(|_| worst_case_envelope(&holder, &issuer, mg.public_key()))
        .collect();
    let content = sign_advertisement_v2(&payload, &mg).unwrap().to_json();
    let event =
        nostr::EventBuilder::new(nostr::Kind::Custom(MG_ADVERTISEMENT_V2_EVENT_KIND), content)
            .tag(nostr::Tag::identifier(MG_ADVERTISEMENT_V2_D_TAG))
            .tag(nostr::Tag::hashtag(MG_ADVERTISEMENT_V2_HASHTAG))
            .sign_with_keys(&mg)
            .unwrap();

    let event_bytes = event.as_json().len();
    println!("worst-case legitimate advertisement v2 event: {event_bytes} bytes");
    assert!(
        event_bytes < ROLE_FETCHED_EVENT_MAX_BYTES,
        "a worst-case legitimate v2 event ({event_bytes} bytes) must fit the 64 KiB cap",
    );
}
