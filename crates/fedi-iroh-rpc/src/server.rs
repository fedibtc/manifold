use std::future::Future;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    RpcError, RpcResult,
    client::{decode, encode},
};

/// Per-connection metadata available to RPC service handlers. A service
/// method receives it by taking an `RpcConnectionContext` argument before its
/// request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcConnectionContext {
    /// The caller's Iroh endpoint id, authenticated by the connection's
    /// handshake.
    pub remote_id: iroh::EndpointId,
}

#[async_trait]
/// Type-erased method dispatcher implemented by generated service servers.
pub trait RpcServiceHandler: Send + Sync + 'static {
    /// Dispatches an encoded request body, sent over the connection
    /// `context` describes, to a method and returns an encoded service
    /// return value.
    async fn handle_rpc(
        &self,
        context: &RpcConnectionContext,
        method: &str,
        body: &[u8],
    ) -> RpcResult<Vec<u8>>;
}

pub async fn decode_call<Req, Ret, Fut>(
    body: &[u8],
    call: impl FnOnce(Req) -> Fut,
) -> RpcResult<Vec<u8>>
where
    Req: DeserializeOwned,
    Ret: Serialize,
    Fut: Future<Output = Ret> + Send,
{
    let request = decode(body)?;
    let response = call(request).await;
    encode(&response)
}

pub fn unknown_method(method: &str) -> RpcError {
    RpcError::UnknownMethod(method.to_owned())
}
