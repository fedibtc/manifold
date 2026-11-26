//! Guardian link: the operator's Fedi app linked to this MG so the daemon
//! can notify it through the push gateway when something needs attention
//! ([`SPEC-guardian-link`](../../mg/specs/SPEC-guardian-link.md)).
//!
//! This is its own Iroh ALPN beside the FI-facing service and the telemetry
//! service: the caller is the operator's phone, not an FI, and nothing here
//! grants seat authority. The phone makes one call, `link_device`, authorized
//! by the one-time secret the dashboard shows as a QR code. The MG records
//! the Iroh endpoint id the call came from: only that endpoint may then open
//! web UIs on its host over [`GUARDIAN_LINK_FORWARD_ALPN`].

use fedi_decentralized_services::domain::ProtocolV1;
use fedi_iroh_rpc::{RpcConnectionContext, service};
use serde::{Deserialize, Serialize};

use crate::DkgCompletionCallback;

/// Dedicated Iroh ALPN for the operator's app to reach its own MG.
pub const GUARDIAN_LINK_ALPN: &[u8] = b"fedi/fman/guardian-link/1";

/// Iroh ALPN on which the linked device reaches web UIs on the host the MG
/// runs on, such as the umbrelOS or StartOS dashboard. The MG holds a map from
/// [`ForwardTarget`] to an address, configured by the platform package; the
/// caller names a target and never an address or a port.
///
/// A connection from anything but the linked device is closed with a
/// [`ForwardRefusal`] code before any stream is served, and an open one is
/// closed the same way once the link is revoked or replaced or the policy
/// turns the feature off. Otherwise the MG first opens one unidirectional
/// stream carrying its [`ForwardTargets`] as JSON, then finishes it.
///
/// Each bidirectional stream the caller opens starts with a
/// [`ForwardTarget::encode_header`] header naming its target. After it, the
/// stream is one TCP connection to that target: bytes pass through unchanged
/// both ways, so HTTP/1.1 requests and WebSocket upgrades work as they would
/// over TCP. A stream the MG cannot serve is reset with a
/// [`ForwardStreamError`] code.
pub const GUARDIAN_LINK_FORWARD_ALPN: &[u8] = b"fedi/fman/guardian-link-forward/1";

/// Streams open at once across every forward connection. A browser keeps
/// about six HTTP connections and a few WebSockets to one origin; a caller
/// keeps under this to avoid [`ForwardStreamError::TooManyStreams`].
pub const MAX_FORWARD_STREAMS: usize = 32;

/// Largest [`ForwardTargets`] announcement, in bytes.
pub const MAX_FORWARD_TARGETS_BYTES: usize = 4096;

/// A web UI on the MG's host the linked device may open. The platform package
/// gives each the address it has as seen from the MG; a target without one is
/// not offered. Adding a target is a new variant here and a line in
/// SPEC-guardian-link.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ForwardTarget {
    /// The host's own dashboard: umbrelOS's or StartOS's web UI.
    HostUi,
    /// The page where the host signs a browser in to one of its apps. umbrelOS
    /// sends an app's visitor here (its port 2000) and back to the app with a
    /// session for it.
    HostAppLogin,
    /// This MG's own dashboard, through the host's app proxy, so the host's
    /// login guards it.
    MgDashboard,
}

/// A target name that is not one of [`ForwardTarget`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unknown forward target")]
pub struct UnknownForwardTarget;

impl ForwardTarget {
    /// Every target, in the order the MG announces them.
    pub const ALL: [Self; 3] = [Self::HostUi, Self::HostAppLogin, Self::MgDashboard];

    /// Longest target name on the wire, in bytes.
    pub const MAX_NAME_BYTES: usize = 32;

    /// The target's wire name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::HostUi => "host-ui",
            Self::HostAppLogin => "host-app-login",
            Self::MgDashboard => "mg-dashboard",
        }
    }

    /// The header that starts a stream to this target: the name's length as
    /// one byte, then the name.
    #[must_use]
    pub fn encode_header(self) -> Vec<u8> {
        let name = self.name().as_bytes();
        let mut header = Vec::with_capacity(1 + name.len());
        header.push(u8::try_from(name.len()).expect("names are short"));
        header.extend_from_slice(name);
        header
    }
}

