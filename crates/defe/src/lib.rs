//! Server-side pieces for the `defe` testing framework.

/// Driver for starting and supervising local Bitcoin Core regtest resources.
pub mod bitcoind;
pub mod flip;
/// Driver for starting and supervising local Fleet Manager resources.
pub mod fman;
/// Driver for starting and supervising local Fedimint gateway daemon resources.
pub mod gatewayd;
/// Driver for starting and supervising local `nostr-rs-relay` resources.
pub mod nostr_relay;
/// Driver for starting and supervising local push gateway resources.
pub mod push_gateway;
/// Shared resource-slot manager used by the `defe` server.
pub mod resource_manager;
/// Supervised child-process wrapper used by concrete resource drivers.
pub mod resource_process;
