#!/usr/bin/env bash
# Tear down the local FMan stack started by up.sh: stops the daemon and the UI.
#
# The daemon's data dir (FMAN_DATA_DIR, default /tmp/fman-dev-data) is left in
# place regardless — delete it manually for a truly clean slate.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.state"
DAEMON_PID_FILE="$STATE_DIR/daemon.pid"

log() { printf '\033[1;36m[fman:live:down]\033[0m %s\n' "$*"; }

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

if [ -z "$daemon_pid" ]; then
  daemon_pid="$(lsof -ti tcp:8180 -sTCP:LISTEN 2>/dev/null | head -1 || true)"
fi

if [ -n "$daemon_pid" ]; then
  log "stopping daemon (pid $daemon_pid)..."
  stop_pid "$daemon_pid"
else
  log "no running daemon found on :8180; nothing to stop."
fi

ui_pids="$(lsof -ti tcp:5174 -sTCP:LISTEN 2>/dev/null || true)"
if [ -n "$ui_pids" ]; then
  log "stopping UI dev server (pids: $(echo "$ui_pids" | tr '\n' ' '))..."
  echo "$ui_pids" | while read -r p; do [ -n "$p" ] && stop_pid "$p"; done
else
  log "no UI dev server on :5174; nothing to stop."
fi

log "done."
