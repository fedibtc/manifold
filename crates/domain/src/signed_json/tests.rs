use serde::Deserialize;

use super::*;

const TAG: PurposeTag = PurposeTag::new(b"fedi-signed-json/test-fixture/v1\0");
const OTHER_TAG: PurposeTag = PurposeTag::new(b"fedi-signed-json/test-fixture/v2\0");

/// Envelope signed outside this crate: a reference BIP-340 implementation
/// with secret key 3 and zero auxiliary randomness, over
/// `SHA256(TAG || payload)`.
const FIXTURE: &str = r#"{"signer":"f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","payload":"{\"issued_at\":9007199254740993,\"note\":\"café \\\"quoted\\\" \\\\ slash/\",\"unknown\":{\"big\":1e400,\"deep\":[[1]]}}","signature":"a72d2069525edc02872a2a92c43d06ef8bdd74b6dcbdeb880f4760c61f36bbd5419a4f9b8f105d265d738538bbfb5e5837458ab9e83080a32988c2623823c910"}"#;

/// The same envelope with the members reordered and the payload string
/// re-escaped (`é`, `\/`), as a reserializing transport may emit it.
const FIXTURE_REESCAPED: &str = r#"{"signature": "a72d2069525edc02872a2a92c43d06ef8bdd74b6dcbdeb880f4760c61f36bbd5419a4f9b8f105d265d738538bbfb5e5837458ab9e83080a32988c2623823c910", "payload": "{\"issued_at\":9007199254740993,\"note\":\"caf\u00e9 \\\"quoted\\\" \\\\ slash\/\",\"unknown\":{\"big\":1e400,\"deep\":[[1]]}}", "signer": "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"}"#;

/// `SHA256(TAG || payload)`, computed outside this crate.
const FIXTURE_DIGEST_HEX: &str = "c9ba59a12c822f7ac95f56dd2762dfce195e7648c98a16270276c04597eda340";

const FIXTURE_SIGNER_HEX: &str = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";

/// The public key of secret key 1, a valid key that did not sign the fixture.
const OTHER_SIGNER_HEX: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

