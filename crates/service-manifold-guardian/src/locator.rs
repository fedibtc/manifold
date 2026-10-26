//! The out-of-band locator: the connection card an operator copies from the
//! daemon's log and hands to an FA (SPEC-fa-rpc).
//!
//! The locator is a wire contract between two programs — the daemon prints
//! it, `fa-cli` parses it — so this crate owns its one definition, like the
//! signed envelopes, and the two sides cannot drift.

use fedi_decentralized_services::domain::ProtocolV1;
use fedi_iroh_rpc::iroh::EndpointAddr;
use secp256k1::XOnlyPublicKey;

/// ALPN for the App ↔ Manifold Guardian iroh protocol.
pub const MANIFOLD_GUARDIAN_ALPN: &[u8] = b"fedi/fleet-manager/0.1";

/// Everything an FA needs to reach and trust one MG: iroh dialing info
/// plus the service key its commitment signatures verify against.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct Locator {
    /// Locator format version; deserialization accepts only version 1.
    /// Private so every in-memory locator speaks the one format this crate
    /// does: [`Locator::new`] stamps it and serde rejects anything else.
    /// The transport (iroh) and ALPN are what the version means, not
    /// fields — the locator is dialing info plus the service key
    /// (SPEC-fa-rpc), and both sides compile the constants in.
    /// `transport: "iroh"` and `alpn` are omitted because they are
    /// single-valued and the specification does not require them.
    version: ProtocolV1,

    /// Iroh dialing info for the MG endpoint.
    pub endpoint_addr: EndpointAddr,

    /// MG commitment-signing public key (x-only secp256k1, hex on the
    /// wire; scheme governed by ARCH-manifold-guardian-identity *Signature
    /// scheme*); FAs verify MG commitment signatures against it.
    pub service_pubkey: XOnlyPublicKey,
}

impl Locator {
    /// Stdout line prefix the daemon prints the locator JSON behind and
    /// harnesses (e.g. the formation E2E test) scan for. Part of the
    /// locator's out-of-band contract, so it lives with the format.
    pub const LOG_PREFIX: &'static str = "Fleet Manager locator: ";

    pub fn new(endpoint_addr: EndpointAddr, service_pubkey: XOnlyPublicKey) -> Self {
        Self {
            version: ProtocolV1,
            endpoint_addr,
            service_pubkey,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a locator always serializes to JSON")
    }
}

/// Name of the network-isolated E2E routing variable shared by every program
/// that dials MGs: comma-separated `<endpoint-id>=<socket-addr>` pairs.
pub const MG_E2E_ENDPOINT_ADDRS_ENV: &str = "MG_E2E_MG_ENDPOINT_ADDRS";

#[derive(Debug, thiserror::Error)]
#[error("{MG_E2E_ENDPOINT_ADDRS_ENV} entry {pair:?} is not `<endpoint-id>=<socket-addr>`")]
pub struct InvalidEndpointAddrPair {
    pair: String,
}

/// Parse [`MG_E2E_ENDPOINT_ADDRS_ENV`]. Advertisements and grants name only an
/// MG endpoint id; the isolated harness has no address lookup service, so each
/// dialing program seeds a memory lookup from the ports the harness pinned.
pub fn parse_endpoint_addr_pairs(
    pairs: &str,
) -> Result<Vec<EndpointAddr>, InvalidEndpointAddrPair> {
    pairs
        .split(',')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let invalid = || InvalidEndpointAddrPair {
                pair: pair.to_owned(),
            };
            let (id, addr) = pair.split_once('=').ok_or_else(invalid)?;
            Ok(EndpointAddr::new(id.parse().map_err(|_| invalid())?)
                .with_ip_addr(addr.parse().map_err(|_| invalid())?))
        })
        .collect()
}

#[cfg(test)]
mod tests;
