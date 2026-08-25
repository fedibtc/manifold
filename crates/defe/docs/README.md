# Defe implementation documentation

This directory contains implementation-facing documentation for `defe`, the
local test resource runner. Keep it synchronized with API, CLI, resource, and
testing behavior.

The durable component map and cross-crate protocol contract are in
[`../specs/`](../specs/). These documents describe local APIs and operational
details that do not need Linked Specs records.


## Documentation map

- `architecture.md` — crate layout, process model, temp layout, lifecycle.
- `rpc.md` — Unix socket transport, framing, protocol types, errors.
- `resources.md` — ownership, sharing, explicit release, restartable slots.
- `nostr-relay.md` — Nostr relay resource behavior.
- `bitcoind.md` — Bitcoin Core regtest resource behavior.
- `flip.md` — FLIP daemon resource behavior.
- `fman.md` — exclusive Fleet Manager resource behavior.
- `gatewayd.md` — shareable Fedimint gateway daemon behavior.
- `cli.md` — `defe` and `defe-cli` command behavior.
- `portalloc.md` — cross-process port allocator crate.
- `testing.md` — default tests and opt-in real relay tests.
- `stages.md` — current prospective work.


## Current decisions

- Use CBOR over length-delimited Unix socket frames.
- `defe exec <cmd...>` performs final cleanup when `<cmd...>` exits.
- Client connection drop releases all resources owned by that connection.
- Explicit resource release is part of the API.
- No automatic restart of dead resources.
- Client-requested restart is supported with `IfExited` and `Force` modes.
- Shared resources are lazy and opportunistic.
- `NostrRelayInfo` includes URL, host, port, and data directory.
- `PushGatewayInfo` includes URL, host, port, app id, and SQLite database path.
- `BitcoindInfo` includes RPC URL/host/port, P2P port, RPC credentials, and data directory.
- `FlipInfo` includes FLIP's Admin API, stable data and trust-fixture paths, and provider public key.
- `FmanInfo` includes the Fleet Manager locator, Admin API, and stable data directory.
- `GatewaydInfo` includes the gateway administrative API and credential.
- Real external-process integration tests are opt-in or require `defe exec` / `just defe-serve`.
- Temp data is cleaned after success and preserved after failure by default.
