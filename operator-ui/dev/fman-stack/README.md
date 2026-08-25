# FMan local dev stack — test the fleet-manager UI against a real daemon

Runs the `fleet-manager` binary from the `fman` Cargo package so the operator UI
can talk to it directly instead of the mock server. Unlike FLIP, FMan needs no
bitcoind/gatewayd/relay dependencies of its own — its payment wallet joins
external Fedimint federations by invite code, not ones this stack stands up.

The daemon's operator HTTP API (`--admin-http-bind`/`--admin-http-auth`,
`POST /api/admin` + `POST /api/auth`) is
[`crates/fman/core/src/admin_http.rs`](../../../crates/fman/core/src/admin_http.rs),
specified by [SPEC-operator-http](../../../crates/fman/specs/SPEC-operator-http.md).
It builds from this checkout — no separate worktree is needed.

## Quick start

From the `operator-ui/` root:

```bash
pnpm fman:live       # daemon + UI on :5174 (foreground)
pnpm fman:live:down  # stop daemon + UI
```

`fman:live` ([up.sh](up.sh)) builds the daemon if its binary is missing, writes
a generated operator password to a gitignored state file, runs the daemon,
onboards a fresh disposable data root as a new FMan through its local admin
socket, then waits for the detached password-auth HTTP adapter on `:8180`.
It reclaims port 5174 from any leftover dev server and runs the Vite dev server
in the foreground with the proxy pointed at the real daemon. The daemon runs in
its own session, so **Ctrl+C stops only the UI** — the daemon keeps running.
`fman:live:down` ([down.sh](down.sh)) stops both.

Overridable env (defaults shown):

```bash
FMAN_DAEMON_REPO=<repo root>                         # daemon checkout to build
FMAN_ADMIN_PASSWORD=fman-local-admin-password        # operator password
FMAN_DATA_DIR=/tmp/fman-dev-data                     # daemon state dir
```

The daemon PID and logs live in `.state/` (gitignored) — tail
`dev/fman-stack/.state/daemon.log` if it won't come up.

## Manual steps (what up.sh automates)

```bash
cargo build -p fman --bin fleet-manager

mkdir -p /tmp/fman-dev-data
echo -n 'fman-local-admin-password' > /tmp/fman-admin-password
chmod 600 /tmp/fman-admin-password

./target/debug/fleet-manager serve \
  --data-dir /tmp/fman-dev-data \
  --bitcoind-url http://127.0.0.1:18443 \
  --bitcoind-username fman-ui-dev \
  --bitcoind-password fman-ui-dev \
  --manifold-environment development \
  --admin-http-bind 127.0.0.1:8180 \
  --admin-http-auth password \
  --admin-http-password-file /tmp/fman-admin-password
```

Point the UI at it:

```bash
cd operator-ui
FMAN_ADMIN_PROXY_TARGET=http://127.0.0.1:8180 pnpm --filter fman dev
```

Open http://localhost:5174, sign in with the operator password, and complete the resumable onboarding wizard.

## Scope

This stack covers the operator flows the daemon actually answers over HTTP:
Overview (earnings), Seats (observe + decommission), Wallet (read the payment
federations), Offer (`ShowPlans`/`SetPrice`), and Backup
(`ShowMnemonic`). Payment-federation membership is not an operator choice —
the daemon exposes `ListPaymentFederations` read-only — so the UI offers no add
or remove action.

**Money-out is not covered.** The daemon's payout verbs
(`PayoutDestination`/`SetPayoutDestination`/`SweepPaymentFees`/`CollectGuardianFees`/
`SweepGuardianFees`) have no dashboard surface yet — use the operator CLI.

**Setup is the first screen on a fresh data root.** The daemon serves the whole
resumable onboarding workflow and dashboard on `:8180`
([`SPEC-operator-http`](../../../crates/fman/specs/SPEC-operator-http.md), *The
onboarding phase*). `up.sh` does not bypass it through the Unix socket.

This stack also does not stand up a target Fedimint federation or an FI client —
those are exercised by the FMan's own integration tests, not this UI stack.

## Docker

`docker-compose.yml` next to this file is a placeholder for a future
`fman-daemon` container — no nix-built image exists yet (no `just
fman-docker-load` target on any branch). Until one does, use the host-cargo
path above.
