//! Guardian link: the operator's Fedi app linked to this FMan so the daemon
//! can notify it through the push gateway when something needs attention
//! ([`SPEC-guardian-link`](../../fman/specs/SPEC-guardian-link.md)).
//!
//! This is its own Iroh ALPN beside the FI-facing service and the telemetry
//! service: the caller is the operator's phone, not an FI, and nothing here
//! grants seat authority. Linking is authorized by a one-time secret the
//! dashboard shows as a QR code; every later verb is signed by the device key
//! the link established, using the exact-byte [`SignedRequest`] envelope with
//! its own per-verb labels.

use fedi_decentralized_services::domain::{ProtocolV1, Timestamp};
use fedi_iroh_rpc::service;
use serde::{Deserialize, Serialize};

use crate::signing::FiSignedRequest;
use crate::{DkgCompletionCallback, FiId, SignedRequest};

/// Dedicated Iroh ALPN for the operator's app to reach its own FMan.
pub const GUARDIAN_LINK_ALPN: &[u8] = b"fedi/fman/guardian-link/1";

/// Longest device label accepted, in characters.
pub const DEVICE_LABEL_MAX_CHARS: usize = 64;

/// Scheme and host of the link invite the dashboard shows as a QR code.
pub const GUARDIAN_LINK_URI_PREFIX: &str = "fedi://guardian-link";

/// One-time secret proving the caller read the dashboard's link invite.
#[derive(Clone, Eq, PartialEq)]
pub struct LinkSecret([u8; 32]);

impl LinkSecret {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    fn from_hex(value: &str) -> Result<Self, InvalidGuardianLinkInvite> {
        let bytes = hex::decode(value).map_err(|_| InvalidGuardianLinkInvite)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| InvalidGuardianLinkInvite)?;
        Ok(Self(bytes))
    }
}

impl core::fmt::Debug for LinkSecret {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("LinkSecret([REDACTED])")
    }
}

impl Serialize for LinkSecret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for LinkSecret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(|_| serde::de::Error::custom("invalid link secret"))
    }
}

/// The dashboard's link invite: everything the app needs to find, trust, and
/// link to one FMan. Carried as a `fedi://guardian-link` URI in a QR code.
#[derive(Clone, Eq, PartialEq)]
pub struct GuardianLinkInvite {
    /// The FMan's public Nostr identity, which names it in the registry.
    pub fman_nostr_pubkey: nostr::PublicKey,
    /// The Iroh endpoint id the app dials on [`GUARDIAN_LINK_ALPN`].
    pub iroh_endpoint_id: String,
    /// Manifold environment of the FMan, so an app refuses a cross-environment link.
    pub environment: String,
    pub secret: LinkSecret,
}

impl core::fmt::Debug for GuardianLinkInvite {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GuardianLinkInvite")
            .field("fman_nostr_pubkey", &self.fman_nostr_pubkey)
            .field("iroh_endpoint_id", &self.iroh_endpoint_id)
            .field("environment", &self.environment)
            .field("secret", &self.secret)
            .finish()
    }
}

/// A link invite URI did not parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid guardian link invite")]
pub struct InvalidGuardianLinkInvite;

impl GuardianLinkInvite {
    /// Encode as the `fedi://guardian-link?v=1&...` URI the QR code carries.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut url = url::Url::parse(GUARDIAN_LINK_URI_PREFIX).expect("constant prefix parses");
        url.query_pairs_mut()
            .append_pair("v", "1")
            .append_pair("fman", &self.fman_nostr_pubkey.to_hex())
            .append_pair("node", &self.iroh_endpoint_id)
            .append_pair("env", &self.environment)
            .append_pair("secret", &self.secret.to_hex());
        url.to_string()
    }

    /// Parse a scanned URI. Only version 1 is accepted.
    pub fn parse(uri: &str) -> Result<Self, InvalidGuardianLinkInvite> {
        let url = url::Url::parse(uri).map_err(|_| InvalidGuardianLinkInvite)?;
        if url.scheme() != "fedi" || url.host_str() != Some("guardian-link") {
            return Err(InvalidGuardianLinkInvite);
        }
        let mut version = None;
        let mut fman = None;
        let mut node = None;
        let mut environment = None;
        let mut secret = None;
        for (key, value) in url.query_pairs() {
            let slot = match key.as_ref() {
                "v" => &mut version,
                "fman" => &mut fman,
                "node" => &mut node,
                "env" => &mut environment,
                "secret" => &mut secret,
                _ => return Err(InvalidGuardianLinkInvite),
            };
            if slot.replace(value.into_owned()).is_some() {
                return Err(InvalidGuardianLinkInvite);
            }
        }
        if version.as_deref() != Some("1") {
            return Err(InvalidGuardianLinkInvite);
        }
        let fman_nostr_pubkey = nostr::PublicKey::from_hex(&fman.ok_or(InvalidGuardianLinkInvite)?)
            .map_err(|_| InvalidGuardianLinkInvite)?;
        let iroh_endpoint_id = node.ok_or(InvalidGuardianLinkInvite)?;
        let environment = environment.ok_or(InvalidGuardianLinkInvite)?;
        if iroh_endpoint_id.is_empty() || environment.is_empty() {
            return Err(InvalidGuardianLinkInvite);
        }
        Ok(Self {
            fman_nostr_pubkey,
            iroh_endpoint_id,
            environment,
            secret: LinkSecret::from_hex(&secret.ok_or(InvalidGuardianLinkInvite)?)?,
        })
    }
}

