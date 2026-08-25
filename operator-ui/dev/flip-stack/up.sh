#!/usr/bin/env bash
# Bring up the full local FLIP stack for UI dev against a real daemon:
#   1. docker deps (bitcoind + gatewayd + nostr relay)
#   2. the FLIP daemon (built from the PR worktree if not already built)
#   3. the liquidity-provider Vite dev server, proxied at the daemon
#
# Runs the Vite server in the foreground; Ctrl+C stops only the UI. Docker and
# the daemon keep running — use down.sh to stop everything.
#
# Overridable via env (defaults shown):
#   FLIP_DAEMON_REPO=/Users/kc/Projects/df-flip-pr70   PR #70 worktree
#   FLIP_ADMIN_TOKEN=flip-local-admin-token            bootstrap admin token
#   FLIP_DATA_DIR=/tmp/flip-dev-data                   daemon SQLite/state dir
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.state"
mkdir -p "$STATE_DIR"

# The daemon now lives in this repo (crates/liquidity-manager-daemon), so build
# and run it from here by default. (Override to an external worktree if needed.)
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
FLIP_DAEMON_REPO="${FLIP_DAEMON_REPO:-$REPO_ROOT}"
FLIP_ADMIN_TOKEN="${FLIP_ADMIN_TOKEN:-flip-local-admin-token}"
FLIP_DATA_DIR="${FLIP_DATA_DIR:-/tmp/flip-dev-data}"
ADMIN_ADDR="127.0.0.1:8173"
PUBLIC_ADDR="127.0.0.1:8174"
DAEMON_BIN="$FLIP_DAEMON_REPO/target/debug/liquidity-manager-daemon"
DAEMON_PID_FILE="$STATE_DIR/daemon.pid"
DAEMON_LOG="$STATE_DIR/daemon.log"

log() { printf '\033[1;36m[flip:live]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[flip:live] %s\033[0m\n' "$*" >&2; exit 1; }

# --- docker binary (not always on PATH on macOS) ---------------------------
resolve_docker() {
  if command -v docker >/dev/null 2>&1; then echo docker; return; fi
  local app="/Applications/Docker.app/Contents/Resources/bin/docker"
  [ -x "$app" ] && { echo "$app"; return; }
  die "docker CLI not found (checked PATH and Docker.app). Is Docker installed?"
}
DOCKER="$(resolve_docker)"

# --- 1. docker dependency stack --------------------------------------------
log "starting docker deps (bitcoind + gatewayd + nostr relay)..."
"$DOCKER" compose -f "$SCRIPT_DIR/docker-compose.yml" up -d

log "waiting for gatewayd to report healthy (first start can take ~1 min)..."
deadline=$(( SECONDS + 180 ))
until "$DOCKER" compose -f "$SCRIPT_DIR/docker-compose.yml" ps --format '{{.Service}} {{.Health}}' \
        | grep -q 'gatewayd healthy'; do
  [ "$SECONDS" -ge "$deadline" ] && die "gatewayd did not become healthy in time; check: $DOCKER compose -f $SCRIPT_DIR/docker-compose.yml logs gatewayd"
  sleep 3
done
log "docker deps healthy."

# --- 2. FLIP daemon ---------------------------------------------------------
if curl -sf -m 3 "http://$ADMIN_ADDR/health" >/dev/null 2>&1; then
  log "daemon already responding on $ADMIN_ADDR — reusing it."
else
  [ -d "$FLIP_DAEMON_REPO" ] || die "FLIP_DAEMON_REPO not found: $FLIP_DAEMON_REPO (set it to your PR #70 checkout)"

  if [ ! -x "$DAEMON_BIN" ]; then
    log "daemon binary missing; building (macOS workarounds applied, see README)..."
    ( cd "$FLIP_DAEMON_REPO" \
      && GIT_CONFIG_COUNT=1 \
         GIT_CONFIG_KEY_0='url.https://github.com/.insteadOf' GIT_CONFIG_VALUE_0='ssh://git@github.com/' \
         CARGO_NET_GIT_FETCH_WITH_CLI=true \
         nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#pkg-config nixpkgs#cmake nixpkgs#libiconv \
           --command cargo build -p fedi-decentralized-liquidity-manager-daemon --bin liquidity-manager-daemon )
  fi

  mkdir -p "$FLIP_DATA_DIR"
  log "starting daemon (admin=$ADMIN_ADDR public=$PUBLIC_ADDR data=$FLIP_DATA_DIR)..."
  # Detach into its own session (via perl setsid — macOS has no setsid binary)
  # so Ctrl+C on the foreground Vite process group can't take the daemon down.
  # down.sh stops it explicitly via the pidfile / port lookup.
  FM_IN_DEVIMINT=1 perl -e 'use POSIX qw(setsid); setsid() or die "setsid: $!"; exec @ARGV or die "exec: $!"' \
    "$DAEMON_BIN" run daemon \
    --manifold-environment development \
    --data-dir "$FLIP_DATA_DIR" \
    --admin-bind-address "$ADMIN_ADDR" \
    --public-bind-address "$PUBLIC_ADDR" \
    --bootstrap-admin-token "$FLIP_ADMIN_TOKEN" \
    >"$DAEMON_LOG" 2>&1 &
  echo $! >"$DAEMON_PID_FILE"

  log "waiting for daemon health..."
  deadline=$(( SECONDS + 30 ))
  until curl -sf -m 3 "http://$ADMIN_ADDR/health" >/dev/null 2>&1; do
    [ "$SECONDS" -ge "$deadline" ] && die "daemon health check failed; see $DAEMON_LOG"
    sleep 1
  done
  log "daemon healthy (logs: $DAEMON_LOG)."
fi

# --- 3. UI dev server (foreground) -----------------------------------------
# Reclaim the UI ports from any leftover dev server so we always land on 5173
# (a stale vite otherwise silently pushes us to 5174).
for port in 5173 5174; do
  holders="$(lsof -ti tcp:$port -sTCP:LISTEN 2>/dev/null || true)"
  if [ -n "$holders" ]; then
    log "reclaiming port $port from an existing dev server (pids: $holders)..."
    echo "$holders" | xargs -r kill 2>/dev/null || true
  fi
done
sleep 1

log "starting UI on http://localhost:5173 (proxy -> http://$ADMIN_ADDR)"
log "log in with admin token: $FLIP_ADMIN_TOKEN"
log "Ctrl+C stops the UI only; run 'pnpm flip:live:down' to stop docker + daemon."
cd "$SCRIPT_DIR/../.."
exec env FLIP_ADMIN_PROXY_TARGET="http://$ADMIN_ADDR" VITE_MOCKS=off pnpm --filter flip dev
