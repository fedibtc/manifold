# MG local dev stack — test the fleet-manager UI against a real daemon

Runs the `fleet-manager` binary from the `mg` Cargo package so the operator UI
can talk to it directly instead of the mock server. Unlike FLIP, MG needs no
bitcoind/gatewayd/relay dependencies of its own — its payment wallet joins
external Fedimint federations by invite code, not ones this stack stands up.

The daemon's operator HTTP API (`--admin-http-bind`/`--admin-http-auth`,
`POST /api/admin` + `POST /api/auth`) is
[`crates/mg/core/src/admin_http.rs`](../../../crates/mg/core/src/admin_http.rs),
specified by [SPEC-operator-http](../../../crates/mg/specs/SPEC-operator-http.md).
It builds from this checkout — no separate worktree is needed.

## Quick start

From the `operator-ui/` root:

```bash
pnpm mg:live       # daemon + UI on :5174 (foreground)
pnpm mg:live:down  # stop daemon + UI
```

`mg:live` ([up.sh](up.sh)) builds the daemon if its binary is missing, writes
a generated operator password to a gitignored state file, runs the daemon,
onboards a fresh disposable data root as a new MG through its local admin
socket, then waits for the detached password-auth HTTP adapter on `:8180`.
It reclaims port 5174 from any leftover dev server and runs the Vite dev server
in the foreground with the proxy pointed at the real daemon. The daemon runs in
its own session, so **Ctrl+C stops only the UI** — the daemon keeps running.
`mg:live:down` ([down.sh](down.sh)) stops both.

Overridable env (defaults shown):

```bash
MG_DAEMON_REPO=<repo root>                         # daemon checkout to build
MG_ADMIN_PASSWORD=mg-local-admin-password        # operator password
MG_DATA_DIR=/tmp/mg-dev-data                     # daemon state dir
```

The daemon PID and logs live in `.state/` (gitignored) — tail
`dev/mg-stack/.state/daemon.log` if it won't come up.

## Manual steps (what up.sh automates)

```bash
cargo build -p mg --bin fleet-manager

mkdir -p /tmp/mg-dev-data
echo -n 'mg-local-admin-password' > /tmp/mg-admin-password
chmod 600 /tmp/mg-admin-password

./target/debug/fleet-manager serve \
  --data-dir /tmp/mg-dev-data \
  --bitcoind-url http://127.0.0.1:18443 \
  --bitcoind-username mg-ui-dev \
  --bitcoind-password mg-ui-dev \
  --manifold-environment development \
  --admin-http-bind 127.0.0.1:8180 \
  --admin-http-auth password \
  --admin-http-password-file /tmp/mg-admin-password
```

Point the UI at it:

```bash
cd operator-ui
MG_ADMIN_PROXY_TARGET=http://127.0.0.1:8180 pnpm --filter mg dev
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
([`SPEC-operator-http`](../../../crates/mg/specs/SPEC-operator-http.md), *The
onboarding phase*). `up.sh` does not bypass it through the Unix socket.

This stack also does not stand up a target Fedimint federation or an FA client —
those are exercised by the MG's own integration tests, not this UI stack.

## Docker

`docker-compose.yml` next to this file is a placeholder for a future
`mg-daemon` container — no nix-built image exists yet (no `just
mg-docker-load` target on any branch). Until one does, use the host-cargo
path above.
