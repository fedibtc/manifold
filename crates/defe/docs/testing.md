# Testing

Regular workspace tests must be reliable and should not require running external resource processes such as `nostr-rs-relay` or `fedi-decentralized-push-gateway`. Checks that are not wrapped in `defe exec` or run against `just defe-serve` should exclude the `tests-e2e` integration crate.


## Regular workspace tests

Default `cargo test --workspace --exclude tests-e2e` or `just test` coverage should include:

- API codec round trips.
- Protocol error decoding.
- Port allocator unit tests.
- Resource manager ownership tests with fake resources.
- Shared resource reuse and last-lease cleanup with fake resources.
- Exclusive resource isolation with fake resources.
- Explicit release behavior.
- Connection-drop cleanup.
- Server-finalization cleanup.
- Restart modes with fake resources.
- `defe exec` and `defe-cli ping` smoke tests if they do not need external binaries.
- Push-gateway resource tests should either use fakes/unit coverage or run under `defe exec` / `just defe-serve` with `--binary-path` pointing at a directory containing a built `fedi-decentralized-push-gateway` binary.
- Bitcoind resource tests should cover API/CLI/resource-manager wiring without requiring a real `bitcoind` binary by default. Real bitcoind lifecycle checks, if added, must be ignored or gated by an explicit opt-in environment variable.


## Opt-in ignored real relay tests

The low-level real `nostr-rs-relay` integration tests in `defe` and `defe-client` are opt-in. They are ignored by default and also check an explicit env var before doing real work:

```bash
DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1 cargo test -p defe real_nostr_relay_lifecycle_and_restart -- --ignored
```

Use `DEV_DEFE_NOSTR_RS_RELAY_BIN=/path/to/nostr-rs-relay` to test a binary that is not on `PATH`.

`defe-client` also has an opt-in end-to-end wrapper probe that runs the real `defe` server and `defe-cli --request-relay`, then executes the integration-test binary as the child command to verify `DEV_DEFE_NOSTR_RELAY_URL`, `DEV_DEFE_NOSTR_RELAY_PORT`, and `DEV_DEFE_NOSTR_RELAY_DATA_DIR` describe a usable relay, publish a valid Nostr kind 1 event to it, and query the event back by id:

```bash
DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1 cargo test -p defe-client defe_cli_request_relay_through_real_defe_server -- --ignored --nocapture
```

It is ignored by default and should keep the same explicit env-var gate as the lower-level real relay tests.


## Normal AsyncDefeClient usage tests

For iterative development, use the persistent dev-server workflow:

```bash
nix develop
just defe-serve
```

Then, in another `nix develop` terminal:

```bash
cargo test
# or
cargo nextest run
```

The dev shell exports `DEFE_SOCKET="$PWD/.defe.sock"` and `DEV_DEFE_SOCKET_PATH` to the same workspace-local socket path. `just defe-serve` uses the same default when `DEFE_SOCKET` is unset, runs `systemfd --no-pid -s unix::$DEFE_SOCKET -- cargo watch ...`, and restarts the built `defe serve --listenfd --log-requests` when binaries in `${CARGO_TARGET_DIR:-target}/<profile-dir>` change so accepted clients and major requests are visible on stderr. It defaults to `debug`; `CARGO_PROFILE=debug` and `CARGO_PROFILE=dev` both use `target/debug`, while `release` or a custom profile uses `target/<profile>`. Do not rebuild/restart the server while a test run is in progress.

The unpublished `tests-e2e` crate contains ordinary cross-project tests that demonstrate normal `AsyncDefeClient::connect_from_env().await` usage. They are not ignored and do not use marker env vars. They expect a running `defe` server and may request real relay, push-gateway, and bitcoind resources through that server.
`tests-e2e` resource coverage requires the external resource binaries to be available to the server. `just defe-serve` builds them and starts `defe` with `--binary-path ${CARGO_TARGET_DIR:-target}/<profile-dir>`; one-shot runs can pass `--binary-path target/debug` after building them.

Nix CI gives each long-running E2E partition its own `defe exec` server and
per-derivation port-allocation ledger. Linux runners may build concurrently
because Nix gives their sandboxes separate network namespaces. Darwin Nix
builds share host loopback, so their service-running test derivations form a
dependency chain and run serially. Without that ordering, independent ledgers
can reserve identical ports before either child service binds. Keep this
platform distinction until Darwin runners have a genuinely shared allocation
domain or isolated networks. The dependency chain covers one Nix test graph;
separate Darwin Nix or SelfCI invocations on the same host still require
external serialization and must not overlap. Darwin runner derivation names and
test-created subdirectories must also remain compact because Nix includes the
derivation name in `TMPDIR` and service sockets count the entire path against
the platform's Unix-domain socket limit.

SelfCI also finishes Darwin's independent compilation and static-check targets
before starting that service-running test graph. Otherwise concurrent Rust
builds can starve a seven-guardian formation long enough to consume its protocol
deadline even though the same formation completes promptly without compiler
contention. Linux keeps the fully concurrent schedule because its builders have
isolated networks and enough parallelism for the test graph.

Nix builds Nextest archives and executes them in separate derivations. Tests
that launch a workspace binary must therefore resolve that executable from the
runner's runtime bundle through a dedicated environment variable, with
`CARGO_BIN_EXE_*` retained only as the fallback for ordinary Cargo test runs.
An archived test must not rely solely on the compile-time `CARGO_BIN_EXE_*`
path: it names the artifact builder's private directory and is not relocatable.

Run them from another terminal while `just defe-serve` is running, or inside one-shot `defe exec` so the server socket and resource binaries are available:

```bash
cargo build -p defe -p fedi-decentralized-push-gateway
target/debug/defe --binary-path target/debug exec cargo test -p tests-e2e
```

Outside the persistent dev-server flow or `defe exec`, a plain `cargo test -p tests-e2e` is expected to fail with the client error explaining that `DEV_DEFE_SOCKET_PATH` must come from a running `defe` server.

Opt-in real relay lifecycle tests should verify:

- shared relay starts and returns a usable URL
- two shared leases return the same URL
- exclusive relay returns a different URL
- descriptor includes data directory
- restart after exit works
- forced restart works
- resource cleanup kills the process


## Commands

Useful checks, depending on the changed area:

```text
cargo fmt --check
cargo check --workspace --all-targets
cargo test --workspace --exclude tests-e2e
cargo build -p defe -p fedi-decentralized-push-gateway
target/debug/defe --binary-path target/debug exec cargo test -p tests-e2e
```
