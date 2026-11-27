//! Seat grant: an MG operator's signed gift of one seat to whichever FA
//! redeems it ([`SPEC-seat-grant`](../../mg/specs/SPEC-seat-grant.md)). The
//! Fedi app calls it a "guardian code".
//!
//! The MG signs a [`SeatGrant`] as a manager response under its own
//! `seat_grant` label, and shares it as a [`SeatGrantCode`]: one
//! `manifold:seat-grant` URI that also carries the MG's [`Locator`] and
//! Manifold environment. The FA presents the signed grant in
//! [`GetQuoteRequest::seat_grant`](crate::GetQuoteRequest::seat_grant).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_iroh_rpc::iroh::{EndpointAddr, EndpointId};
use secp256k1::XOnlyPublicKey;
use serde::{Deserialize, Serialize};

use crate::{AuthError, Locator, ManagerSignature, SignedResponse, Timestamp};

/// Scheme and path of the shared seat grant URI. The scheme is Manifold's
/// own, like the guardian link invite, so the code names no particular app.
const SEAT_GRANT_URI_SCHEME: &str = "manifold";
const SEAT_GRANT_URI_PATH: &str = "seat-grant";

/// Longest operator label a grant carries, in characters.
pub const SEAT_GRANT_LABEL_MAX_CHARS: usize = 32;

/// Federation sizes a seat redeemed from a grant may be bound to: 4 through
/// 20, wider than the verified path's
/// [`FEDERATION_SIZES_0_1`](crate::FEDERATION_SIZES_0_1). The operator gets no
/// say: the size is delegated to the FA with the grant.
pub const SEAT_GRANT_FEDERATION_SIZES: std::ops::RangeInclusive<u16> = 4..=20;

/// Random identity of one grant. The MG refuses a nonce that is already
/// used or revoked. Hex on the wire, which keeps the shared code short.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SeatGrantNonce([u8; 32]);

impl SeatGrantNonce {
    /// Wraps persisted or test bytes; MG issuance uses fresh CSPRNG bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for SeatGrantNonce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

/// A seat grant nonce that is not 32 hex-encoded bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("seat grant nonce is not 32 hex bytes")]
pub struct InvalidSeatGrantNonce;

impl std::str::FromStr for SeatGrantNonce {
    type Err = InvalidSeatGrantNonce;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        hex::decode(value)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(Self)
            .ok_or(InvalidSeatGrantNonce)
    }
}

impl Serialize for SeatGrantNonce {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SeatGrantNonce {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The operator's label for a grant: non-empty and at most
/// [`SEAT_GRANT_LABEL_MAX_CHARS`] characters. A grant without a label has
/// none rather than an empty one.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SeatGrantLabel(String);

/// A seat grant label is empty or longer than [`SEAT_GRANT_LABEL_MAX_CHARS`].
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("seat grant label must be 1 to {SEAT_GRANT_LABEL_MAX_CHARS} characters")]
pub struct InvalidSeatGrantLabel;

impl SeatGrantLabel {
    pub fn new(label: impl Into<String>) -> Result<Self, InvalidSeatGrantLabel> {
        let label = label.into();
        if label.is_empty() || label.chars().count() > SEAT_GRANT_LABEL_MAX_CHARS {
            return Err(InvalidSeatGrantLabel);
        }
        Ok(Self(label))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SeatGrantLabel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The signed grant payload. It names no FA, federation size or expiry:
/// those are delegated to whoever redeems it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SeatGrant {
    pub grant_nonce: SeatGrantNonce,
    pub issued_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<SeatGrantLabel>,
}

/// A `manifold:seat-grant` URI did not parse, or its grant does not verify.
///
/// `env`, `node` and `key` are not signed: like a locator, the code as a
/// whole is trusted because the operator's dashboard produced it, and anyone
/// able to edit it could as well substitute a grant of their own MG. The
/// environment check catches a code shared into the wrong app, not forgery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidSeatGrantCode {
    #[error("not a version 1 seat grant code")]
    Malformed,
    #[error("seat grant code is for the {found} environment, not {expected}")]
    WrongEnvironment {
        found: ManifoldEnvironment,
        expected: ManifoldEnvironment,
    },
    #[error("seat grant signature does not verify")]
    BadSignature,
}

/// The shared guardian code: a signed grant plus the MG's locator and
/// environment. Only constructed from a grant that verified under the
/// locator's key, so holding one means the signature checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeatGrantCode {
    environment: ManifoldEnvironment,
    locator: Locator,
    signed: SignedResponse<SeatGrant>,
    grant: SeatGrant,
}

