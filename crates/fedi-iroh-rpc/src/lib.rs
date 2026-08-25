//! Tiny encoded RPC transport for iroh bidirectional streams.
//!
//! Use [`service`] on an async service trait to generate a typed iroh client and
//! server wrapper.

mod client;
mod error;
mod frame;
mod iroh_protocol;
mod rpc_return;
mod server;

pub use async_trait;
pub use client::RpcClient;
pub use error::{RpcError, RpcResult};
pub use fedi_iroh_rpc_macros::service;
pub use iroh;
pub use iroh_protocol::IrohProtocol;
pub use rpc_return::RpcReturn;
pub use server::{RpcConnectionContext, RpcServiceHandler};

#[doc(hidden)]
pub mod __private {
    pub use crate::server::{decode_call, unknown_method};
}
