//! Exact-payload JSON signature envelope: [`SignedJsonEnvelope`] and
//! [`PurposeTag`].
//!
//! The envelope carries a serialized JSON payload as a JSON string and signs
//! the UTF-8 bytes of its decoded contents under a fixed purpose tag. A
//! verifier checks the signature before it interprets the payload, and never
//! reserializes the payload to derive the signed bytes. The wire contract on
//! those two types is cross-program: other programs sign and verify these
//! exact bytes, so a change to it is a wire break.
//!
//! Alternatives considered (fedibtc/manifold#247):
//!
//! - Signing an embedded JSON object as a raw value: a JSON transport may
//!   legally reserialize objects, so the bytes do not survive. The decoded
//!   contents of a string do.
//! - An opaque CBOR byte string: viable and compact, but it needs a text
//!   encoding for Nostr and is not readable.
//! - The Nostr event signature alone: the documents travel beyond Nostr (RPC,
//!   storage, forwarding), and an event signature has no purpose tag.
//! - A tag or scheme version inside the envelope: a message-chosen tag defeats
//!   domain separation, and a second version source must stay in sync.
//! - Legacy tags under the new signing input: that silently reinterprets
//!   legacy signatures.

#[cfg(test)]
mod tests;

use std::borrow::Cow;

use nostr::secp256k1::{Message, schnorr::Signature};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Maximum encoded size of one envelope document, checked before it is
/// decoded.
pub const SIGNED_JSON_ENVELOPE_MAX_BYTES: usize = 64 * 1024;

/// Maximum size of the decoded payload string contents, in bytes.
pub const SIGNED_JSON_PAYLOAD_MAX_BYTES: usize = 65_536;

const PURPOSE_TAG_PREFIX: &[u8] = b"fedi-signed-json/";

/// Fixed, NUL-terminated purpose tag `fedi-signed-json/<purpose>/v<N>\0`.
///
/// The verifier's context selects the tag: the Nostr kind and `d` tag, or the
/// API, verb and direction. An envelope never names its own tag, and a
/// different envelope shape or algorithm needs a new context, not a new
/// member.
///
/// The `fedi-signed-json/` prefix keeps these tags disjoint from every legacy
/// domain (`fedi-fman-…`, `fedi-flip-…`), so a legacy JCS or CBOR signature
/// never verifies as an envelope signature, or the reverse. The NUL makes
/// tags prefix-free. `<purpose>` names the owner and the message, plus the
/// verb and direction for an RPC message, so a response cannot be replayed as
/// a request or as another verb.
///
/// `v<N>` is the major version of the purpose's payload schema and its only
/// home: payloads carry no `version` field. An additive change keeps the tag.
/// A change that old readers must enforce, or that changes an existing
/// field's meaning, needs a new `v<N>`, and over Nostr also a new kind or `d`
/// tag. Each purpose defines its tag as a constant next to its payload type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PurposeTag(&'static [u8]);

impl PurposeTag {
    /// Create a tag, panicking (at compile time in a `const`) unless it has
    /// the `fedi-signed-json/` prefix, a non-empty `<purpose>/v<N>` body
    /// without NUL, and a single trailing NUL.
    #[must_use]
    pub const fn new(tag: &'static [u8]) -> Self {
        assert!(
            tag.len() > PURPOSE_TAG_PREFIX.len() + 1,
            "purpose tag has no purpose"
        );
        let mut index = 0;
        while index < PURPOSE_TAG_PREFIX.len() {
            assert!(
                tag[index] == PURPOSE_TAG_PREFIX[index],
                "purpose tag must start with fedi-signed-json/"
            );
            index += 1;
        }
        let last = tag.len() - 1;
        assert!(tag[last] == 0, "purpose tag must end with NUL");
        let mut slash = None;
        while index < last {
            assert!(tag[index] != 0, "purpose tag body must not contain NUL");
            if tag[index] == b'/' {
                slash = Some(index);
            }
            index += 1;
        }
        let Some(slash) = slash else {
            panic!("purpose tag must end with /v<N>");
        };
        assert!(
            slash > PURPOSE_TAG_PREFIX.len(),
            "purpose tag has no purpose"
        );
        assert!(
            slash + 2 < last && tag[slash + 1] == b'v',
            "purpose tag must end with /v<N>"
        );
        let mut digit = slash + 2;
        while digit < last {
            assert!(
                tag[digit].is_ascii_digit(),
                "purpose tag must end with /v<N>"
            );
            digit += 1;
        }
        Self(tag)
    }

    /// The exact tag bytes, including the trailing NUL.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        self.0
    }

    fn digest(self, payload: &str) -> Message {
        Message::from_digest(
            Sha256::new()
                .chain_update(self.0)
                .chain_update(payload.as_bytes())
                .finalize()
                .into(),
        )
    }
}

