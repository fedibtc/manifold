# CLAIM-public-rpc-slow-stream-exhaustion: Public rpc slow stream exhaustion

An unauthenticated remote peer cannot indefinitely prevent all new FLIP Public
Liquidity API RPC streams from reaching frame decoding by holding the protocol's
finite stream-handler permits with incomplete streams.

The adversary may establish authenticated Iroh transport connections and open
bidirectional streams but sends no complete RPC frame, presents no FLIP
signature or endorsement, and keeps those streams open. It may also disconnect
other connections and schedule ordinary legitimate RPC attempts. It cannot
break Iroh transport confidentiality/integrity, kill the daemon, or exhaust
resources outside this protocol's configured stream limit or prevent the Tokio
runtime's timer driver and workers from making progress.

## Status

Unverified.

## Assumptions

- **A1 — Iroh stream liveness.** A remote peer can keep an accepted
  bidirectional stream open without finishing its receive half, and
  `read_to_end` does not return until a stream finishes, errors, or exceeds its
  byte limit.
- **A2 — Tokio/Iroh progress and fair semaphore acquisition.** While the Tokio
  runtime's timer driver and workers make progress, Iroh dispatches an accepted
  stream to its connection's `ProtocolHandler::accept` loop, and
  `tokio::time::timeout` eventually observes expiry for a pending
  finite-duration future and cancels it. A QUIC response write can remain
  pending while its peer does not read, and cancelling that write abandons its
  future. Tokio semaphore acquisition is fair:
  a finite FIFO queue precedes every acquisition request, and later requests
  cannot overtake it. An acquired owned semaphore permit remains held only
  until its owning scope drops it.
