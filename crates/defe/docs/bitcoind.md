# Bitcoind resource

`Bitcoind` is a local Bitcoin Core regtest resource for integration tests that
need a real Bitcoin RPC endpoint. It is primarily used by Fleet Manager 0.1 E2E
work to feed `fleet-manager --manifold-environment development --bitcoind-*`
arguments; the environment profile supplies regtest.

## API shape

`defe-api` exposes:

```rust
ResourceRequest::Bitcoind(BitcoindRequest { sharing })
ResourceDescriptor::Bitcoind(BitcoindInfo { .. })
SharedResourceKey::Bitcoind
```

`BitcoindInfo` includes:

- `rpc_url`
- `rpc_host`
- `rpc_port`
- `p2p_port`
- `rpc_username`
- `rpc_password`
- `data_dir`

The descriptor intentionally contains credentials because `defe` resources are
local trusted test resources scoped to the child command or persistent dev
server.

## Process behavior

The server starts the configured `bitcoind` binary in regtest mode with loopback
RPC and P2P listeners. The resource is considered ready once the RPC TCP port
accepts connections. Startup failure returns `ResourceStartFailed` with a log
path in the message.

The node enables the full transaction index (`txindex=1`) so consumers can use
`getrawtransaction` for transactions created by other processes and wallets in
the same E2E stack, such as a gatewayd withdrawal observed by FLIP.

The default binary resolution follows other process resources: explicit
`--bitcoind-bin`, then each `--binary-path <dir>`, then `PATH`.

## Stable allocation and restart

Each bitcoind slot owns stable:

- RPC port
- P2P port
- RPC username/password
- data directory under `resources/bitcoind/<resource-id>/`
- generation-specific log path

`RestartMode::IfExited` and `RestartMode::Force` preserve those stable fields.
Shared bitcoind requests use `SharedResourceKey::Bitcoind`; exclusive requests
always allocate a new slot.

## CLI environment

`defe-cli --request-bitcoind[=shared|exclusive] <cmd...>` keeps the lease alive
until the child exits and exports:

- `DEV_DEFE_BITCOIND_URL`
- `DEV_DEFE_BITCOIND_RPC_HOST`
- `DEV_DEFE_BITCOIND_RPC_PORT`
- `DEV_DEFE_BITCOIND_P2P_PORT`
- `DEV_DEFE_BITCOIND_RPC_USERNAME`
- `DEV_DEFE_BITCOIND_RPC_PASSWORD`
- `DEV_DEFE_BITCOIND_DATA_DIR`

Fleet Manager E2E harnesses should pass these through to the daemon
`--bitcoind-url`, `--bitcoind-username`, and `--bitcoind-password` arguments.

## Tests

Default tests should cover API round trips, CLI parsing/env export, resource
manager mapping, and stable path helpers without requiring a real `bitcoind`
binary. A real bitcoind lifecycle probe may be added later, but it must be
ignored or run under an explicit opt-in environment gate like the real relay
tests.