/// Why the FMan wants the operator to look. Coarse on purpose: these codes
/// travel through the push provider, so they name a condition and never a
/// seat, federation, amount, or message.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Deserialize,
    Serialize,
    strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// A seat's child gave up; the operator must act.
    SeatFailed,
    /// A seat has been unavailable for longer than a restart should take.
    SeatUnavailable,
    /// The daemon stopped advertising and quoting new seats.
    NotReadyForNewSeats,
    /// An accepted payment federation cannot receive while seats are priced.
    PaymentFederationNotReceivable,
    /// The host has no Holder authorization, so it is not advertised.
    NotApproved,
    /// Fedi support wrote to the operator.
    SupportMessage,
    /// The operator asked the dashboard for a test notification.
    Test,
}

impl AttentionReason {
    /// The stable wire code, which is also the push payload token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SeatFailed => "seat_failed",
            Self::SeatUnavailable => "seat_unavailable",
            Self::NotReadyForNewSeats => "not_ready_for_new_seats",
            Self::PaymentFederationNotReceivable => "payment_federation_not_receivable",
            Self::NotApproved => "not_approved",
            Self::SupportMessage => "support_message",
            Self::Test => "test",
        }
    }

    /// Parse one wire code.
    pub fn parse(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator as _;
        Self::iter().find(|reason| reason.as_str() == value)
    }
}

/// Link the calling device. Unsigned: the one-time secret authorizes it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkDeviceRequest {
    pub version: ProtocolV1,
    pub secret: LinkSecret,
    /// The key every later verb from this device is signed with.
    pub device_id: FiId,
    /// Operator-facing name of the phone, at most [`DEVICE_LABEL_MAX_CHARS`].
    pub device_label: String,
    /// The push-gateway hook the FMan invokes, under its configured origin.
    pub callback: DkgCompletionCallback,
    /// When the hook expires at the gateway; the app renews before then.
    pub callback_expires_at: Timestamp,
}

impl core::fmt::Debug for LinkDeviceRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LinkDeviceRequest")
            .field("device_id", &self.device_id)
            .field("device_label", &self.device_label)
            .field("callback_expires_at", &self.callback_expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkDeviceResponse {
    pub version: ProtocolV1,
    /// The FMan's display name, for the app's guardian screen.
    pub fman_name: String,
    pub linked_at: Timestamp,
}

/// What the FMan currently wants the operator to look at.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionRequest {
    pub ts: Timestamp,
    pub device_id: FiId,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionResponse {
    pub version: ProtocolV1,
    pub fman_name: String,
    /// Current reasons, sorted; empty when nothing needs attention.
    pub reasons: Vec<AttentionReason>,
    pub checked_at: Timestamp,
    pub linked_at: Timestamp,
    pub last_notified_at: Option<Timestamp>,
    pub callback_expires_at: Timestamp,
}

/// Replace the hook before it expires, or after the gateway rejected it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenewCallbackRequest {
    pub ts: Timestamp,
    pub device_id: FiId,
    pub callback: DkgCompletionCallback,
    pub callback_expires_at: Timestamp,
}

