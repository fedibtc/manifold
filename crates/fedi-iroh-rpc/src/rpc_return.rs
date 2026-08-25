use crate::RpcError;

/// Return type adapter used by generated clients.
///
/// Service methods normally return `Result<Response, ServiceError>`. Implement
/// `From<RpcError>` for the service error and this blanket impl lets generated
/// clients turn transport failures into the service's normal result type.
pub trait RpcReturn {
    fn from_rpc_error(error: RpcError) -> Self;
}

impl<T, E> RpcReturn for Result<T, E>
where
    E: From<RpcError>,
{
    fn from_rpc_error(error: RpcError) -> Self {
        Err(E::from(error))
    }
}
