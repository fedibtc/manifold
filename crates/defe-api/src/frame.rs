use std::io::Cursor;

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

/// Maximum allowed CBOR payload size in one frame.
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;
const LENGTH_PREFIX_SIZE: usize = 4;

/// Encode a serializable value as a length-delimited CBOR frame.
pub fn encode_frame<T>(value: &T) -> Result<Vec<u8>, FrameError>
where
    T: Serialize,
{
    let mut payload = Vec::new();
    ciborium::into_writer(value, &mut payload)?;

    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::FrameTooLarge {
            size: payload.len(),
            max: MAX_FRAME_SIZE,
        });
    }

    let payload_len = u32::try_from(payload.len()).expect("frame limit fits in u32");
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_SIZE + payload.len());
    frame.extend_from_slice(&payload_len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode one complete length-delimited CBOR frame.
///
/// This helper expects exactly one complete frame in `frame`; trailing bytes are
/// rejected so callers do not accidentally ignore an extra request.
pub fn decode_frame<T>(frame: &[u8]) -> Result<T, FrameError>
where
    T: DeserializeOwned,
{
    if frame.len() < LENGTH_PREFIX_SIZE {
        return Err(FrameError::IncompleteLengthPrefix { size: frame.len() });
    }

    let payload_len = u32::from_be_bytes(
        frame[..LENGTH_PREFIX_SIZE]
            .try_into()
            .expect("slice length checked"),
    ) as usize;

    if payload_len > MAX_FRAME_SIZE {
        return Err(FrameError::FrameTooLarge {
            size: payload_len,
            max: MAX_FRAME_SIZE,
        });
    }

    let expected_len = LENGTH_PREFIX_SIZE + payload_len;
    if frame.len() < expected_len {
        return Err(FrameError::IncompletePayload {
            expected: payload_len,
            actual: frame.len() - LENGTH_PREFIX_SIZE,
        });
    }
    if frame.len() > expected_len {
        return Err(FrameError::TrailingBytes {
            trailing: frame.len() - expected_len,
        });
    }

    let payload = &frame[LENGTH_PREFIX_SIZE..];
    let mut cursor = Cursor::new(payload);
    let value = ciborium::from_reader(&mut cursor)?;
    if cursor.position() as usize != payload.len() {
        return Err(FrameError::TrailingCborBytes {
            trailing: payload.len() - cursor.position() as usize,
        });
    }

    Ok(value)
}

/// Errors produced while encoding or decoding a length-delimited CBOR frame.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The value could not be serialized into CBOR.
    #[error("CBOR encode error: {0}")]
    Encode(#[from] ciborium::ser::Error<std::io::Error>),
    /// The payload could not be deserialized from CBOR.
    #[error("CBOR decode error: {0}")]
    Decode(#[from] ciborium::de::Error<std::io::Error>),
    /// The byte slice ended before the four-byte length prefix was complete.
    #[error("frame length prefix is incomplete: got {size} bytes")]
    IncompleteLengthPrefix {
        /// Number of bytes available for the length prefix.
        size: usize,
    },
    /// The byte slice ended before the declared payload length was available.
    #[error("frame payload is incomplete: expected {expected} bytes, got {actual}")]
    IncompletePayload {
        /// Declared payload length in bytes.
        expected: usize,
        /// Actual payload bytes available after the length prefix.
        actual: usize,
    },
    /// The declared or encoded payload exceeds [`MAX_FRAME_SIZE`].
    #[error("frame payload is too large: {size} bytes exceeds {max} byte limit")]
    FrameTooLarge {
        /// Payload size that exceeded the limit.
        size: usize,
        /// Maximum accepted payload size.
        max: usize,
    },
    /// Additional bytes followed an otherwise complete frame.
    #[error("frame has {trailing} trailing bytes after payload")]
    TrailingBytes {
        /// Number of bytes remaining after the declared payload.
        trailing: usize,
    },
    /// Additional bytes remained after decoding one CBOR value from the payload.
    #[error("CBOR payload has {trailing} trailing bytes")]
    TrailingCborBytes {
        /// Number of unused bytes remaining inside the CBOR payload.
        trailing: usize,
    },
}

#[cfg(test)]
mod tests;