#[derive(Debug, Deserialize, PartialEq)]
struct FixturePayload {
    issued_at: u64,
    note: String,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Counter {
    n: u64,
}

fn envelope_text(signer_hex: &str, payload: &str, signature_hex: &str) -> String {
    serde_json::json!({"signer": signer_hex, "payload": payload, "signature": signature_hex})
        .to_string()
}

/// An envelope for `payload` signed by a fresh key under [`TAG`].
fn signed(payload: &str) -> SignedJsonEnvelope {
    let keys = nostr::Keys::generate();
    SignedJsonEnvelope {
        signer: keys.public_key(),
        payload: payload.to_owned(),
        signature: keys.sign_schnorr(&TAG.digest(payload)),
    }
}

fn verify_counter(payload: &str) -> Result<Counter, SignedJsonError> {
    signed(payload)
        .verify::<Counter>(TAG)
        .map(Verified::into_payload)
}

fn fixture() -> SignedJsonEnvelope {
    SignedJsonEnvelope::decode(FIXTURE).expect("fixture decodes")
}

fn fixture_signature_hex() -> String {
    hex::encode(fixture().signature.serialize())
}

#[test]
fn pinned_fixture_signs_the_decoded_payload_bytes() {
    let envelope = fixture();
    assert_eq!(
        hex::encode(TAG.digest(&envelope.payload).as_ref()),
        FIXTURE_DIGEST_HEX
    );
    let verified = envelope
        .verify::<FixturePayload>(TAG)
        .expect("fixture verifies");
    assert_eq!(verified.signer().to_hex(), FIXTURE_SIGNER_HEX);
    assert_eq!(
        verified.payload(),
        &FixturePayload {
            issued_at: 9_007_199_254_740_993,
            note: "caf\u{e9} \"quoted\" \\ slash/".to_owned(),
        }
    );
    // The writer's encoding of the same envelope is byte-identical.
    assert_eq!(envelope.to_json(), FIXTURE);
}

#[test]
fn reescaped_fixture_still_verifies() {
    let envelope = SignedJsonEnvelope::decode(FIXTURE_REESCAPED).expect("re-escaped decodes");
    assert_eq!(envelope, fixture());
    envelope
        .verify::<FixturePayload>(TAG)
        .expect("re-escaped fixture verifies");
}

#[test]
fn tampering_fails_the_signature_before_the_payload_is_parsed() {
    let fixture = fixture();
    let signature_hex = fixture_signature_hex();
    let changed_number = fixture.payload.replace("740993", "740994");
    // Unparsable as JSON, so `InvalidPayload` would mean it was parsed first.
    let broken_json = fixture.payload.replacen('{', "[", 1);
    let mut flipped_signature = fixture.signature.serialize();
    flipped_signature[63] ^= 1;
    for tampered in [
        envelope_text(FIXTURE_SIGNER_HEX, &changed_number, &signature_hex),
        envelope_text(FIXTURE_SIGNER_HEX, &broken_json, &signature_hex),
        envelope_text(
            FIXTURE_SIGNER_HEX,
            &format!("{} ", fixture.payload),
            &signature_hex,
        ),
        envelope_text(OTHER_SIGNER_HEX, &fixture.payload, &signature_hex),
        envelope_text(
            FIXTURE_SIGNER_HEX,
            &fixture.payload,
            &hex::encode(flipped_signature),
        ),
    ] {
        let envelope = SignedJsonEnvelope::decode(&tampered).expect("tampered envelope decodes");
        assert_eq!(
            envelope.verify::<FixturePayload>(TAG),
            Err(SignedJsonError::InvalidSignature),
            "{tampered}"
        );
    }
}

#[test]
fn wrong_purpose_tag_fails() {
    assert_eq!(
        fixture().verify::<FixturePayload>(OTHER_TAG),
        Err(SignedJsonError::InvalidSignature)
    );
    let request = PurposeTag::new(b"fedi-signed-json/test-verb-request/v1\0");
    let response = PurposeTag::new(b"fedi-signed-json/test-verb-response/v1\0");
    let signed = SignedJsonEnvelope::sign(request, &Counter { n: 7 }, &nostr::Keys::generate())
        .expect("request signs");
    assert_eq!(
        signed.verify::<Counter>(response),
        Err(SignedJsonError::InvalidSignature)
    );
}

#[test]
fn sign_round_trips_through_the_encoded_envelope() {
    let keys = nostr::Keys::generate();
    let envelope = SignedJsonEnvelope::sign(TAG, &Counter { n: u64::MAX }, &keys).expect("signs");
    let decoded = SignedJsonEnvelope::decode(&envelope.to_json()).expect("decodes");
    let verified = decoded.verify::<Counter>(TAG).expect("verifies");
    assert_eq!(verified.signer(), keys.public_key());
    assert_eq!(verified.into_payload(), Counter { n: u64::MAX });
}

/// A payload of `quotes` escaped quotes and `filler` plain bytes. Each quote
/// is 2 payload bytes and 4 envelope bytes.
fn quoted_payload(quotes: usize, filler: usize) -> String {
    format!(
        r#"{{"n":1,"q":"{}{}"}}"#,
        "\\\"".repeat(quotes),
        "a".repeat(filler)
    )
}

/// The signed envelope of exactly `target` encoded bytes.
fn envelope_of_size(quotes: usize, target: usize) -> SignedJsonEnvelope {
    let base = signed(&quoted_payload(quotes, 0)).to_json().len();
    let envelope = signed(&quoted_payload(quotes, target - base));
    assert_eq!(envelope.to_json().len(), target);
    envelope
}

#[test]
fn envelope_bound_counts_escaped_bytes() {
    let quotes = 16_000;
    let at_limit = envelope_of_size(quotes, SIGNED_JSON_ENVELOPE_MAX_BYTES);
    // Escaping makes the payload far smaller than the envelope it needs.
    assert!(at_limit.payload.len() < SIGNED_JSON_ENVELOPE_MAX_BYTES * 3 / 4);
    let decoded = SignedJsonEnvelope::decode(&at_limit.to_json()).expect("at the limit decodes");
    assert_eq!(
        decoded.verify::<Counter>(TAG).map(Verified::into_payload),
        Ok(Counter { n: 1 })
    );
    assert_eq!(
        SignedJsonEnvelope::sign_payload_text(TAG, at_limit.payload, &nostr::Keys::generate())
            .map(|envelope| envelope.to_json().len()),
        Ok(SIGNED_JSON_ENVELOPE_MAX_BYTES)
    );

    let over = envelope_of_size(quotes, SIGNED_JSON_ENVELOPE_MAX_BYTES + 1);
    assert_eq!(
        SignedJsonEnvelope::decode(&over.to_json()),
        Err(SignedJsonError::EnvelopeTooLarge)
    );
    assert_eq!(
        SignedJsonEnvelope::sign_payload_text(TAG, over.payload, &nostr::Keys::generate()),
        Err(SignedJsonError::EnvelopeTooLarge)
    );
}

#[test]
fn payload_bound_counts_decoded_bytes_independently() {
    // Two-byte characters: the bound counts bytes, not characters.
    let payload = format!(r#"{{"n":1,"q":"{}"}}"#, "\u{e9}".repeat(100));
    let text = signed(&payload).to_json();
    let limit = payload.len();
    assert!(decode_with_limits(&text, usize::MAX, limit).is_ok());
    assert_eq!(
        decode_with_limits(&text, usize::MAX, limit - 1),
        Err(SignedJsonError::PayloadTooLarge)
    );
    assert_eq!(
        SignedJsonEnvelope::sign_payload_text(
            TAG,
            "a".repeat(SIGNED_JSON_PAYLOAD_MAX_BYTES + 1),
            &nostr::Keys::generate()
        ),
        Err(SignedJsonError::PayloadTooLarge)
    );
}

#[test]
fn unpaired_surrogate_payload_is_malformed() {
    let signature_hex = fixture_signature_hex();
    for escape in [r"\ud800", r"\udc00", r"\ud83d x"] {
        let text = format!(
            r#"{{"signer":"{FIXTURE_SIGNER_HEX}","payload":"{escape}","signature":"{signature_hex}"}}"#
        );
        assert_eq!(
            SignedJsonEnvelope::decode(&text),
            Err(SignedJsonError::MalformedEnvelope),
            "{escape}"
        );
    }
    let paired = format!(
        r#"{{"signer":"{FIXTURE_SIGNER_HEX}","payload":"😀","signature":"{signature_hex}"}}"#
    );
    let decoded = SignedJsonEnvelope::decode(&paired).expect("paired surrogates decode");
    assert_eq!(decoded.payload, "\u{1f600}");
}

#[test]
fn numbers_in_unknown_fields_are_skipped_by_syntax() {
    for payload in [
        r#"{"n":1,"x":1e400}"#,
        r#"{"n":1,"x":-1e-400}"#,
        r#"{"x":123456789012345678901234567890123456789,"n":1}"#,
        r#"{"n":1,"x":{"y":[1e400,0.1,-0]}}"#,
    ] {
        assert_eq!(verify_counter(payload), Ok(Counter { n: 1 }), "{payload}");
    }
    for malformed in [
        r#"{"n":1,"x":01}"#,
        r#"{"n":1,"x":1.}"#,
        r#"{"n":1,"x":NaN}"#,
    ] {
        assert_eq!(
            verify_counter(malformed),
            Err(SignedJsonError::InvalidPayload),
            "{malformed}"
        );
    }
    // Unknown envelope members follow the same rule.
    let with_member = FIXTURE.replacen('{', r#"{"extra":1e400,"#, 1);
    assert_eq!(SignedJsonEnvelope::decode(&with_member), Ok(fixture()));
}

#[test]
fn known_integers_are_exact_or_rejected() {
    assert_eq!(
        verify_counter(r#"{"n":9007199254740993}"#),
        Ok(Counter {
            n: 9_007_199_254_740_993
        })
    );
    assert_eq!(
        verify_counter(r#"{"n":18446744073709551615}"#),
        Ok(Counter { n: u64::MAX })
    );
    for inexact in [
        r#"{"n":9007199254740993.0}"#,
        r#"{"n":1e3}"#,
        r#"{"n":18446744073709551616}"#,
        r#"{"n":-1}"#,
        r#"{"n":"1"}"#,
    ] {
        assert_eq!(
            verify_counter(inexact),
            Err(SignedJsonError::InvalidPayload),
            "{inexact}"
        );
    }
}

#[test]
fn duplicate_known_names_fail_and_duplicate_unknown_names_are_skipped() {
    assert_eq!(
        verify_counter(r#"{"n":1,"n":2}"#),
        Err(SignedJsonError::InvalidPayload)
    );
    assert_eq!(
        verify_counter(r#"{"n":1,"x":1,"x":2}"#),
        Ok(Counter { n: 1 })
    );

    let signature_hex = fixture_signature_hex();
    let duplicate_payload = format!(
        r#"{{"signer":"{FIXTURE_SIGNER_HEX}","payload":"{{}}","payload":"{{}}","signature":"{signature_hex}"}}"#
    );
    assert_eq!(
        SignedJsonEnvelope::decode(&duplicate_payload),
        Err(SignedJsonError::MalformedEnvelope)
    );
}

#[test]
fn nesting_is_bounded_where_the_payload_is_decoded() {
    let nested = |depth: usize| {
        format!(
            r#"{{"n":1,"x":{}{}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        )
    };
    // Unknown values are skipped by syntax at any depth within the size bound.
    assert_eq!(verify_counter(&nested(10_000)), Ok(Counter { n: 1 }));
    // Decoded structure has at most 127 containers, counting the top-level
    // object.
    let decode_all = |depth: usize| {
        signed(&nested(depth))
            .verify::<serde_json::Value>(TAG)
            .map(|_| ())
    };
    assert_eq!(decode_all(126), Ok(()));
    assert_eq!(decode_all(127), Err(SignedJsonError::InvalidPayload));
}

#[test]
fn malformed_envelopes_are_rejected() {
    let signature_hex = fixture_signature_hex();
    for text in [
        "[]".to_owned(),
        "null".to_owned(),
        FIXTURE.replace(FIXTURE_SIGNER_HEX, &FIXTURE_SIGNER_HEX.to_uppercase()),
        FIXTURE.replace(&signature_hex, &signature_hex[2..]),
        format!(
            r#"{{"signer":"{FIXTURE_SIGNER_HEX}","payload":{{"n":1}},"signature":"{signature_hex}"}}"#
        ),
        format!(r#"{{"signer":"{FIXTURE_SIGNER_HEX}","signature":"{signature_hex}"}}"#),
        // Not the x coordinate of a curve point.
        FIXTURE.replace(FIXTURE_SIGNER_HEX, &"f".repeat(64)),
    ] {
        assert_eq!(
            SignedJsonEnvelope::decode(&text),
            Err(SignedJsonError::MalformedEnvelope),
            "{text}"
        );
    }
}

#[test]
fn positional_sequence_form_is_rejected() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    let array = serde_json::json!([fixture["signer"], fixture["payload"], fixture["signature"]])
        .to_string();
    assert_eq!(
        SignedJsonEnvelope::decode(&array),
        Err(SignedJsonError::MalformedEnvelope)
    );
}

#[test]
#[should_panic(expected = "fedi-signed-json/")]
fn legacy_domain_is_not_a_purpose_tag() {
    let _ = PurposeTag::new(b"fedi-fman-advertisement/v1\0");
}

#[test]
#[should_panic(expected = "NUL")]
fn purpose_tag_requires_a_trailing_nul() {
    let _ = PurposeTag::new(b"fedi-signed-json/test-fixture/v1");
}

#[test]
#[should_panic(expected = "/v<N>")]
fn purpose_tag_requires_a_version() {
    let _ = PurposeTag::new(b"fedi-signed-json/test-fixture\0");
}
