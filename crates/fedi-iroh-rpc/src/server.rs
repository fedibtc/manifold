use std::future::Future;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    RpcError, RpcResult,
    client::{decode, encode},
};

/// Per-connection metadata available to RPC service handlers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RpcConnectionContext {
    /// Authenticated remote Iroh node id, when available from the connection.
    pub remote_node_id: Option<String>,
}

#[async_trait]
/// Type-erased method dispatcher implemented by generated service servers.
pub trait RpcServiceHandler: Send + Sync + 'static {
    /// Dispatches a encoded request body to a method and returns a encoded service return value.
    async fn handle_rpc(&self, method: &str, body: &[u8]) -> RpcResult<Vec<u8>>;

    /// Dispatches a request with transport connection metadata.
    async fn handle_rpc_with_context(
        &self,
        _context: RpcConnectionContext,
        method: &str,
        body: &[u8],
    ) -> RpcResult<Vec<u8>> {
        self.handle_rpc(method, body).await
    }
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
