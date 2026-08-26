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

Supported options are `--tmp-dir <path>`, `--log-dir <path>`, `--log-requests`,
`--binary-path <dir>`, `--nostr-rs-relay-bin <path>`,
`--push-gateway-bin <path>`, `--bitcoind-bin <path>`,
`--fleet-manager-bin <path>`, `--liquidity-manager-daemon-bin <path>`,
`--gatewayd-bin <path>` and `--gateway-cli-bin <path>`.
`--log-requests` writes simple client connection, disconnect, request, and error
lines to stderr for persistent development servers. If `--tmp-dir` is omitted in
serve mode, the server uses a stable development temp root under the system temp
directory (`defe-dev-server`), so logs and resources are predictable across
`cargo watch` restarts.

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

## Disposable environment

`defe env [OPTIONS] [-- COMMAND...]` forms a seven-guardian federation, connects
a gateway, advertises FLIP, then starts `COMMAND` with a ready-to-use environment.
With no command it starts `$SHELL`. The composer owns every connection-scoped
lease until that child exits; child exit is the teardown boundary and its status
is preserved. A signal sent to a foreground job in the interactive shell remains
inside that job's terminal process group. Terminating the outer `defe` process
also reaches the composer during setup. On command exit or external termination,
the composer stops, terminates, and reaps every setup or command descendant,
including foreground jobs and background or disowned jobs in separate process
groups, before it marks the environment stopped and releases leases. A nested
PID namespace supplies the kernel-owned containment boundary, while terminal
sessions and job-control process groups remain unchanged. This strict descendant
boundary requires Linux pidfds and enabled unprivileged user namespaces.
Normal exit codes and native terminating signals cross both the composer and
outer `defe` boundaries unchanged.

The child receives `DEFE_ENV=1`, `DEFE_ENV_SCHEMA_VERSION=1`, paths for the root,
manifest, secrets, logs, invite, FI state, and Iroh routes, plus the stable local
Nostr, gateway, and FLIP endpoint variables. `$DEFE_ENV_BIN_DIR` is prepended to
`PATH`. Its private cross-shell tools include `defe-env-info`, `fman-1` through
`fman-7`, `fi-cli`, `gateway`, `bitcoin-cli`, `fman-ui`, `fees`, and `traffic`. (`fi` is a
POSIX shell keyword and therefore cannot be a cross-shell executable name.) Every service
wrapper selects the exact binary, state, endpoint, and dummy credential chosen by
the composer and forwards its remaining arguments unchanged.

`fees show --guardian N` and `fees collect --guardian N|--all` invoke FMan's real
guardian-fee admin path with the formed seat IDs. Collection prints a fresh
post-collect status. These commands do not synthesize remittances or imply that
traffic accrued production payer fees.

`fees synthetic-remit --guardian N --amount-msats AMOUNT` prepares one
collectable remittance through the real wallet-v2 and stability-pool path. It
reuses a dedicated disposable `fi-cli payment-wallet`, funds it when needed,
seals a mint/send breakdown to the FMan's actual remittance account, and waits
for `fees show` to observe the result. It then prints the exact `fees show` and
`fees collect` next steps. This is deliberately synthetic: production payer
accrual was bypassed, so it does not validate Fedi app accrual, 4:1:1 splitting,
threshold accumulation, or scheduling. Every successful invocation adds a new
remittance; after an uncertain failed invocation, inspect `fees show` before
retrying.

`traffic connections [--users N] [--duration-secs S]` repeatedly downloads client
configuration over real federation API connections through the pinned
Fedimint 0.11.1 `fedimint-load-test-tool`. Its sustained-connection subcommand assumes endpoint
URLs have TCP ports and panics on the formed federation's portless Iroh URLs, so
the wrapper uses the tool's compatible config-download mode. Calls are serialized
and time-bounded. User counts are limited to 1,000 and connection duration to one
hour. The defaults are 10 users and 60 seconds.

`traffic mint --users N --notes-per-user N` currently fails with an explicit
unsupported-mode error. The formed federation has `mintv2` and `walletv2`, while
the Fedimint 0.11.1 load tool hard-codes its v1 mint client and attempts funding
through a v1 wallet. A mintv2-capable upstream load path is required.

`traffic lightning --users N --invoices-per-user N` likewise fails explicitly.
The Fedimint 0.11.1 tester forbids creating an invoice on the same gateway that
pays it, and the composed environment does not yet provide an independent invoice
source. All traffic modes state that ordinary federation operations do not cause
or prove production Fedi fee accrual.

The mode-0600 JSON manifest changes atomically from `ready` to `stopped` before
leases are released. Credentials remain in the mode-0600 `secrets.json` and in
mode-0700 generated wrappers beneath the mode-0700 environment root. Successful
commands remove the temporary root unless `--keep-temp`; failures preserve it by
default unless `--no-keep-temp-on-failure` is selected.

`--complete-liquidity` remains reserved and fails explicitly. Use `just defe-env`
to build the selected binaries and enter the environment. The environment-only
`--fedimint-load-test-tool-bin PATH` option selects an exact trusted load-tool
binary when it appears immediately after `env`.
