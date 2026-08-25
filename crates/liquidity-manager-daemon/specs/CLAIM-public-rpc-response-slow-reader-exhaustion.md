# CLAIM-public-rpc-response-slow-reader-exhaustion: Public rpc response slow reader exhaustion

After a FLIP Public Liquidity API handler returns, an unauthenticated peer that
stops reading cannot make the shared RPC transport indefinitely accumulate
detached response tasks or their encoded write buffers.

The peer may establish many authenticated Iroh connections, send complete valid
requests that reach public handlers, advertise tiny QUIC receive windows, and
leave every response unread. It cannot prevent Tokio workers or timers from
making progress. This claim begins when a service handler returns its encoded
body; it does not bound service work, allocations inside a handler, or the
generated server's initial serialization of a typed service return.

## Status

Unverified.

## Assumptions

- **A1 — Tokio/Iroh progress.** A response write may remain pending while its
  peer does not read. With runtime workers and the timer driver progressing,
  `tokio::time::timeout` eventually cancels that pending write at its finite
  deadline, and dropping the task's send stream abandons further response I/O.
- **A2 — semaphore ownership.** Tokio's owned semaphore permit remains held
  until its owner drops and no more tasks can acquire permits than the
  semaphore's fixed capacity.
- **A3 — finite synchronous progress.** Given an already-allocated finite
  handler body and available process memory, CBOR serialization through a
  writer that either accepts or rejects every write completes in finite local
  work. This says nothing about how large the handler body may be.
