use thiserror::Error;

pub type RpcResult<T> = Result<T, RpcError>;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("request is larger than the configured limit")]
    RequestTooLarge,

    #[error("request did not finish before the configured deadline")]
    RequestTimedOut,

    #[error("response is larger than the configured limit")]
    ResponseTooLarge,

    #[error("unknown RPC method: {0}")]
    UnknownMethod(String),

    #[error("encode error: {0}")]
    Encode(String),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("remote RPC transport error: {0}")]
    Remote(String),
    #[error("iroh error: {0}")]
    Iroh(String),
}
