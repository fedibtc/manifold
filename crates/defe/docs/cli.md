# CLI behavior

There are two binaries:

- `defe` from the server crate.
- `defe-cli` from the client crate.


## `defe exec`

Main form:

```text
defe [opts...] exec <cmd...>
```

Behavior:

1. Start a server and bind a Unix socket.
2. Set `DEV_DEFE_SOCKET_PATH` for `<cmd...>`.
3. Run `<cmd...>`.
4. Serve requests while the command runs.
5. Clean all resources when the command exits.
6. Exit with the same status as `<cmd...>`.

Options:

- `--tmp-dir <path>` chooses the temp root.
- `--keep-temp` preserves temp data after success or failure.
- `--no-keep-temp-on-failure` disables default failure preservation.
- `--log-dir <path>` optionally separates logs from temp data.
- `--binary-path <dir>` adds a directory searched for managed resource binaries before falling back to `PATH`; repeatable.
- `--nostr-rs-relay-bin <path>` chooses the relay binary explicitly.
- `--push-gateway-bin <path>` chooses the push gateway binary explicitly.
- `--bitcoind-bin <path>` chooses the Bitcoin Core binary explicitly.

Default temp behavior:

- success cleans temp data
- failure preserves temp data and logs


## Persistent `defe serve`

Development test runs can use a persistent socket-activated server:

```text
defe [opts...] serve --listenfd
```

`serve --listenfd` takes an inherited Unix listener using the `listenfd`/systemd socket activation protocol. It does not unlink or bind the socket path itself; `systemfd --no-pid -s unix::$DEFE_SOCKET -- defe serve --listenfd` owns the socket path and passes the listener fd. The inherited listener is set nonblocking before the normal accept loop starts.

Supported options are `--tmp-dir <path>`, `--log-dir <path>`, `--log-requests`, `--binary-path <dir>`, `--nostr-rs-relay-bin <path>`, `--push-gateway-bin <path>`, `--bitcoind-bin <path>`, `--fleet-manager-bin <path>`, `--liquidity-manager-daemon-bin <path>`, `--gatewayd-bin <path>`, and `--gateway-cli-bin <path>`. `--log-requests` writes simple client connection, disconnect, request, and error lines to stderr for persistent development servers. If `--tmp-dir` is omitted in serve mode, the server uses a stable development temp root under the system temp directory (`defe-dev-server`), so logs and resources are predictable across `cargo watch` restarts.

From the Nix dev shell, `just defe-serve` runs this mode through `systemfd` and `cargo watch` with `--log-requests` enabled. It does one initial build, watches `${CARGO_TARGET_DIR:-target}/<profile-dir>` for rebuilt binaries, and restarts `defe` with `--binary-path` pointing at that directory. `CARGO_PROFILE=debug` and `CARGO_PROFILE=dev` both use `target/debug`; `release` or a custom profile uses `target/<profile>`. The shell exports both `DEFE_SOCKET` and `DEV_DEFE_SOCKET_PATH` to the same workspace-local `.defe.sock` Unix socket path, and the recipe uses that same default if `DEFE_SOCKET` is unset. Run `just defe-serve` in one terminal and `cargo test` or `cargo nextest run` in another. Outside this flow, code using `AsyncDefeClient::connect_from_env().await` needs either `defe exec <cmd...>` or an already-running server with `DEV_DEFE_SOCKET_PATH` set.

## `defe-cli` wrapper

Primary forms:

```text
defe-cli [opts...] <cmd...>
defe-cli --request-relay <cmd...>
defe-cli --request-relay=shared <cmd...>
defe-cli --request-relay=exclusive <cmd...>
defe-cli --request-push-gateway <cmd...>
defe-cli --request-push-gateway=shared <cmd...>
defe-cli --request-push-gateway=exclusive <cmd...>
defe-cli --request-bitcoind <cmd...>
defe-cli --request-bitcoind=shared <cmd...>
defe-cli --request-bitcoind=exclusive <cmd...>
defe-cli --request-relay -- <cmd-with-leading-dash> [args...]
```

