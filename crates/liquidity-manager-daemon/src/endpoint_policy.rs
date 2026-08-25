//! Outbound address policy for requester-supplied federation endpoints.
//!
//! An FMan endorsement authenticates a federation *id*, not the guardian API
//! URLs carried independently in its invite. A requester holding a valid
//! endorsement can therefore substitute transport input while FLIP decides
//! whether to trust the returned configuration, which would make FLIP dial
//! whatever host that requester chose.
//!
//! This module closes the arbitrary-request WebSocket path: `GlobalOnly`
//! rejects every location-bearing `ws`/`wss` URL before its internally
//! resolving, redirect-following connector runs. Canonical Iroh locators remain
//! accepted. Their node owner can publish direct or relay destinations, but the
//! pinned transport sends only its generated discovery/QUIC or relay handshake
//! and framing traffic before end-to-end authentication; it does not provide an
//! arbitrary request stream or return the destination response.
//!
//! The broad claim that no requester-selected connection attempt occurs remains
//! false. FLIP consciously accepts the narrower Iroh residual; deployments
//! giving FLIP sensitive internal reachability should enforce egress policy.

use std::str::FromStr as _;

use fedimint_core::invite_code::InviteCode;
use fedimint_core::util::SafeUrl;
use iroh_base_035::NodeId;

/// Maximum guardian endpoints one requester may make FLIP inspect.
const MAX_INVITE_ENDPOINTS: usize = 16;

/// Which endpoints FLIP is willing to dial for a requester-supplied invite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointPolicy {
    /// Only canonical identity-authenticated Iroh endpoints. The production setting.
    GlobalOnly,

    /// Canonical Iroh or WebSocket endpoints, including loopback and
    /// deployment-private addresses.
    ///
    /// Every local harness runs its federation on loopback, so a policy with
    /// no way to say this would be a policy every test disabled — and a
    /// disabled guard protects nothing. Refused on mainnet.
    AllowPrivate,
}

impl EndpointPolicy {
    /// The policy the boot flag selects.
    ///
    /// All dial paths derive it the same way, so a deployment cannot end up
    /// with one of them stricter than the others.
    pub(crate) fn from_allow_private(allow_private: bool) -> Self {
        if allow_private {
            Self::AllowPrivate
        } else {
            Self::GlobalOnly
        }
    }
}

/// Why an endpoint was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum EndpointPolicyError {
    #[error("invite code is malformed")]
    MalformedInvite,

    #[error("invite endpoint uses an unsupported scheme")]
    UnsupportedScheme,

    #[error("invite endpoint is not a canonical iroh node-id URL")]
    InvalidIrohEndpoint,

    #[error("invite carries too many endpoints")]
    TooManyEndpoints,

    #[error("WebSocket invite endpoints require the explicit private-endpoint allowance")]
    WebSocketDisallowed,
}

/// Checks every guardian endpoint in `invite_code` against `policy`.
///
/// Under [`EndpointPolicy::GlobalOnly`], location-bearing WebSocket endpoints
/// fail before DNS or connector work. The pinned WebSocket connector both
/// resolves internally and follows redirects, so a separate address verdict
/// cannot constrain the socket it eventually opens. Canonical Iroh URLs name an
/// authenticated endpoint identity and remain the production transport.
///
/// # Errors
///
/// Returns the first endpoint that fails, before any connection is attempted.
pub(crate) async fn check_invite_endpoints(
    policy: EndpointPolicy,
    invite_code: &str,
) -> Result<InviteCode, EndpointPolicyError> {
    let invite: InviteCode = invite_code
        .parse()
        .map_err(|_: anyhow::Error| EndpointPolicyError::MalformedInvite)?;
    let peers = invite.peers();
    if peers.len() > MAX_INVITE_ENDPOINTS {
        return Err(EndpointPolicyError::TooManyEndpoints);
    }
    for url in peers.values() {
        check_endpoint(policy, url)?;
    }
    Ok(invite)
}

fn check_endpoint(policy: EndpointPolicy, url: &SafeUrl) -> Result<(), EndpointPolicyError> {
    match url.scheme() {
        "iroh" => check_iroh_endpoint(url),
        "ws" | "wss" if matches!(policy, EndpointPolicy::AllowPrivate) => Ok(()),
        "ws" | "wss" => Err(EndpointPolicyError::WebSocketDisallowed),
        _ => Err(EndpointPolicyError::UnsupportedScheme),
    }
}

/// Accept only Fedimint's canonical identity-only Iroh guardian locator.
///
/// Iroh's parser also accepts a base32 spelling of an endpoint id, while its
/// canonical display (and Fedimint's generated guardian URLs) use lowercase
/// hex. The equality check closes that alternate spelling as well as checking
/// that the 32-byte value is a valid Ed25519 public key.
fn check_iroh_endpoint(url: &SafeUrl) -> Result<(), EndpointPolicyError> {
    let invalid = || EndpointPolicyError::InvalidIrohEndpoint;
    let raw = url.clone().to_unsafe();
    if !raw.username().is_empty()
        || raw.password().is_some()
        || raw.port().is_some()
        || !raw.path().is_empty()
        || raw.query().is_some()
        || raw.fragment().is_some()
    {
        return Err(invalid());
    }

    let host = raw.host_str().ok_or_else(&invalid)?;
    // Use the NodeId parser from the pinned connector's own Iroh release,
    // rather than accepting according to a different Iroh dependency version.
    let node_id = NodeId::from_str(host).map_err(|_| invalid())?;
    if node_id.to_string() != host || raw.as_str() != format!("iroh://{node_id}") {
        return Err(invalid());
    }

    Ok(())
}

#[cfg(test)]
#[path = "../tests/endpoint_policy.rs"]
mod tests;
