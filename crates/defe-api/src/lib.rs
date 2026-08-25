//! Shared API types for the `defe` testing framework.

mod frame;
mod types;

pub use frame::{FrameError, MAX_FRAME_SIZE, decode_frame, encode_frame};
pub use types::{
    ApiError, ApiErrorKind, NostrRelayInfo, NostrRelayRequest, PushGatewayInfo, PushGatewayRequest,
    Request, ResourceDescriptor, ResourceHandleId, ResourceLease, ResourceRequest, Response,
    RestartMode, SharingMode,
};
pub use types::{
    BitcoindInfo, BitcoindRequest, FlipInfo, FlipRequest, FmanInfo, FmanRequest, GatewaydInfo,
    GatewaydRequest,
};

/// Environment variable containing the path to the active `defe` Unix socket.
pub const DEV_DEFE_SOCKET_PATH: &str = "DEV_DEFE_SOCKET_PATH";
