#!/usr/bin/env bash
# Bring up the local FMan stack for UI dev against a real daemon:
#   1. the fleet-manager daemon, built from this checkout — the operator HTTP
#      adapter it needs (--admin-http-bind/--admin-http-auth) lives in
#      crates/fman/core/src/admin_http.rs
#   2. the fleet-manager Vite dev server, proxied at the daemon
#
# Unlike FLIP, FMan needs no bitcoind/gatewayd/relay containers — its wallet
# joins external federations by invite code, not ones this stack stands up.
#
# Runs the Vite server in the foreground; Ctrl+C stops only the UI. The daemon
# keeps running — use down.sh to stop it too.
#
# Overridable via env (defaults shown):
#   FMAN_DAEMON_REPO=<repo root>                        daemon checkout to build
#   FMAN_ADMIN_PASSWORD=fman-local-admin-password       operator password
#   FMAN_DATA_DIR=/tmp/fman-dev-data                    daemon state dir
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.state"
mkdir -p "$STATE_DIR"

REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
FMAN_DAEMON_REPO="${FMAN_DAEMON_REPO:-$REPO_ROOT}"
FMAN_ADMIN_PASSWORD="${FMAN_ADMIN_PASSWORD:-fman-local-admin-password}"
FMAN_DATA_DIR="${FMAN_DATA_DIR:-/tmp/fman-dev-data}"
ADMIN_ADDR="127.0.0.1:8180"
DAEMON_BIN="$FMAN_DAEMON_REPO/target/debug/fleet-manager"
FMAN_CLI_BIN="$FMAN_DAEMON_REPO/target/debug/fman-cli"
DAEMON_PID_FILE="$STATE_DIR/daemon.pid"
DAEMON_LOG="$STATE_DIR/daemon.log"
PASSWORD_FILE="$STATE_DIR/admin-password"

log() { printf '\033[1;36m[fman:live]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[fman:live] %s\033[0m\n' "$*" >&2; exit 1; }

if curl -sf -m 3 "http://$ADMIN_ADDR/api/auth" -X POST -H 'content-type: application/json' -d '{}' >/dev/null 2>&1; then
  log "daemon already responding on $ADMIN_ADDR — reusing it."
else
  [ -d "$FMAN_DAEMON_REPO" ] || die "FMAN_DAEMON_REPO not found: $FMAN_DAEMON_REPO"

  if [ ! -x "$DAEMON_BIN" ] || [ ! -x "$FMAN_CLI_BIN" ]; then
    log "Fleet Manager binaries missing; building..."
    # The dev shell carries the pinned toolchain and the fedimint build inputs;
    # a bare `cargo` is not on PATH on a fresh macOS checkout.
    if command -v cargo >/dev/null 2>&1; then
      ( cd "$FMAN_DAEMON_REPO" && cargo build -p fman --bin fleet-manager -p fman-cli --bin fman-cli )
    else
      ( cd "$FMAN_DAEMON_REPO" && nix develop --command cargo build -p fman --bin fleet-manager -p fman-cli --bin fman-cli )
    fi
  fi

  mkdir -p "$FMAN_DATA_DIR"
  printf '%s' "$FMAN_ADMIN_PASSWORD" >"$PASSWORD_FILE"
  chmod 600 "$PASSWORD_FILE"

  log "starting daemon (admin-http=$ADMIN_ADDR data=$FMAN_DATA_DIR)..."
  FM_IN_DEVIMINT=1 perl -e 'use POSIX qw(setsid); setsid() or die "setsid: $!"; exec @ARGV or die "exec: $!"' \
    "$DAEMON_BIN" serve \
    --data-dir "$FMAN_DATA_DIR" \
    --bitcoind-url http://127.0.0.1:18443 \
    --bitcoind-username fman-ui-dev \
    --bitcoind-password fman-ui-dev \
    --manifold-environment development \
    --admin-http-bind "$ADMIN_ADDR" \
    --admin-http-auth password \
    --admin-http-password-file "$PASSWORD_FILE" \
    >"$DAEMON_LOG" 2>&1 &
  echo $! >"$DAEMON_PID_FILE"

  log "waiting for daemon to accept connections..."
  deadline=$(( SECONDS + 30 ))
  until curl -sf -m 3 "http://$ADMIN_ADDR/api/auth" -X POST -H 'content-type: application/json' -d '{}' >/dev/null 2>&1; do
    [ "$SECONDS" -ge "$deadline" ] && die "daemon did not come up in time; see $DAEMON_LOG"
    sleep 1
  done
  log "daemon up (logs: $DAEMON_LOG)."
fi

port=5174
holders="$(lsof -ti tcp:$port -sTCP:LISTEN 2>/dev/null || true)"
if [ -n "$holders" ]; then
  log "reclaiming port $port from an existing dev server (pids: $holders)..."
  echo "$holders" | xargs -r kill 2>/dev/null || true
fi
sleep 1

log "starting UI on http://localhost:5174 (proxy -> http://$ADMIN_ADDR)"
log "sign in with the operator password: $FMAN_ADMIN_PASSWORD"
log "Ctrl+C stops the UI only; run 'pnpm fman:live:down' to stop the daemon too."
cd "$SCRIPT_DIR/../.."
exec env FMAN_ADMIN_PROXY_TARGET="http://$ADMIN_ADDR" VITE_MOCKS=off pnpm --filter fman dev
