# Nostr relay resource

The first real resource is a local `nostr-rs-relay` instance.


## Descriptor

`NostrRelayInfo` should include:

```rust
pub struct NostrRelayInfo {
    pub url: String,
    pub host: String,
    pub port: u16,
    pub data_dir: std::path::PathBuf,
}
```

The relay:

- host is `127.0.0.1`
- URL format is `ws://127.0.0.1:<port>`
- data directory is the relay database directory


## Startup

Startup steps:

1. Allocate one TCP port through `defe-portalloc`.
2. Create `resources/nostr-relay/<resource-id>/db`.
3. Write `resources/nostr-relay/<resource-id>/config.toml`.
4. Spawn `nostr-rs-relay --config <config.toml>` with `RUST_LOG` set.
5. Write stdout and stderr to `logs/nostr-relay-<resource-id>.log`.
6. Poll TCP connect to `127.0.0.1:<port>` until ready or timeout.
7. Return the descriptor.

The relay reads its log level from `RUST_LOG` and prints nothing when that
variable is unset, so the driver supplies `info` when it inherits no value. An
inherited value wins. Without this the log file is empty on every run and a
start failure has no evidence to quote.

A start failure quotes the end of the log in its message rather than only the
path. The log does not always outlive the run that produced it: a Nix build
sandbox discards its build directory, so a message citing a path alone is
unreadable by the time anybody sees it.

The readiness budget is 60 seconds, the same as the push gateway. The relay
binds its listener only after it builds three SQLite connection pools and
migrates the database, so readiness is disk-bound rather than
process-spawn-bound. A relay whose port is already taken panics and exits in
milliseconds, and the liveness check reports that immediately, so the budget
does not delay the common failure.

Config template:

```toml
[database]
data_directory = "<data-dir>"

[network]
address = "127.0.0.1"
port = <port>
```


## Binary location

Lookup order:

1. Explicit server option such as `--nostr-rs-relay-bin <path>`.
2. Existing `<binary-path>/nostr-rs-relay` from repeatable `--binary-path <dir>` entries, in CLI order.
3. `nostr-rs-relay` from `PATH`.

The project flake should include `pkgs.nostr-rs-relay` so it is available in dev shells.


## Sharing

Shared relay requests use one singleton key. The shared relay starts only when
first requested. Later shared relay requests reuse it while it remains
compatible and leased.

Exclusive relay requests always start a separate relay.


## Restart

No automatic restart after process exit.

Supported client requests:

- `IfExited`: restart the relay if the child process exited.
- `Force`: kill the relay if running, then start it again.

Restart should reuse the same port, config, log path, and data directory.


## Tests

Default tests should use fake process resources and unit tests.

Real `nostr-rs-relay` tests are opt-in and should be skipped unless a specific env var or ignored-test selection enables them.
