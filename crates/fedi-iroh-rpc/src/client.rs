use std::io::Cursor;

use crate::{
    RpcError, RpcResult,
    frame::{RequestFrame, ResponseFrame, WIRE_VERSION},
};
use iroh::endpoint::{Connection, ReadToEndError};
use serde::{Serialize, de::DeserializeOwned};

/// Low-level client for making encoded RPC calls over an existing iroh connection.
#[derive(Debug, Clone)]
pub struct RpcClient {
    connection: Connection,
    max_request_bytes: usize,
    max_response_bytes: usize,
}

impl RpcClient {
    /// Creates a client with 1 MiB request and response limits.
    #[must_use]
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 1024 * 1024,
        }
    }

    /// Creates a client with explicit request and response byte limits.
    #[must_use]
    pub fn with_limits(
        connection: Connection,
        max_request_bytes: usize,
        max_response_bytes: usize,
    ) -> Self {
        Self {
            connection,
            max_request_bytes,
            max_response_bytes,
        }
    }

    /// Calls one method by opening one bidirectional stream.
    pub async fn call<Req, Ret>(&self, method: &str, request: Req) -> RpcResult<Ret>
    where
        Req: Serialize,
        Ret: DeserializeOwned,
    {
        let body = encode(&request)?;
        if self.max_request_bytes < body.len() {
            return Err(RpcError::RequestTooLarge);
        }

        let frame = RequestFrame::new(method, body);
        let request_bytes = encode(&frame)?;
        if self.max_request_bytes < request_bytes.len() {
            return Err(RpcError::RequestTooLarge);
        }

        let (mut send, mut recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|err| RpcError::Iroh(err.to_string()))?;
        send.write_all(&request_bytes)
            .await
            .map_err(|err| RpcError::Iroh(err.to_string()))?;
        send.finish()
            .map_err(|err| RpcError::Iroh(err.to_string()))?;

        let response_bytes = recv
            .read_to_end(self.max_response_bytes)
            .await
            .map_err(map_response_read_error)?;
        match decode(&response_bytes)? {
            ResponseFrame::Service(body) => decode(&body),
            ResponseFrame::Transport(message) => Err(RpcError::Remote(message)),
        }
    }
}

pub(crate) fn encode<T>(value: &T) -> RpcResult<Vec<u8>>
where
    T: Serialize,
{
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|err| RpcError::Encode(err.to_string()))?;
    Ok(bytes)
}

pub(crate) fn decode<T>(bytes: &[u8]) -> RpcResult<T>
where
    T: DeserializeOwned,
{
    let cursor = Cursor::new(bytes);
    ciborium::from_reader(cursor).map_err(|err| RpcError::Decode(err.to_string()))
}

pub(crate) fn decode_frame(bytes: &[u8]) -> RpcResult<RequestFrame> {
    let frame: RequestFrame = decode(bytes)?;
    if frame.version != WIRE_VERSION {
        return Err(RpcError::Decode(format!(
            "unsupported wire version {}",
            frame.version
        )));
    }
    Ok(frame)
}

fn map_response_read_error(error: ReadToEndError) -> RpcError {
    match error {
        ReadToEndError::TooLong => RpcError::ResponseTooLarge,
        ReadToEndError::Read(error) => RpcError::Iroh(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::frame::{RequestFrame, ResponseFrame};

    use super::{decode, encode};

    #[test]
    fn raw_bodies_are_cbor_byte_strings_without_per_byte_amplification() {
        let body = vec![0x7f; 768 * 1024];
        let request = encode(&RequestFrame::new("bytes", body.clone())).unwrap();
        let response = encode(&ResponseFrame::Service(body)).unwrap();

        assert!(request.len() < 769 * 1024);
        assert!(response.len() < 769 * 1024);
    }

    #[test]
    fn byte_string_frames_still_decode_legacy_byte_arrays_and_vice_versa() {
        #[derive(Deserialize, Serialize)]
        struct LegacyRequestFrame {
            version: u16,
            method: String,
            body: Vec<u8>,
        }

        let legacy = LegacyRequestFrame {
            version: 1,
            method: "bytes".to_owned(),
            body: vec![1, 2, 3],
        };
        let current: RequestFrame = decode(&encode(&legacy).unwrap()).unwrap();
        assert_eq!(current.body, legacy.body);

        let current = RequestFrame::new("bytes", vec![4, 5, 6]);
        let legacy: LegacyRequestFrame = decode(&encode(&current).unwrap()).unwrap();
        assert_eq!(legacy.body, current.body);
    }
}
