//! The out-of-band locator: the connection card an operator copies from the
//! daemon's log and hands to an FI (SPEC-fi-rpc).
//!
//! The locator is a wire contract between two programs — the daemon prints
//! it, `fi-cli` parses it — so this crate owns its one definition, like the
//! signed envelopes, and the two sides cannot drift.

use fedi_decentralized_services::domain::ProtocolV1;
use fedi_iroh_rpc::iroh::EndpointAddr;
use secp256k1::XOnlyPublicKey;

/// ALPN for the App ↔ Fleet Manager iroh protocol.
pub const FLEET_MANAGER_ALPN: &[u8] = b"fedi/fleet-manager/0.1";

/// Everything an FI needs to reach and trust one FMan: iroh dialing info
/// plus the service key its commitment signatures verify against.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct Locator {
    /// Locator format version; deserialization accepts only version 1.
    /// Private so every in-memory locator speaks the one format this crate
    /// does: [`Locator::new`] stamps it and serde rejects anything else.
    /// The transport (iroh) and ALPN are what the version means, not
    /// fields — the locator is dialing info plus the service key
    /// (SPEC-fi-rpc), and both sides compile the constants in.
    /// `transport: "iroh"` and `alpn` are omitted because they are
    /// single-valued and the specification does not require them.
    version: ProtocolV1,

    /// Iroh dialing info for the FMan endpoint.
    pub endpoint_addr: EndpointAddr,

    /// FMan commitment-signing public key (x-only secp256k1, hex on the
    /// wire; scheme governed by ARCH-fleet-manager-identity *Signature
    /// scheme*); FIs verify FMan commitment signatures against it.
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

#[cfg(test)]
mod tests;
