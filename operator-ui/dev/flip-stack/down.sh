#!/usr/bin/env bash
# Tear down the local FLIP stack started by up.sh: stops the daemon and the
# docker deps. By default also drops the docker volumes (chain + gateway +
# daemon-in-compose state); pass --keep-data to preserve them.
#
# The host daemon's data dir (FLIP_DATA_DIR, default /tmp/flip-dev-data) is
# left in place regardless — delete it manually for a truly clean slate.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.state"
DAEMON_PID_FILE="$STATE_DIR/daemon.pid"

COMPOSE_DOWN_ARGS=(down --remove-orphans -v)
for arg in "$@"; do
  case "$arg" in
    --keep-data) COMPOSE_DOWN_ARGS=(down --remove-orphans) ;;
    *) echo "unknown arg: $arg (supported: --keep-data)" >&2; exit 2 ;;
  esac
done

log() { printf '\033[1;36m[flip:live:down]\033[0m %s\n' "$*"; }

resolve_docker() {
  if command -v docker >/dev/null 2>&1; then echo docker; return; fi
  local app="/Applications/Docker.app/Contents/Resources/bin/docker"
  [ -x "$app" ] && { echo "$app"; return; }
  echo ""  # docker gone; skip compose teardown
}
DOCKER="$(resolve_docker)"

# --- stop the host daemon ---------------------------------------------------
stop_pid() {
  local pid="$1"
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.5; done
  kill -9 "$pid" 2>/dev/null || true
}

daemon_pid=""
if [ -f "$DAEMON_PID_FILE" ]; then
  pid="$(cat "$DAEMON_PID_FILE")"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then daemon_pid="$pid"; fi
  rm -f "$DAEMON_PID_FILE"
fi

# Fallback: find whatever is listening on the admin port (daemon started
# outside up.sh, e.g. by hand).
if [ -z "$daemon_pid" ]; then
  daemon_pid="$(lsof -ti tcp:8173 -sTCP:LISTEN 2>/dev/null | head -1 || true)"
fi

if [ -n "$daemon_pid" ]; then
  log "stopping daemon (pid $daemon_pid)..."
  stop_pid "$daemon_pid"
else
  log "no running daemon found on :8173; nothing to stop."
fi

# --- stop the UI dev server -------------------------------------------------
ui_pids="$(lsof -ti tcp:5173 -sTCP:LISTEN 2>/dev/null || true)"
ui_pids="$ui_pids $(lsof -ti tcp:5174 -sTCP:LISTEN 2>/dev/null || true)"
ui_pids="$(echo "$ui_pids" | tr ' ' '\n' | grep -E '^[0-9]+$' | sort -u || true)"
if [ -n "$ui_pids" ]; then
  log "stopping UI dev server (pids: $(echo "$ui_pids" | tr '\n' ' '))..."
  echo "$ui_pids" | while read -r p; do [ -n "$p" ] && stop_pid "$p"; done
else
  log "no UI dev server on :5173/:5174; nothing to stop."
fi

# --- stop docker deps -------------------------------------------------------
if [ -n "$DOCKER" ]; then
  log "stopping docker deps (${COMPOSE_DOWN_ARGS[*]})..."
  "$DOCKER" compose -f "$SCRIPT_DIR/docker-compose.yml" "${COMPOSE_DOWN_ARGS[@]}"
else
  log "docker CLI not found; skipped compose teardown."
fi

log "done."