impl core::fmt::Debug for RenewCallbackRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RenewCallbackRequest")
            .field("device_id", &self.device_id)
            .field("callback_expires_at", &self.callback_expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenewCallbackResponse {
    pub version: ProtocolV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnlinkDeviceRequest {
    pub ts: Timestamp,
    pub device_id: FiId,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnlinkDeviceResponse {
    pub version: ProtocolV1,
}

/// Device-signed verbs reuse the FI request envelope with their own labels:
/// the signer field carries the device key the link established, and the
/// labels keep a device signature from replaying as any FI verb.
macro_rules! device_signed_requests {
    ($($ty:ty => $label:literal,)+) => {
        $(impl FiSignedRequest for $ty {
            const LABEL: &'static str = $label;

            fn ts(&self) -> Timestamp {
                self.ts
            }

            fn fi_id(&self) -> &FiId {
                &self.device_id
            }
        })+

        #[cfg(test)]
        pub(crate) const DEVICE_REQUEST_LABELS: &[&str] = &[$($label),+];
    };
}

device_signed_requests! {
    GetAttentionRequest => "guardian_link/get_attention",
    RenewCallbackRequest => "guardian_link/renew_callback",
    UnlinkDeviceRequest => "guardian_link/unlink_device",
}

/// Service failures as the app sees them; transport failures fold in through
/// [`From<fedi_iroh_rpc::RpcError>`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum GuardianLinkError {
    /// The secret is unknown, used, or expired: scan a fresh code.
    #[error("link invite is not valid")]
    InvalidInvite,
    /// The signature failed or the signer is not the linked device.
    #[error("unauthorized")]
    Unauthorized,
    /// No device is linked right now.
    #[error("no device is linked")]
    NotLinked,
    /// The daemon cannot invoke hooks: no push gateway origin is configured,
    /// or the hook is not under it.
    #[error("push gateway not usable: {0}")]
    PushGatewayUnusable(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("internal error")]
    Internal,
}

impl From<fedi_iroh_rpc::RpcError> for GuardianLinkError {
    fn from(error: fedi_iroh_rpc::RpcError) -> Self {
        Self::Transport(error.to_string())
    }
}

pub type GuardianLinkResult<T> = Result<T, GuardianLinkError>;

/// The operator's app ↔ FMan service on [`GUARDIAN_LINK_ALPN`].
#[service]
pub trait GuardianLinkService {
    /// Link the calling device with the dashboard's one-time secret. A new
    /// link replaces the previous device.
    async fn link_device(
        &self,
        request: LinkDeviceRequest,
    ) -> GuardianLinkResult<LinkDeviceResponse>;

    /// Read what currently needs attention.
    async fn get_attention(
        &self,
        request: SignedRequest<GetAttentionRequest>,
    ) -> GuardianLinkResult<GetAttentionResponse>;

    /// Replace the hook the FMan invokes.
    async fn renew_callback(
        &self,
        request: SignedRequest<RenewCallbackRequest>,
    ) -> GuardianLinkResult<RenewCallbackResponse>;

    /// Forget this device; the FMan stops notifying it.
    async fn unlink_device(
        &self,
        request: SignedRequest<UnlinkDeviceRequest>,
    ) -> GuardianLinkResult<UnlinkDeviceResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> GuardianLinkInvite {
        GuardianLinkInvite {
            fman_nostr_pubkey: nostr::Keys::generate().public_key(),
            iroh_endpoint_id: "3b7d8e".repeat(8),
            environment: "staging".to_owned(),
            secret: LinkSecret::from_bytes([7; 32]),
        }
    }

    #[test]
    fn invite_uri_round_trips() {
        let invite = invite();
        let uri = invite.to_uri();
        assert!(uri.starts_with("fedi://guardian-link?v=1&fman="), "{uri}");
        assert_eq!(GuardianLinkInvite::parse(&uri).unwrap(), invite);
    }

    #[test]
    fn invite_uri_rejects_other_versions_and_duplicate_or_unknown_keys() {
        let uri = invite().to_uri();
        assert!(GuardianLinkInvite::parse(&uri.replacen("v=1", "v=2", 1)).is_err());
        assert!(GuardianLinkInvite::parse(&format!("{uri}&extra=1")).is_err());
        assert!(GuardianLinkInvite::parse(&format!("{uri}&env=production")).is_err());
        assert!(GuardianLinkInvite::parse(&uri.replace("guardian-link", "guardian")).is_err());
        assert!(GuardianLinkInvite::parse(&uri.replace("&secret=", "&secret=00")).is_err());
    }

    #[test]
    fn secret_is_hex_on_the_wire_and_redacted_in_debug() {
        let secret = LinkSecret::from_bytes([0xab; 32]);
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(serde_json::from_str::<LinkSecret>(&json).unwrap(), secret);
        assert!(serde_json::from_str::<LinkSecret>("\"abcd\"").is_err());
        assert!(!format!("{secret:?}").contains("abab"));
    }

    #[test]
    fn reasons_round_trip_through_their_codes() {
        use strum::IntoEnumIterator as _;
        for reason in AttentionReason::iter() {
            assert_eq!(AttentionReason::parse(reason.as_str()), Some(reason));
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{}\"", reason.as_str())
            );
        }
        assert_eq!(AttentionReason::parse("seat_exploded"), None);
    }

    #[test]
    fn device_labels_never_collide_with_fi_labels() {
        for label in DEVICE_REQUEST_LABELS {
            assert!(label.starts_with("guardian_link/"), "{label}");
            assert!(!label.contains('\0'));
        }
        let mut sorted = DEVICE_REQUEST_LABELS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), DEVICE_REQUEST_LABELS.len());
    }
}
