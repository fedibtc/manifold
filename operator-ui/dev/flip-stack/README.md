# FLIP local dev stack — test the liquidity-provider UI against a real daemon

Runs the regtest dependency stack the PR #70 (`chore/initial-flip`) integration
tests use, so the operator UI can talk to a real `liquidity-manager-daemon`
instead of the mock server.

Components:

| Service     | Image                            | Host port         |
| ----------- | -------------------------------- | ----------------- |
| bitcoind    | `bitcoin/bitcoin:31.0` (regtest) | `127.0.0.1:18443` |
| gatewayd    | `fedimint/gatewayd:v0.11.1`      | `127.0.0.1:8175`  |
| nostr relay | `scsibug/nostr-rs-relay:0.10.0`  | `127.0.0.1:8081`  |
| FLIP daemon | host cargo run **or** nix image  | `127.0.0.1:8173` (admin, TCP), `127.0.0.1:8174` (public, Iroh/QUIC — UDP) |

Credentials (same as the integration harness — regtest only, do not reuse):

- bitcoind RPC: `bitcoin` / `bitcoin`
- gatewayd admin password: `testpassword`
- FLIP admin bearer token: `flip-local-admin-token` (or whatever you pass)

## Quick start (one command)

From the `operator-ui/` root:

```bash
pnpm flip:live         # docker deps + FLIP daemon + UI on :5173 (foreground)
pnpm flip:live:down    # stop daemon + docker deps (add --keep-data to keep volumes)
```

`flip:live` ([up.sh](up.sh)) starts the docker deps, waits for gatewayd to go
healthy, builds the daemon if its binary is missing, runs it (detached, on
`:8173`/`:8174`), reclaims port 5173 from any leftover dev server, then runs the
Vite dev server in the foreground with the proxy pointed at the real daemon.
The daemon runs in its own session, so **Ctrl+C stops only the UI** — the daemon
and docker keep running. `flip:live:down` ([down.sh](down.sh)) stops everything:
the daemon, the UI, and the docker deps.

Overridable env (defaults shown):

```bash
FLIP_DAEMON_REPO=<repo root>                      # where to build/run the daemon from (defaults to this repo)
FLIP_ADMIN_TOKEN=flip-local-admin-token           # bootstrap admin token
FLIP_DATA_DIR=/tmp/flip-dev-data                  # daemon SQLite/state dir
```

First run builds the daemon (slow, one-time — see the macOS caveats in step 2);
later runs reuse the binary and are fast. The daemon PID and logs live in
`.state/` (gitignored) — tail `dev/flip-stack/.state/daemon.log` if it won't
come up.

The rest of this doc is the manual, step-by-step version behind that script.

## 1. Start the dependency stack

```bash
cd operator-ui/dev/flip-stack
docker compose up -d
docker compose ps   # wait until gatewayd is healthy (first start takes ~1 min)
```

`bitcoind-init` mines 101 blocks once and exits — that's expected.

## 2. Start the FLIP daemon

### Option A — on the host with cargo (recommended on macOS)

Check out the PR branch (e.g. `git worktree add ../df-flip-pr70 origin/chore/initial-flip`).

macOS caveats (as of Jul 2026, verified working around both):

- `nix develop` fails on darwin — the dev shell's `selfci` uses the Linux-only
  `pidfd_open` syscall. Use a plain nixpkgs toolchain via `nix shell` instead.
- `netwatch` 0.5.0 (via iroh 0.90) fails to compile on macOS — it needs
  socket2's `all` feature but doesn't enable it. Local workaround: add to
  `crates/liquidity-manager-daemon/Cargo.toml` `[dependencies]` (do not commit):
  `socket2 = { version = "0.5", features = ["all"] }`
- Older checkouts may pin the public credential SDK to an SSH URL. Without a
  GitHub SSH key, rewrite that URL to anonymous HTTPS as below.

Build (from the PR checkout):

```bash
GIT_CONFIG_COUNT=1 \
GIT_CONFIG_KEY_0='url.https://github.com/.insteadOf' GIT_CONFIG_VALUE_0='ssh://git@github.com/' \
CARGO_NET_GIT_FETCH_WITH_CLI=true \
nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#pkg-config nixpkgs#cmake nixpkgs#libiconv \
  --command cargo build -p fedi-decentralized-liquidity-manager-daemon --bin liquidity-manager-daemon
```

Run:

```bash
mkdir -p /tmp/flip-dev-data
FM_IN_DEVIMINT=1 ./target/debug/liquidity-manager-daemon \
  run daemon \
  --manifold-environment development \
  --data-dir /tmp/flip-dev-data \
  --admin-bind-address 127.0.0.1:8173 \
  --public-bind-address 127.0.0.1:8174 \
  --bootstrap-admin-token flip-local-admin-token
```

(On Linux the plain `nix develop` + `cargo run` path works without any of the
workarounds, or use `nix build path:.#liquidityManagerDaemon`.)

### Option B — docker image (linux, or a mac with a nix linux builder)

On the PR branch:

```bash
just flip-docker-load     # nix build + docker load flip-liquidity-manager:<workspace-version>
GATEWAY_API_ADDR=http://gatewayd:8175 docker compose --profile daemon up -d
```

Note `GATEWAY_API_ADDR`: with the daemon inside compose, the gateway admin URL
must be the compose-network name (`http://gatewayd:8175`), not `127.0.0.1`.

Verify: `curl -s http://127.0.0.1:8173/health`

## 3. Point the UI at the daemon

The Vite dev server proxies `/admin` and `/health`. Default target is the mock
server (`localhost:8787`); override it with `FLIP_ADMIN_PROXY_TARGET`:

```bash
cd operator-ui
FLIP_ADMIN_PROXY_TARGET=http://127.0.0.1:8173 pnpm --filter liquidity-provider dev
```

Open http://localhost:5173 and enter the admin token
(`flip-local-admin-token`) at the auth prompt.

## 4. Complete setup through the UI wizard

The daemon starts unconfigured; the setup wizard drives
`/admin/v1/apply_setup_config`. Values that make the harness config pass
validation ("ready"):

- **Network**: `regtest`
- **Gateway**: admin URL `http://127.0.0.1:8175` (daemon on host) or
  `http://gatewayd:8175` (daemon in compose), credential `testpassword`
- **Chain observer**: bitcoind `http://127.0.0.1:18443` (host) or
  `http://bitcoind:18443` (compose), user/pass `bitcoin` / `bitcoin`
- **Relays**: `ws://127.0.0.1:8081` (host) or `ws://nostr-relay:8080` (compose)
- **Capacity**: mode `available_funds`, sources `gateway`

Alternatively apply the exact harness config over curl — see
`live_setup_config()` in
`crates/liquidity-manager-daemon/tests/integration_live_liquidity.rs`.

## Funding / mining helpers

```bash
alias btc='docker compose exec bitcoind bitcoin-cli -regtest -rpcuser=bitcoin -rpcpassword=bitcoin -rpcwallet=testwallet'
btc getnewaddress            # fresh address
btc sendtoaddress <addr> 1.0 # fund something (e.g. the daemon's deposit address)
btc -generate 11             # mine blocks to confirm (harness uses 11 finality blocks)
```

## Scope

This stack covers the admin/operator workflow (setup wizard, funds, health,
backups). The full liquidity-request flow additionally needs a target Fedimint
federation (fedimintd + DKG) and a signed FI request — that path is only
automated in the integration test:
`just flip-test-integration` on the PR branch (requires docker).

## Teardown

```bash
docker compose --profile daemon down -v   # -v drops chain + gateway + daemon state
```