impl core::fmt::Display for ForwardTarget {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.name())
    }
}

impl core::str::FromStr for ForwardTarget {
    type Err = UnknownForwardTarget;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|target| target.name() == name)
            .ok_or(UnknownForwardTarget)
    }
}

/// The targets an MG serves, as it announces them on each forward connection.
/// Names stay strings, so a caller skips a target it does not know rather
/// than failing to read the list.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ForwardTargets {
    pub targets: Vec<AnnouncedForwardTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct AnnouncedForwardTarget {
    /// A [`ForwardTarget`] name.
    pub name: String,
    /// The host port the target's web UI is on, so a caller can tell which
    /// target a page's link to that port on the host means.
    pub port: u16,
}

/// Why the MG closed a [`GUARDIAN_LINK_FORWARD_ALPN`] connection, as its QUIC
/// application close code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForwardRefusal {
    /// The caller is not the endpoint that linked to this MG, or no device
    /// is linked. The same answer for both, so it tells a stranger nothing.
    NotLinked,
    /// The feature policy has the guardian link switched off.
    Disabled,
    /// This MG was started without any target: the host has nothing it can
    /// forward.
    Unavailable,
}

impl ForwardRefusal {
    const ALL: [Self; 3] = [Self::NotLinked, Self::Disabled, Self::Unavailable];

    #[must_use]
    pub fn code(self) -> u32 {
        match self {
            Self::NotLinked => 1,
            Self::Disabled => 2,
            Self::Unavailable => 3,
        }
    }

    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| u64::from(refusal.code()) == code)
    }

    /// The close reason sent with the code, for logs on the caller's side.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotLinked => "not_linked",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Why the MG reset one [`GUARDIAN_LINK_FORWARD_ALPN`] stream, as its QUIC
/// reset code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForwardStreamError {
    /// The MG could not open a TCP connection to the target.
    TargetUnreachable,
    /// [`MAX_FORWARD_STREAMS`] streams are open already.
    TooManyStreams,
    /// The stream's header is malformed, or names a target this MG does not
    /// serve.
    UnknownTarget,
}

impl ForwardStreamError {
    const ALL: [Self; 3] = [
        Self::TargetUnreachable,
        Self::TooManyStreams,
        Self::UnknownTarget,
    ];

    #[must_use]
    pub fn code(self) -> u32 {
        match self {
            Self::TargetUnreachable => 1,
            Self::TooManyStreams => 2,
            Self::UnknownTarget => 3,
        }
    }

    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|error| u64::from(error.code()) == code)
    }
}

/// Longest device label accepted, in characters.
pub const DEVICE_LABEL_MAX_CHARS: usize = 64;

/// Scheme and path of the link invite the dashboard shows as a QR code. The
/// scheme is Manifold's own, so the invite names no particular app.
pub const GUARDIAN_LINK_URI_SCHEME: &str = "manifold";
const GUARDIAN_LINK_URI_PATH: &str = "guardian-link";

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
/// link to one MG. Carried as a `manifold:guardian-link` URI in a QR code.
/// `Debug` shows the secret redacted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardianLinkInvite {
    /// The MG's public Nostr identity, which names it in the registry.
    pub fman_nostr_pubkey: nostr::PublicKey,
    /// The Iroh endpoint id the app dials on [`GUARDIAN_LINK_ALPN`].
    pub iroh_endpoint_id: String,
    /// Manifold environment of the MG, so an app refuses a cross-environment link.
    pub environment: String,
    pub secret: LinkSecret,
}

/// A link invite URI did not parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid guardian link invite")]
pub struct InvalidGuardianLinkInvite;

impl GuardianLinkInvite {
    /// Encode as the `manifold:guardian-link?v=1&...` URI the QR code carries.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut url = url::Url::parse(&format!(
            "{GUARDIAN_LINK_URI_SCHEME}:{GUARDIAN_LINK_URI_PATH}"
        ))
        .expect("constant prefix parses");
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
        if url.scheme() != GUARDIAN_LINK_URI_SCHEME
            || url.host().is_some()
            || url.path() != GUARDIAN_LINK_URI_PATH
        {
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

/// Link the calling device. Unsigned: the one-time secret authorizes it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkDeviceRequest {
    pub version: ProtocolV1,
    pub secret: LinkSecret,
    /// Operator-facing name of the phone, at most [`DEVICE_LABEL_MAX_CHARS`].
    pub device_label: String,
    /// The push-gateway hook the MG invokes, under its configured origin.
    pub callback: DkgCompletionCallback,
}