/// A signed message: a signer, the payload as the exact serialized JSON text,
/// and a BIP-340 signature over `SHA256(tag || payload)`.
///
/// # Wire contract
///
/// The envelope is a JSON object
/// `{"signer": <64 lowercase hex x-only key>, "payload": <JSON text as a
/// string>, "signature": <128 lowercase hex BIP-340 signature>}`. The signed
/// bytes are the UTF-8 bytes of the decoded `payload` contents, without
/// quotes or escapes and without Unicode normalization, so the signature
/// survives any conforming JSON transport that re-escapes the string. The
/// signer serializes the payload once and never again; a store or forwarder
/// keeps the envelope or payload string verbatim, since a typed projection
/// loses unknown fields. `signer` sits outside the payload so the key is
/// chosen before any payload is read. A purpose whose payload also names its
/// signer must require the two to be equal.
///
/// A verifier stops at the first failing step:
///
/// 1. The transport's ingress bound, then at most
///    [`SIGNED_JSON_ENVELOPE_MAX_BYTES`] of encoded envelope.
/// 2. Decode: only an object, no missing or duplicate known member, canonical
///    hex `signer` and `signature`, a string `payload` without an unpaired
///    surrogate. Unknown envelope members are unsigned and ignored.
/// 3. At most [`SIGNED_JSON_PAYLOAD_MAX_BYTES`] of decoded payload bytes.
/// 4. The signature under the tag the context selects.
/// 5. Only then the payload parse. Structure the reader decodes is bounded at
///    127 nested containers, counting the top-level object. Unknown values,
///    including duplicate unknown names, are skipped by syntax at any depth,
///    bounded only by the payload size. Reject duplicate known names.
/// 6. Purpose checks: the signer bound to the transport identity (for Nostr,
///    the event author), then freshness, authorization and semantics.
///
/// The bounds are independent: escaping can double the payload inside the
/// envelope and double it again in a Nostr event, so writers check the
/// encoded size of the outer message too.
///
/// Numbers: every known number field is an integer, validated exactly from
/// its token, never through a rounding representation; a fraction, exponent
/// or out-of-range token is rejected. A number inside an unknown field only
/// has to match the RFC 8259 grammar, so `1e400` there is accepted and
/// ignored.
///
/// Unknown fields: after verification, readers ignore payload fields they do
/// not know at every level of a schema object the purpose defines. A value
/// whose schema another project owns, such as a PeerBadge credential's `info`
/// and `blind_msg`, keeps its owner's parsing rules. Writers add a field only
/// when it keeps every existing meaning and has a safe default when absent;
/// a field old readers would have to enforce needs a new tag version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedJsonEnvelope {
    signer: nostr::PublicKey,
    payload: String,
    signature: Signature,
}

/// Wire form. Unknown members are skipped by syntax; missing or duplicate
/// known members fail. The derive also accepts serde's positional sequence
/// form, so [`decode_with_limits`] admits only an object before it decodes.
#[derive(Serialize, Deserialize)]
struct WireEnvelope<'a> {
    #[serde(borrow)]
    signer: Cow<'a, str>,
    #[serde(borrow)]
    payload: Cow<'a, str>,
    #[serde(borrow)]
    signature: Cow<'a, str>,
}

impl SignedJsonEnvelope {
    /// Serialize `payload` once and sign exactly that text.
    ///
    /// # Errors
    ///
    /// [`SignedJsonError::SerializePayload`] when the payload does not
    /// serialize, and the size errors when the payload or the encoded
    /// envelope exceeds its limit.
    pub fn sign<T: Serialize>(
        tag: PurposeTag,
        payload: &T,
        keys: &nostr::Keys,
    ) -> Result<Self, SignedJsonError> {
        let payload =
            serde_json::to_string(payload).map_err(|_| SignedJsonError::SerializePayload)?;
        Self::sign_payload_text(tag, payload, keys)
    }

    fn sign_payload_text(
        tag: PurposeTag,
        payload: String,
        keys: &nostr::Keys,
    ) -> Result<Self, SignedJsonError> {
        if payload.len() > SIGNED_JSON_PAYLOAD_MAX_BYTES {
            return Err(SignedJsonError::PayloadTooLarge);
        }
        let signature = keys.sign_schnorr(&tag.digest(&payload));
        let envelope = Self {
            signer: keys.public_key(),
            payload,
            signature,
        };
        if envelope.to_json().len() > SIGNED_JSON_ENVELOPE_MAX_BYTES {
            return Err(SignedJsonError::EnvelopeTooLarge);
        }
        Ok(envelope)
    }

    /// Decode an envelope document without interpreting its payload.
    ///
    /// # Errors
    ///
    /// [`SignedJsonError::EnvelopeTooLarge`] before any decoding,
    /// [`SignedJsonError::MalformedEnvelope`] for anything but an object
    /// with exactly one string `payload` (valid Unicode) and canonical
    /// lowercase-hex `signer` and `signature`, and
    /// [`SignedJsonError::PayloadTooLarge`] for oversized decoded contents.
    pub fn decode(input: &str) -> Result<Self, SignedJsonError> {
        decode_with_limits(
            input,
            SIGNED_JSON_ENVELOPE_MAX_BYTES,
            SIGNED_JSON_PAYLOAD_MAX_BYTES,
        )
    }