Future shape:

```text
defe-cli --request-relay --request-foo --request-bar=3 <cmd...>
```

Behavior with resource request flags:

1. Read `DEV_DEFE_SOCKET_PATH`.
2. Connect to the parent `defe` server.
3. Allocate every requested resource.
4. Export env vars for `<cmd...>`.
5. Run `<cmd...>`.
6. Exit with the child's status.
7. Drop the client connection, releasing all owned leases.

Behavior without resource request flags:

- Run `<cmd...>` directly and preserve its exit status.
- Do not require `DEV_DEFE_SOCKET_PATH`.

Nostr relay env vars:

- `DEV_DEFE_NOSTR_RELAY_URL`
- `DEV_DEFE_NOSTR_RELAY_PORT`
- `DEV_DEFE_NOSTR_RELAY_DATA_DIR`

Push gateway env vars:

- `DEV_DEFE_PUSH_GATEWAY_URL`
- `DEV_DEFE_PUSH_GATEWAY_PORT`
- `DEV_DEFE_PUSH_GATEWAY_APP_ID`
- `DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH`

Bitcoind env vars:

- `DEV_DEFE_BITCOIND_URL`
- `DEV_DEFE_BITCOIND_RPC_HOST`
- `DEV_DEFE_BITCOIND_RPC_PORT`
- `DEV_DEFE_BITCOIND_P2P_PORT`
- `DEV_DEFE_BITCOIND_RPC_USERNAME`
- `DEV_DEFE_BITCOIND_RPC_PASSWORD`
- `DEV_DEFE_BITCOIND_DATA_DIR`


## Client library

Rust tests and tools can use `defe_client::AsyncDefeClient::connect_from_env().await` to connect to the server socket exported by `defe exec <cmd...>`. If `DEV_DEFE_SOCKET_PATH` is missing, the constructor returns a typed, displayable error explaining that the code must run inside `defe exec <cmd...>` or have the env var set explicitly.


## Utility commands

```bash
defe exec defe-cli ping
```

A print-and-exit resource command may exist later, but it must clearly document that resources are released as soon as the command exits.

## Disposable staging

`defe staging` owns a private, foreground environment for humans, UIs, and
external E2E tests. It forms a seven-guardian federation, connects a gateway,
configures FLIP and publishes its advertisement, then writes and prints the
path to `env.json`. The manifest's `ready` field becomes true only after all of
those phases succeed.

The manifest contains endpoints and paths, not credentials. Credentials live
in a sibling mode-0600 `secrets.json` inside a mode-0700 staging directory.
Each FMan manifest entry exposes its HTTP API proxy base as `api_base_url` and
the exact POST endpoints as `auth_url` and `admin_url`; the base URL itself
does not serve a browser page.

The debug `fleet-manager` binary used by `just defe-staging` serves the HTTP
API but does not embed the browser dashboard. The ready output prints an exact
per-FMan Vite attach command and its loopback browser URL. Run the printed
`pnpm install` command once, then run one attach command at a time (the
dashboard uses fixed port 5174), open `http://127.0.0.1:5174`, and enter the
matching FMan password from `secrets.json`. The command also prints exact
`fman-cli` examples, Defe's process-log directory, and each FMan safe-journal
directory. For example:

```bash
jq -e '.ready == true' /path/printed/by/defe/env.json
```

Press Ctrl-C to close the owning Defe connection and tear every resource down.
Startup failures keep Defe's private temporary root by default, matching
`defe exec`; use `--no-keep-temp-on-failure` to opt out.

`--complete-liquidity` is reserved for driving the FI-funded liquidity
allocation through consensus registration. It currently fails explicitly
rather than making basic staging wait on that optional flow.

Use `just defe-staging` in a checkout. Direct invocation requires all resource
and composer binaries in `--binary-path`.
