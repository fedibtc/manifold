# Current argument

## Argument

**L1 (`code`) — FLIP uses one finite shared response-task budget.** The public
RPC server constructs `IrohProtocol::new`, whose semaphore has 128 permits. Its
accept loop acquires one owned permit before spawning a stream task, and the
task owns that permit until it has completed or abandoned response I/O
([`public.rs`](../src/public.rs),
[`iroh_protocol.rs`](../../fedi-iroh-rpc/src/iroh_protocol.rs)).

**L2 (`code` + `test`) — transport serialization is structurally bounded.**
The default complete encoded-frame cap is 1 MiB. `BoundedWriter::write` checks
the prospective encoded length before extending its `Vec`; overflow returns
`ResponseTooLarge`. The exact-cap/cap-plus-one test covers both sides of the
boundary and incremental writes across the initial allocation. The bound is on
encoded length; allocator bookkeeping and capacity rounding are outside it.

**L3 (`code` + `test`) — an oversized handler body is not retained across
response I/O.** `encode_response` owns the handler body only inside the bounded
encoding attempt. Its frame drops before the caller constructs and awaits the
small `ResponseTooLarge` fallback write. The integration test observes only
that fallback on the wire. The handler and generated server may already have
allocated an arbitrarily large body before this transport boundary; L2–L3
prevent slow readers from retaining it, not its initial allocation.

**L4 (`code` + `axiom`) — every response await has a finite permit lifetime.**
The sole response await is `send.write_all`, inside the configured response
timeout; `finish` is attempted in the same timed future after a successful
write. Successful completion, write failure, deadline cancellation, bounded
encoding failure, and an unwritable fallback all return from the task and drop
its permit. By A1–A3 there is no completed-handler response path that waits
indefinitely.

**L5 (`test`) — slow readers exhaust only the configured slots until their
deadline.** `slow_readers_are_task_capped_and_response_timeout_releases_permits`
gives a peer tiny QUIC receive windows and leaves exactly two large responses
unread. It asserts both permits remain held and a third request stays queued
before the response deadline, then asserts that request completes after the
deadline releases the slots. The negative pre-deadline assertion fails under
the previous early-permit-release implementation.

By L1/A2, at most 128 response tasks exist after handler return. By L2–L3, each
task reaching response I/O retains at most one configured-size encoded frame,
not an oversized handler body. By L4/A1–A3 each task abandons that I/O within
the finite response deadline, and L5 exercises saturation, non-admission before
the deadline, and later release. A peer that does not read therefore cannot
cause indefinite response-task or encoded-buffer accumulation.

## Residual windows

- Service handlers deliberately run to completion after client disconnect.
  Their work, and the generated server's unbounded `Vec` serialization of a
  typed return before this claim's boundary, require application-level bounds.
  One handler can still allocate a response too large for the process before
  the transport can reject and promptly drop it.
- The semaphore bounds spawned stream tasks globally, not accepted QUIC
  connections. Each connection's accept loop may hold one accepted stream while
  waiting for a permit, outside the detached response-task population claimed
  here.
- A custom response cap too small to encode `ResponseTooLarge` closes the
  stream without a fallback frame. It does not retain the oversized body or
  task.

## Weakest links

1. **L4 (`code`/`axiom`)** — the permit scope and enumeration of response awaits
   need regeneration whenever the shared protocol task changes.
2. **L3 (`code`/`test`)** — Rust lexical drop order keeps the oversized body out
   of the fallback await; the wire test does not directly measure allocations.
3. **L5 (`test`)** — the fixture uses reduced limits and local QUIC flow control,
   relying on the same production constructor and task path.
4. **L1–L2 (`code`)** — default limits and bounded-writer ordering remain local
   source properties.
5. **A1–A3 (`axiom`)** — runtime, QUIC cancellation, semaphore, and finite
   serialization semantics bottom out outside this repository.