    /// Verify the signature under `tag`, then parse the payload as `T`.
    ///
    /// `T` must decode known integer fields as integers and skip unknown
    /// fields (the default for derived structs). A `T` that buffers values
    /// before choosing a type, such as `#[serde(flatten)]` or untagged and
    /// internally tagged enums, converts unknown numbers and breaks the
    /// specification's number rule.
    ///
    /// # Errors
    ///
    /// [`SignedJsonError::InvalidSignature`] when the signature does not
    /// verify, and [`SignedJsonError::InvalidPayload`] when the verified
    /// payload does not parse as `T`.
    pub fn verify<T: DeserializeOwned>(
        &self,
        tag: PurposeTag,
    ) -> Result<Verified<T>, SignedJsonError> {
        let signer = self
            .signer
            .xonly()
            .expect("decode and sign only admit valid x-only keys");
        nostr::SECP256K1
            .verify_schnorr(&self.signature, &tag.digest(&self.payload), &signer)
            .map_err(|_| SignedJsonError::InvalidSignature)?;
        let payload =
            serde_json::from_str(&self.payload).map_err(|_| SignedJsonError::InvalidPayload)?;
        Ok(Verified {
            signer: self.signer,
            payload,
        })
    }

    /// The claimed signer. Unauthenticated until [`Self::verify`] succeeds.
    #[must_use]
    pub fn signer(&self) -> nostr::PublicKey {
        self.signer
    }

    /// Encode the envelope as a JSON document.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(&WireEnvelope {
            signer: Cow::Owned(self.signer.to_hex()),
            payload: Cow::Borrowed(&self.payload),
            signature: Cow::Owned(hex::encode(self.signature.serialize())),
        })
        .expect("string members always serialize")
    }
}

fn decode_with_limits(
    input: &str,
    max_envelope_bytes: usize,
    max_payload_bytes: usize,
) -> Result<SignedJsonEnvelope, SignedJsonError> {
    if input.len() > max_envelope_bytes {
        return Err(SignedJsonError::EnvelopeTooLarge);
    }
    // Only an object is an envelope: `[signer, payload, signature]` would
    // otherwise decode through the derive's sequence form.
    if !input
        .trim_start_matches([' ', '\t', '\n', '\r'])
        .starts_with('{')
    {
        return Err(SignedJsonError::MalformedEnvelope);
    }
    let wire: WireEnvelope<'_> =
        serde_json::from_str(input).map_err(|_| SignedJsonError::MalformedEnvelope)?;
    if wire.payload.len() > max_payload_bytes {
        return Err(SignedJsonError::PayloadTooLarge);
    }
    let signer = parse_lower_hex::<32>(&wire.signer)
        .and_then(|bytes| nostr::PublicKey::from_slice(&bytes).ok())
        .filter(|signer| signer.xonly().is_ok())
        .ok_or(SignedJsonError::MalformedEnvelope)?;
    let signature = parse_lower_hex::<64>(&wire.signature)
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or(SignedJsonError::MalformedEnvelope)?;
    Ok(SignedJsonEnvelope {
        signer,
        payload: wire.payload.into_owned(),
        signature,
    })
}

fn parse_lower_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if !text
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut bytes = [0; N];
    hex::decode_to_slice(text, &mut bytes).ok()?;
    Some(bytes)
}

/// A payload whose envelope signature verified under the caller's tag.
///
/// Only [`SignedJsonEnvelope::verify`] constructs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verified<T> {
    signer: nostr::PublicKey,
    payload: T,
}

impl<T> Verified<T> {
    /// The key the signature verified under.
    #[must_use]
    pub fn signer(&self) -> nostr::PublicKey {
        self.signer
    }

    /// The parsed payload.
    #[must_use]
    pub fn payload(&self) -> &T {
        &self.payload
    }

    /// Take the parsed payload.
    #[must_use]
    pub fn into_payload(self) -> T {
        self.payload
    }
}

/// Error signing, decoding, or verifying a [`SignedJsonEnvelope`].
///
/// Variants carry no parser detail, which is attacker-influenced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignedJsonError {
    /// The encoded envelope exceeds [`SIGNED_JSON_ENVELOPE_MAX_BYTES`].
    EnvelopeTooLarge,

    /// The envelope is not a well-formed envelope document.
    MalformedEnvelope,

    /// The decoded payload exceeds [`SIGNED_JSON_PAYLOAD_MAX_BYTES`].
    PayloadTooLarge,

    /// The signature does not verify for the signer, tag, and payload.
    InvalidSignature,

    /// The verified payload does not parse as the expected type.
    InvalidPayload,

    /// The payload could not be serialized for signing.
    SerializePayload,
}

impl core::fmt::Display for SignedJsonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::EnvelopeTooLarge => "signed JSON envelope too large",
            Self::MalformedEnvelope => "malformed signed JSON envelope",
            Self::PayloadTooLarge => "signed JSON payload too large",
            Self::InvalidSignature => "invalid signed JSON signature",
            Self::InvalidPayload => "invalid signed JSON payload",
            Self::SerializePayload => "failed to serialize signed JSON payload",
        })
    }
}

impl std::error::Error for SignedJsonError {}
