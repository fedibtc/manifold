# Current argument

## Argument

**L1 (`code`) — FLIP uses the shared default 128-permit protocol.** The public
RPC server constructs `IrohProtocol::new`; its default protocol constructor
sets one shared semaphore to 128 permits, 1 MiB request and encoded-response
limits, and 10-second initial-frame and response-write deadlines for every
accepted public stream ([`public.rs`](../src/public.rs),
[`iroh_protocol.rs`](../../fedi-iroh-rpc/src/iroh_protocol.rs)).

**L2 (`enum` + `code`) — every incomplete request's permit has a finite
lifetime through response abandonment.** The sole `ProtocolHandler::accept`
loop acquires one owned permit and detaches one task. The task's only pre-frame
read is `recv.read_to_end(max_request_bytes)`, wrapped in
`tokio::time::timeout`. Normal completion decodes and invokes the handler;
byte/read errors and expiry produce a transport response. The permit remains
owned through bounded response encoding and a timed response write, and drops
after that write completes or is cancelled at its deadline. No deadline
surrounds an already-decoded service handler, so client disconnects still
cannot cancel its run-to-completion work.

**L3 (`code` + `axiom`) — a legitimate stream cannot be overtaken while it
waits for a permit.** Each connection's sole accept loop calls
`acquire_owned` immediately after `accept_bi` returns. By A2, a legitimate
accepted stream eventually reaches that acquisition. At that instant only a
finite FIFO queue precedes it. By L2/A2, every incomplete predecessor
eventually drops its permit after the request and, if needed, response timeout
observations; any attacker
replenishment queues behind the legitimate stream. The legitimate stream
therefore eventually acquires a permit and reaches frame decoding.

**L4 (`test`) — configured partial-stream saturation releases permits.**
`partial_streams_release_handler_permits_after_the_initial_frame_deadline`
opens exactly the configured two partial streams, waits until they hold every
permit, and then asserts a separate legitimate RPC completes within two
seconds. The test fails if incomplete streams do not eventually release all
permits and admit a waiting legitimate RPC.

**L5 (`test`) — decoded work survives a disconnect.**
`decoded_handler_runs_after_client_disconnect` sends and finishes a valid
request, waits until its handler begins, closes the client connection, and
then allows the handler to complete. It fails if a pre-frame deadline or
connection cancellation reaches decoded service work.

By L1/L2 and A2, every incomplete permit holder drops its permit once the
runtime observes the finite request and response deadlines. L3 covers
adversarial replenishment, L4 exercises partial-stream saturation, and L5 preserves
the intentional run-to-completion boundary. Therefore an adversary cannot retain all finite permits
indefinitely before frame decoding.

## Residual windows

- This is a transport-availability claim about incomplete pre-frame streams.
  A decoded request may intentionally retain a handler permit while its service
  work runs; FLIP's application-level admission and service-time limits govern
  that separate work.
- Network/host-level connection limits may prevent the adversary from opening
  128 streams, but they are not an application-level admission defense and do
  not contribute to this argument.

## Weakest links

1. **L2 (`enum`/`code`)** — request read, permit, response encoding/write, and
   decoded-handler cancellation sites need regeneration whenever shared RPC
   transport changes.
2. **L3 (`code`/`axiom`)** — continuous admission needs both Iroh handler
   dispatch and Tokio's fair semaphore scheduling.
3. **L4 (`test`)** — the regression fixture exercises a reduced configured
   limit, relying on the common constructor and handler path for FLIP's 128
   permits.
4. **L5 (`test`)** — the disconnect fixture covers the deliberate detached
   handler boundary rather than every possible service implementation.
5. **A1–A2 (`axiom`)** — Iroh and Tokio asynchronous semantics bottom out
   outside this record.
