# Architecture

`defe` is a local client-server test resource runner. The server owns scarce resources and child processes. Clients request leases over a Unix socket.


## Workspace crates

Implementation should use these crates:

- `crates/defe-api`
  - Shared request, response, descriptor, and error types.
  - Env constants.
  - CBOR frame encode and decode helpers if dependency-light.
- `crates/defe-portalloc`
  - Cross-process port allocation adapted from Fedimint `fedimint-portalloc`.
- `crates/defe`
  - Server binary named `defe`.
  - `defe exec <cmd...>` command runner.
  - Unix listener, resource manager, and process supervision.
  - This `specs/` directory lives here.
- `crates/defe-client`
  - Client library.
  - Binary named `defe-cli`.

Package names may use hyphens. Rust crate names may use underscores as required by Cargo.


## Server process model

`defe exec <cmd...>` runs one server for one command invocation.

Execution order:

1. Atomically create a private temp root with mode `0700`. The default root and
   internal socket names are deliberately compact so the Unix socket remains
   usable beneath long CI/Nix temporary paths, including Darwin's shorter
   `sockaddr_un.sun_path` limit.
2. Bind a Unix socket under the temp root.
3. Start the server accept loop.
4. Spawn `<cmd...>` with `DEV_DEFE_SOCKET_PATH` set.
5. Serve client requests while `<cmd...>` is running.
6. When `<cmd...>` exits, stop accepting new clients.
7. Drop all resource manager state.
8. Terminate every child process still owned by resources.
9. Exit with the child command's status.

The command process lifetime is the final boundary. Leaked or daemonized clients must not keep resources alive after command exit.


## Temp layout

Suggested temp root layout:

```text
<tmp-root>/
  s
  logs/
  resources/
    nostr-relay/
      <resource-id>/
        config.toml
        db/
    push-gateway/
      <resource-id>/
        push-gateway.sqlite
    bitcoind/
      <resource-id>/
        regtest/
```

Child-process resources currently include a Nostr relay, a push gateway, and a Bitcoin Core regtest node. Nostr relay slots own a config file, database directory, and WebSocket port. Push gateway slots own a stable loopback HTTP port, app id, and SQLite database path under `resources/push-gateway/<resource-id>/push-gateway.sqlite`. Bitcoind slots own stable loopback RPC/P2P ports, RPC credentials, and a data directory under `resources/bitcoind/<resource-id>/`.

FLIP slots own a `liquidity-manager-daemon` process, admin/public ports, and a
data directory under `resources/flip/<resource-id>/`. They do not own gatewayd:
the daemon's gateway is configured later by the consuming test through its
Admin API.

Default temp policy:

- command succeeds: remove temp root
- command fails: preserve temp root
- `--keep-temp`: preserve temp root even on success
- `--no-keep-temp-on-failure`: remove temp root even on failure


## Resource ownership levels

- Connection level: resources allocated through one client connection are owned by that connection.
- Explicit release level: a client can release one handle early.
- Server finalization level: command exit releases everything unconditionally.


## Non-goals

- Remote server transport.
- Network authentication beyond private Unix-socket directory permissions.
- Automatic resource restart after failure.
- Persistent resource state across separate `defe exec` invocations.
