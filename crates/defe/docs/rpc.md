# RPC protocol

Clients talk to the server over the Unix socket named by `DEV_DEFE_SOCKET_PATH`.


## Transport

- Unix domain stream socket.
- Socket path lives under a private temp directory.
- No authentication beyond filesystem permissions.
- One connection can issue multiple sequential requests.
- No multiplexing.


## Framing

Use length-delimited frames:

```text
u32 big-endian payload length
CBOR payload bytes
```

Limits:

- Reject frames larger than a small fixed maximum.
- The maximum is 1 MiB.
- Treat malformed CBOR as a protocol error and close or error the request.


## Serialization

- Use serde-compatible types.
- Use `ciborium` for CBOR.
- Keep API structs in `defe-api`.
- Avoid server-only dependencies in `defe-api`.


## Request sketch

```rust
pub enum Request {
    Ping,
    Allocate(ResourceRequest),
    Release(ResourceHandleId),
    Restart {
        handle_id: ResourceHandleId,
        mode: RestartMode,
    },
}

pub enum ResourceRequest {
    NostrRelay(NostrRelayRequest),
    PushGateway(PushGatewayRequest),
    Bitcoind(BitcoindRequest),
    Fman(FmanRequest),
    Flip(FlipRequest),
    Gatewayd(GatewaydRequest),
}

pub enum SharingMode {
    Shared,
    Exclusive,
}

pub enum RestartMode {
    IfExited,
    Force,
}
```

`Restart` requires the caller to own the handle on that connection.


## Response sketch

```rust
pub enum Response {
    Pong,
    Resource(ResourceLease),
    Released,
    Error(ApiError),
}

pub struct ResourceLease {
    pub handle_id: ResourceHandleId,
    pub descriptor: ResourceDescriptor,
}

pub enum ResourceDescriptor {
    NostrRelay(NostrRelayInfo),
    PushGateway(PushGatewayInfo),
    Bitcoind(BitcoindInfo),
    Fman(FmanInfo),
    Flip(FlipInfo),
    Gatewayd(GatewaydInfo),
}
```

A restart response should return `Resource(ResourceLease)` with the same logical handle and the latest descriptor.


## Error categories

API errors should be structured enough for clients and tests:

- invalid request
- unknown handle, including a handle id that is not owned by this connection
- resource kind unavailable
- resource start failed
- resource restart refused because it is still running and mode is `IfExited`
- protocol decode error
- internal server error

Human-readable messages are fine, but callers should not need string matching for common categories. The API type may contain more granular categories than the current server needs; the current handle checks intentionally return `UnknownHandle` when a handle is missing from the caller's connection state.
