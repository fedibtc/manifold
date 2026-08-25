# fedi-iroh-rpc

Small typed RPC helper for service traits over iroh bidirectional streams.

Each RPC call opens one iroh bi stream, writes one encoded request frame, finishes the send side, then reads one encoded response payload.

## Define a service

```rust
use fedi_iroh_rpc::{service, RpcError};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, Deserialize, Serialize)]
pub enum Error {
    #[error("transport: {0}")]
    Transport(String),
}

impl From<RpcError> for Error {
    fn from(error: RpcError) -> Self {
        Self::Transport(error.to_string())
    }
}

#[derive(Deserialize, Serialize)]
pub struct PingRequest {
    pub message: String,
}

#[derive(Deserialize, Serialize)]
pub struct PingResponse {
    pub message: String,
}

#[service]
pub trait PingService {
    async fn ping(&self, request: PingRequest) -> Result<PingResponse, Error>;
}
```

The macro keeps the trait and generates:

- `PingServiceClient`
- `PingServiceServer<S>`

## Serve it with iroh

```rust
use fedi_iroh_rpc::IrohProtocol;
use iroh::{Endpoint, endpoint::presets, protocol::Router};

const ALPN: &[u8] = b"example/ping/1";

let endpoint = Endpoint::bind(presets::N0).await?;
let server = PingServiceServer::new(MyPingService);
let router = Router::builder(endpoint)
    .accept(ALPN, IrohProtocol::new(server))
    .spawn();
```

## Call it

```rust
let endpoint = Endpoint::bind(presets::N0).await?;
let connection = endpoint.connect(router.endpoint().addr(), ALPN).await?;
let client = PingServiceClient::new(connection);
let response = client.ping(PingRequest { message: "hello".into() }).await?;
```

## Calling convention

The iroh connection is established by the caller using the service's ALPN. After that, each service method call uses exactly one new bidirectional iroh stream on that connection.

Client side:

1. Open one bidirectional stream with `Connection::open_bi`.
2. Encode the typed request value into a byte payload.
3. Encode and write one request frame:

   ```rust
   struct RequestFrame {
       version: u16,    // currently 1
       method: String, // service method name, e.g. "ping"
       body: Vec<u8>,  // encoded request value
   }
   ```

4. Finish the stream send side.
5. Read one bounded response frame from the receive side.

Server side:

1. Accept one bidirectional stream with `Connection::accept_bi`.
2. Read one bounded request frame before the server's initial-frame deadline.
3. Reject unsupported frame versions and unknown method names as transport errors.
4. Decode `RequestFrame::body` as that method's request type.
5. Call the matching service trait method.
6. Encode the method's full return value, normally `Result<Response, ServiceError>`.
7. Encode one size-bounded response frame, write it before the response deadline,
   then finish the stream send side:

   ```rust
   enum ResponseFrame {
       Service(Vec<u8>),   // encoded service method return value
       Transport(String), // transport-level failure message
   }
   ```

`ResponseFrame::Service` contains the service-level return value exactly as the trait method returned it, including service errors. `ResponseFrame::Transport` is reserved for framing, decoding, unknown-method, size-limit, and iroh transport failures.

There is no streaming request body and no streaming response body within an RPC
call. The finite stream-task limit covers request reading, service work,
response encoding, and response writing. The initial-frame deadline prevents
incomplete streams from retaining a task slot, while the response deadline
prevents an unread peer from retaining a slot and its bounded response buffer.
Neither deadline cancels a decoded service handler when its client disconnects.
Larger or long-lived exchanges should be modeled as separate service methods or
a different protocol.

The response-frame cap applies after the generated server has encoded the typed
service return into its body `Vec`. It bounds transport framing and retained
write buffers, not allocations performed inside a service handler or that
initial service-return encoding. Custom response limits that are too small to
hold a transport error may close a rejected stream without an error frame.

The current implementation uses serde-compatible binary encoding internally for frames and bodies. Treat the Rust frame shapes and method semantics above as the calling convention; do not rely on a stable raw byte format unless this crate later promises one.

## Requirements

Request types, return types, and service errors must be serde-serializable with serde.

Generated clients convert transport failures into the service method return type through `RpcReturn`. For normal `Result<T, E>` methods, this means `E: From<RpcError>`.

Consumers that must distinguish a local transport failure from a remote
service error can use the generated typed transport view (for example,
`client.transport().ping(request)`) and keep its outer `RpcResult` in their
local adapter. The inner value remains the exact serialized service return
type; transport errors must not be added to the service's serialized error
vocabulary merely to classify a local call failure.

## Examples

Run:

```sh
cargo run -p fedi-iroh-rpc --example ping
```