impl SeatGrantCode {
    /// Pair a signed grant with the MG that signed it (the MG side, after
    /// [`SignedResponse::create`]). Fails unless `signed` verifies under the
    /// locator's `service_pubkey`.
    pub fn new(
        environment: ManifoldEnvironment,
        locator: Locator,
        signed: SignedResponse<SeatGrant>,
    ) -> Result<Self, InvalidSeatGrantCode> {
        let grant = signed
            .verify(&locator.service_pubkey)
            .map_err(|err| match err {
                AuthError::BadSignature => InvalidSeatGrantCode::BadSignature,
                _ => InvalidSeatGrantCode::Malformed,
            })?
            .into_inner();
        Ok(Self {
            environment,
            locator,
            signed,
            grant,
        })
    }

    #[must_use]
    pub fn environment(&self) -> ManifoldEnvironment {
        self.environment
    }

    /// The MG to quote: its endpoint id and commitment-signing key.
    #[must_use]
    pub fn locator(&self) -> &Locator {
        &self.locator
    }

    /// The verified grant contents, for previewing the code.
    #[must_use]
    pub fn grant(&self) -> &SeatGrant {
        &self.grant
    }

    /// The exact signed grant to present in a quote request.
    #[must_use]
    pub fn signed_grant(&self) -> &SignedResponse<SeatGrant> {
        &self.signed
    }

    /// Encode as the `manifold:seat-grant?v=1&...` URI the operator shares.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let (payload, signature) = self.signed.as_parts();
        let mut url = url::Url::parse(&format!("{SEAT_GRANT_URI_SCHEME}:{SEAT_GRANT_URI_PATH}"))
            .expect("constant prefix parses");
        url.query_pairs_mut()
            .append_pair("v", "1")
            .append_pair("env", &self.environment.to_string())
            .append_pair("node", &self.locator.endpoint_addr.id.to_string())
            .append_pair("key", &self.locator.service_pubkey.to_string())
            .append_pair("grant", &URL_SAFE_NO_PAD.encode(payload))
            .append_pair("sig", &signature.0.to_string());
        url.into()
    }

    /// Parse a shared URI and verify its grant over the exact payload bytes
    /// it carries. Only version 1 is accepted, and only a code for
    /// `environment`, the FA's own.
    pub fn parse(
        uri: &str,
        environment: ManifoldEnvironment,
    ) -> Result<Self, InvalidSeatGrantCode> {
        use InvalidSeatGrantCode::Malformed;
        let url = url::Url::parse(uri).map_err(|_| Malformed)?;
        if url.scheme() != SEAT_GRANT_URI_SCHEME
            || url.host().is_some()
            || url.path() != SEAT_GRANT_URI_PATH
        {
            return Err(Malformed);
        }
        let mut version = None;
        let mut env = None;
        let mut node = None;
        let mut key = None;
        let mut grant = None;
        let mut sig = None;
        for (name, value) in url.query_pairs() {
            let slot = match name.as_ref() {
                "v" => &mut version,
                "env" => &mut env,
                "node" => &mut node,
                "key" => &mut key,
                "grant" => &mut grant,
                "sig" => &mut sig,
                _ => return Err(Malformed),
            };
            if slot.replace(value.into_owned()).is_some() {
                return Err(Malformed);
            }
        }
        if version.as_deref() != Some("1") {
            return Err(Malformed);
        }
        let found: ManifoldEnvironment = env.ok_or(Malformed)?.parse().map_err(|_| Malformed)?;
        if found != environment {
            return Err(InvalidSeatGrantCode::WrongEnvironment {
                found,
                expected: environment,
            });
        }
        let node: EndpointId = node.ok_or(Malformed)?.parse().map_err(|_| Malformed)?;
        let key: XOnlyPublicKey = key.ok_or(Malformed)?.parse().map_err(|_| Malformed)?;
        let payload = URL_SAFE_NO_PAD
            .decode(grant.ok_or(Malformed)?)
            .map_err(|_| Malformed)?;
        let signature = sig.ok_or(Malformed)?.parse().map_err(|_| Malformed)?;
        Self::new(
            found,
            Locator::new(EndpointAddr::new(node), key),
            SignedResponse::from_parts(payload, ManagerSignature(signature)),
        )
    }
}

#[cfg(test)]
mod tests;
