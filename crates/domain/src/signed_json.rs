//! Exact-payload JSON signature envelope (`SPEC-exact-payload-signatures`,
//! `specs/` at the repository root).
//!
//! The envelope carries a serialized JSON payload as a JSON string and signs
//! the UTF-8 bytes of its decoded contents under a fixed purpose tag. A
//! verifier checks the signature before it interprets the payload, and never
//! reserializes the payload to derive the signed bytes.

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
/// The verifier's context selects the tag; an envelope never names its own.
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
/// Decode with [`Self::decode`], which applies the size limits; obtain the
/// payload only through [`Self::verify`]. Keep or forward the envelope (or
/// [`Self::to_json`]) verbatim; a typed projection loses unknown fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedJsonEnvelope {
    signer: nostr::PublicKey,
    payload: String,
    signature: Signature,
}

/// Wire form. Decoding accepts only a JSON object (not serde's positional
/// sequence form), rejects missing or duplicate known members, and skips
/// unknown members by syntax.
#[derive(Serialize)]
struct WireEnvelope<'a> {
    signer: Cow<'a, str>,
    payload: Cow<'a, str>,
    signature: Cow<'a, str>,
}

impl<'de> Deserialize<'de> for WireEnvelope<'static> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(WireEnvelopeVisitor)
    }
}

struct WireEnvelopeVisitor;

impl<'de> serde::de::Visitor<'de> for WireEnvelopeVisitor {
    type Value = WireEnvelope<'static>;

    fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("a signed JSON envelope object")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        use serde::de::Error as _;
        fn set<E: serde::de::Error>(
            slot: &mut Option<String>,
            value: String,
            name: &'static str,
        ) -> Result<(), E> {
            if slot.replace(value).is_some() {
                return Err(E::duplicate_field(name));
            }
            Ok(())
        }
        let (mut signer, mut payload, mut signature) = (None, None, None);
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "signer" => set(&mut signer, map.next_value()?, "signer")?,
                "payload" => set(&mut payload, map.next_value()?, "payload")?,
                "signature" => set(&mut signature, map.next_value()?, "signature")?,
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(WireEnvelope {
            signer: Cow::Owned(signer.ok_or_else(|| A::Error::missing_field("signer"))?),
            payload: Cow::Owned(payload.ok_or_else(|| A::Error::missing_field("payload"))?),
            signature: Cow::Owned(signature.ok_or_else(|| A::Error::missing_field("signature"))?),
        })
    }
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
    let wire: WireEnvelope<'static> =
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
