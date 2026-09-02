use fedi_credential_sdk_protocol::{
    Credential, CredentialDigest, CredentialProof, HolderAuthorization,
    HolderAuthorizationStatement, HolderId, IssuerId, ProtocolV1 as SdkProtocolV1,
    SignedCredential, SubjectPubkey, Timestamp as SdkTimestamp,
};
use fedi_decentralized_service_fleet_manager::{FEDERATION_SIZES_0_1, FEDIMINTD_VERSION_0_1, Plan};
use nostr::{JsonUtil as _, Keys};

use super::*;

/// The BIP-340 test-vector x-only pubkey for secret `0x...03`, doubling as a
/// syntactically valid commitment-signing service pubkey.
const SERVICE_PUBKEY_HEX: &str = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";

fn payload(keys: &Keys) -> AdvertisementPayload {
    AdvertisementPayload {
        version: FMAN_ADVERTISEMENT_PROTOCOL_VERSION,
        fman_id_pubkey: keys.public_key().to_string(),
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
            fedi_decentralized_service_fleet_manager::Plan::InfiniteBestEffort {
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
                "version": 2,
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
        "kind 37701 must retain its exact payload shape without a per-FMan payment list",
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
    document.payload.fman_id_pubkey = "not-a-pubkey".to_owned();
    assert_eq!(
        verify_advertisement_self_signature(&document).unwrap_err(),
        AdvertisementDocumentError::MalformedFmanIdPubkey
    );
}

#[test]
fn unsupported_advertisement_version_is_rejected() {
    let keys = Keys::generate();
    let mut value = serde_json::to_value(payload(&keys)).unwrap();
    value["version"] = serde_json::json!(1);
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

/// The former signed v1 fixture cannot be mistaken for a v2 advertisement.
#[test]
fn pinned_v1_advertisement_fixture_is_rejected_after_the_versioned_break() {
    const FIXTURE: &str = r#"{"payload":{"version":1,"fman_id_pubkey":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","service_pubkey":"2f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4","issued_at":1730000000,"expires_at":1730007200,"api_endpoints":[{"transport":"iroh","url":"iroh://fixture-endpoint"}],"availability":{"fedimintd_version":"0.8.1+fedi","federation_sizes":[7,13]},"plans":[{"InfiniteBestEffort":{"price_msats":250000}}],"holder_authorizations":[]},"proof":{"signature":"3FMOmhFQ0sHinyiy0bnyGzp8xFRYL4lcewDve5fn8bjlS8AMS-RUrVIxMmkIyB_4buZfOC9stXfANFyJGBghew"}}"#;

    assert!(serde_json::from_str::<AdvertisementDocument>(FIXTURE).is_err());
}

#[test]
fn pinned_v2_advertisement_fixture_verifies_exact_signing_bytes() {
    // Generated independently once with fixed secret scalar 3. Keeping the
    // complete document literal pins the v2 schema, JCS bytes, domain
    // separator, and signature against coordinated producer/verifier drift.
    const FIXTURE: &str = r#"{"payload":{"version":2,"fman_id_pubkey":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","service_pubkey":"2f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4","issued_at":1730000000,"expires_at":1730007200,"api_endpoints":[{"transport":"iroh","url":"iroh://fixture-endpoint"}],"availability":{"fedimintd_version":"0.11.2+fedi","federation_sizes":[7,13]},"plans":[{"InfiniteBestEffort":{"price_msats":250000}}],"holder_authorizations":[]},"proof":{"signature":"mNruInBNqo1ccqxEtmTAaSO8mtJBMvF8PqwctYgwzf8G2ZLvHiDrZmgyb0Sqb_bL3n8Nt7jrntOLk8FlkHJYFQ"}}"#;

    let document = serde_json::from_str::<AdvertisementDocument>(FIXTURE).unwrap();
    assert_eq!(
        document.payload.version,
        FMAN_ADVERTISEMENT_PROTOCOL_VERSION
    );
    assert_eq!(
        document.payload.fman_id_pubkey,
        "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
    );
    assert_eq!(
        document.payload.availability.fedimintd_version.to_string(),
        "0.11.2+fedi"
    );
    assert_eq!(document.payload.availability.federation_sizes, [7, 13]);
    verify_advertisement_self_signature(&document).unwrap();

    let mut tampered = document;
    tampered.payload.issued_at += 1;
    assert_eq!(
        verify_advertisement_self_signature(&tampered).unwrap_err(),
        AdvertisementDocumentError::InvalidProof
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
/// it under the shared 256 KiB per-event relay cap.
///
/// Every field is filled to a generous realistic maximum: 8 long iroh
/// endpoint URLs, one long fedimintd version, 64 federation sizes,
/// 4 plans with verbose prices, and 4 complete holder-authorization
/// envelopes (`FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS` examines at
/// most 4). Measured size at the time of writing: 7299 bytes (~7.1 KiB) —
/// roughly 3% of the 256 KiB cap. The consumers' per-event cap
/// (`ROLE_FETCHED_EVENT_MAX_BYTES` in `crates/nostr-clients`) should be
/// tightened toward this measurement, with margin, in a follow-up; see the
/// TODO beside `FMAN_ADVERTISEMENTS_RETAINED_MAX_BYTES` in
/// `crates/nostr-clients/src/fi.rs`.
#[test]
fn worst_case_advertisement_event_fits_the_per_event_cap() {
    // Restated literally: `ROLE_FETCHED_EVENT_MAX_BYTES` lives in
    // `crates/nostr-clients`, which depends on this crate, so importing it
    // here would invert the dependency.
    const ROLE_FETCHED_EVENT_MAX_BYTES: usize = 256 * 1024;

    let fman = Keys::generate();
    let holder = Keys::generate();
    let issuer = Keys::generate();
    let payload = AdvertisementPayload {
        version: FMAN_ADVERTISEMENT_PROTOCOL_VERSION,
        fman_id_pubkey: fman.public_key().to_string(),
        service_pubkey: SERVICE_PUBKEY_HEX.to_owned(),
        issued_at: 1_730_000_000,
        expires_at: 1_730_007_200,
        api_endpoints: (0..8)
            .map(|index| ApiEndpoint {
                transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
                url: format!(
                    "{IROH_API_ENDPOINT_URL_SCHEME}{index:064x}?relay=https://use1-1.relay.example.iroh.network/&alpn=fedi/fleet-manager/0.2"
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
            .map(|_| worst_case_envelope(&holder, &issuer, fman.public_key()))
            .collect(),
    };
    let document = sign_advertisement(payload, &fman).expect("worst-case advertisement signs");
    let content = serde_json::to_string(&document).expect("worst-case advertisement serializes");
    let event =
        nostr::EventBuilder::new(nostr::Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND), content)
            .tag(nostr::Tag::identifier(FMAN_ADVERTISEMENT_D_TAG))
            .tag(nostr::Tag::hashtag(FMAN_ADVERTISEMENT_HASHTAG))
            .sign_with_keys(&fman)
            .expect("worst-case advertisement event signs");

    let event_bytes = event.as_json().len();
    println!("worst-case legitimate advertisement event: {event_bytes} bytes");
    assert!(
        event_bytes < ROLE_FETCHED_EVENT_MAX_BYTES,
        "a worst-case legitimate advertisement event ({event_bytes} bytes) must fit \
         the shared 256 KiB per-event relay cap",
    );
}