impl core::fmt::Debug for LinkDeviceRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LinkDeviceRequest")
            .field("device_label", &self.device_label)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkDeviceResponse {
    pub version: ProtocolV1,
}

/// Service failures as the app sees them; transport failures fold in through
/// [`From<fedi_iroh_rpc::RpcError>`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum GuardianLinkError {
    /// The secret is unknown, used, or expired: scan a fresh code.
    #[error("link invite is not valid")]
    InvalidInvite,
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

/// The operator's app ↔ MG service on [`GUARDIAN_LINK_ALPN`].
#[service]
pub trait GuardianLinkService {
    /// Link the calling device with the dashboard's one-time secret. A new
    /// link replaces the previous device. The MG keeps the caller's endpoint
    /// id, which alone may then use [`GUARDIAN_LINK_FORWARD_ALPN`], so the device dials
    /// from the stable key it will use for that.
    async fn link_device(
        &self,
        caller: RpcConnectionContext,
        request: LinkDeviceRequest,
    ) -> GuardianLinkResult<LinkDeviceResponse>;
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
        assert!(uri.starts_with("manifold:guardian-link?v=1&fman="), "{uri}");
        assert_eq!(GuardianLinkInvite::parse(&uri).unwrap(), invite);
    }

    #[test]
    fn invite_uri_rejects_other_versions_and_duplicate_or_unknown_keys() {
        let uri = invite().to_uri();
        assert!(GuardianLinkInvite::parse(&uri.replacen("v=1", "v=2", 1)).is_err());
        assert!(GuardianLinkInvite::parse(&format!("{uri}&extra=1")).is_err());
        assert!(GuardianLinkInvite::parse(&format!("{uri}&env=production")).is_err());
        assert!(GuardianLinkInvite::parse(&uri.replace("guardian-link", "guardian")).is_err());
        // The earlier app-specific form and a host-style form are not invites.
        let query = uri.split_once('?').unwrap().1;
        assert!(GuardianLinkInvite::parse(&format!("fedi://guardian-link?{query}")).is_err());
        assert!(GuardianLinkInvite::parse(&format!("manifold://guardian-link?{query}")).is_err());
        assert!(GuardianLinkInvite::parse(&uri.replace("&secret=", "&secret=00")).is_err());
    }

    #[test]
    fn forward_codes_are_distinct_and_round_trip() {
        assert_eq!(ForwardRefusal::ALL.map(ForwardRefusal::code), [1, 2, 3]);
        for refusal in ForwardRefusal::ALL {
            assert_eq!(
                ForwardRefusal::from_code(refusal.code().into()),
                Some(refusal)
            );
        }
        assert_eq!(ForwardRefusal::from_code(0), None);
        assert_eq!(ForwardRefusal::from_code(4), None);
        assert_eq!(
            ForwardStreamError::ALL.map(ForwardStreamError::code),
            [1, 2, 3]
        );
        for error in ForwardStreamError::ALL {
            assert_eq!(
                ForwardStreamError::from_code(error.code().into()),
                Some(error)
            );
        }
        assert_eq!(ForwardStreamError::from_code(0), None);
        assert_eq!(ForwardStreamError::from_code(4), None);
    }

    #[test]
    fn forward_target_names_are_the_wire_contract() {
        assert_eq!(
            ForwardTarget::ALL.map(ForwardTarget::name),
            ["host-ui", "host-app-login", "mg-dashboard"]
        );
        for target in ForwardTarget::ALL {
            assert_eq!(target.name().parse::<ForwardTarget>(), Ok(target));
            assert!(target.name().len() <= ForwardTarget::MAX_NAME_BYTES);
        }
        assert_eq!(
            "Host-UI".parse::<ForwardTarget>(),
            Err(UnknownForwardTarget)
        );
        assert_eq!(ForwardTarget::HostUi.encode_header(), b"\x07host-ui");
        let announced: ForwardTargets =
            serde_json::from_str(r#"{"targets":[{"name":"later","port":9}]}"#).unwrap();
        assert_eq!(announced.targets[0].name, "later");
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
}
